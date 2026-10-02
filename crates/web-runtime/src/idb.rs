//! IndexedDB の request と transaction を future で待つ。JS の object は呼んだ task の中だけで持つ（ADR 0056 §4）。
//!
//! 結果は success の event の callback で channel へ送り、待つ task は同じ event の後の microtask で再開する。
//! transaction はその間も有効なので、読んだ結果に続けて同じ transaction へ request を出せる。

use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use tokio::sync::oneshot;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::{Closure, JsValue};
use web_sys::{IdbCursorWithValue, IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransaction};

pub(crate) fn js_error(value: JsValue) -> anyhow::Error {
    anyhow!("indexeddb: {value:?}")
}

/// 成否の 2 つの callback を付け、どちらかが呼ばれるまで待つ。
async fn settle(
    set: impl Fn(Option<&js_sys::Function>, Option<&js_sys::Function>),
) -> Result<bool> {
    let (sender, receiver) = oneshot::channel();
    let sender = Rc::new(Cell::new(Some(sender)));
    let callback = |ok: bool| {
        let sender = sender.clone();
        Closure::<dyn FnMut()>::new(move || {
            if let Some(sender) = sender.take() {
                let _ = sender.send(ok);
            }
        })
    };
    let (success, failure) = (callback(true), callback(false));
    set(
        Some(success.as_ref().unchecked_ref()),
        Some(failure.as_ref().unchecked_ref()),
    );
    let ok = receiver.await;
    set(None, None);
    ok.map_err(|_| anyhow!("indexeddb callback dropped"))
}

/// request の結果。失敗した request は transaction を中断させる（既定の動作）。
pub(crate) async fn done(request: &IdbRequest) -> Result<JsValue> {
    let ok = settle(|success, failure| {
        request.set_onsuccess(success);
        request.set_onerror(failure);
    })
    .await?;
    if ok {
        request.result().map_err(js_error)
    } else {
        let error = request.error().map_err(js_error)?;
        Err(js_error(error.map(JsValue::from).unwrap_or(JsValue::NULL)))
    }
}

/// transaction の確定を待つ。中断（quota 超過・request の失敗）では何も書かれていない。
pub(crate) async fn committed(transaction: &IdbTransaction) -> Result<()> {
    let ok = settle(|success, failure| {
        transaction.set_oncomplete(success);
        transaction.set_onabort(failure);
    })
    .await?;
    if ok {
        Ok(())
    } else {
        Err(js_error(
            transaction
                .error()
                .map(JsValue::from)
                .unwrap_or(JsValue::NULL),
        ))
    }
}

/// cursor の次の行。`continue_` の後も同じ request が結果を返す。
pub(crate) async fn next(request: &IdbRequest) -> Result<Option<IdbCursorWithValue>> {
    let value = done(request).await?;
    Ok((!value.is_null()).then(|| value.unchecked_into()))
}

/// database を開き、版が上がるときは `upgrade` で object store と索引を作る。
pub(crate) async fn open(
    name: &str,
    version: u32,
    upgrade: fn(&IdbDatabase) -> std::result::Result<(), JsValue>,
) -> Result<IdbDatabase> {
    let factory = web_sys::window()
        .ok_or_else(|| anyhow!("indexeddb needs a window"))?
        .indexed_db()
        .map_err(js_error)?
        .ok_or_else(|| anyhow!("indexeddb is unavailable"))?;
    let request: IdbOpenDbRequest = factory.open_with_u32(name, version).map_err(js_error)?;
    let upgrading = request.clone();
    let on_upgrade = Closure::<dyn FnMut()>::new(move || {
        let created = upgrading
            .result()
            .and_then(|db| upgrade(&db.unchecked_into()));
        if created.is_err()
            && let Some(transaction) = upgrading.transaction()
        {
            let _ = transaction.abort();
        }
    });
    request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
    let db = done(&request).await;
    request.set_onupgradeneeded(None);
    Ok(db?.unchecked_into())
}
