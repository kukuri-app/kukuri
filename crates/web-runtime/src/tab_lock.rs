//! origin の中で runtime を動かす tab を 1 つにする lock（Web Locks の `kukuri-runtime-v1`。ADR 0059 §4）。
//! web-sys の Web Locks は unstable の cfg を要るので、`navigator.locks.request` を直接呼ぶ。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Function, Object, Promise, Reflect};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

const NAME: &str = "kukuri-runtime-v1";

/// 持っている lock。drop で手放す。
pub(crate) struct RuntimeLock {
    release: Function,
}

impl Drop for RuntimeLock {
    fn drop(&mut self) {
        let _ = self.release.call0(&JsValue::NULL);
    }
}

/// lock を取る。`steal` でなければ、他の tab が持つ間は取らずに `None` を返す（`ifAvailable`）。`steal` なら持ち主から
/// 奪う（利用者の明示の操作のときだけ）。取った lock を他の tab に奪われたら `on_lost` を呼ぶ。
pub(crate) async fn acquire(
    steal: bool,
    on_lost: impl FnOnce() + 'static,
) -> Result<Option<RuntimeLock>, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let locks = Reflect::get(&window.navigator(), &"locks".into())?;
    let request: Function = Reflect::get(&locks, &"request".into())?.dyn_into()?;
    let options = Object::new();
    let option = if steal { "steal" } else { "ifAvailable" };
    Reflect::set(&options, &option.into(), &JsValue::TRUE)?;
    // 持っている間の promise。resolve で手放す。
    let mut release = None;
    let held = Promise::new(&mut |resolve, _| release = Some(resolve));
    let release = release.ok_or("the promise executor did not run")?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let decided = Rc::new(RefCell::new(Some(sender)));
    let granted = Rc::new(Cell::new(false));
    let callback = {
        let (decided, granted) = (decided.clone(), granted.clone());
        Closure::once_into_js(move |lock: JsValue| -> JsValue {
            granted.set(!lock.is_null());
            if let Some(sender) = decided.borrow_mut().take() {
                let _ = sender.send(granted.get());
            }
            if granted.get() {
                held.into()
            } else {
                JsValue::UNDEFINED
            }
        })
    };
    let settled: Promise = request
        .call3(&locks, &NAME.into(), &options, &callback)?
        .dyn_into()?;
    wasm_bindgen_futures::spawn_local(async move {
        // 手放す（resolve）と成功、奪われると AbortError で失敗する。callback の前に終わったら、取れなかったとする。
        let result = JsFuture::from(settled).await;
        if let Some(sender) = decided.borrow_mut().take() {
            let _ = sender.send(false);
        } else if granted.get() && result.is_err() {
            on_lost();
        }
    });
    if receiver.await.map_err(|_| "the lock request was dropped")? {
        Ok(Some(RuntimeLock { release }))
    } else {
        Ok(None)
    }
}
