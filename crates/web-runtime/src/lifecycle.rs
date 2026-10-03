//! browser の lifecycle の event を、host の中断・復帰の入口へ渡す（ADR 0059 §5）。Web だけの retry の loop は作らない。

use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{Document, Event, EventTarget, VisibilityState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    /// pagehide・freeze・offline: WebRTC の交渉の世代を終える。
    Suspend,
    /// 可視・online・pageshow・resume: 経路の確かめ直しと、送信待ちの再送など。
    Resume,
}

/// 登録した 1 つの listener（対象・event の名前・callback）。
type Listener = (EventTarget, &'static str, Closure<dyn Fn(Event)>);

/// 登録した listener。drop で外す（止めた後の event は渡さない）。
pub(crate) struct LifecycleListeners(Vec<Listener>);

impl Drop for LifecycleListeners {
    fn drop(&mut self) {
        for (target, name, closure) in &self.0 {
            let _ =
                target.remove_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
        }
    }
}

/// window と document の lifecycle の event を `on` へ渡す。`visibilitychange` は可視になったときだけ復帰とする。
pub(crate) fn listen_lifecycle(
    on: impl Fn(Lifecycle) + 'static,
) -> Result<LifecycleListeners, JsValue> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let on = Rc::new(on);
    let events: [(&EventTarget, &'static str, Option<Lifecycle>); 7] = [
        (&window, "pagehide", Some(Lifecycle::Suspend)),
        (&window, "offline", Some(Lifecycle::Suspend)),
        (&document, "freeze", Some(Lifecycle::Suspend)),
        (&window, "online", Some(Lifecycle::Resume)),
        (&window, "pageshow", Some(Lifecycle::Resume)),
        (&document, "resume", Some(Lifecycle::Resume)),
        (&document, "visibilitychange", None),
    ];
    let mut listeners = Vec::with_capacity(events.len());
    for (target, name, lifecycle) in events {
        let (on, document): (_, Document) = (on.clone(), document.clone());
        let closure = Closure::<dyn Fn(Event)>::new(move |_: Event| match lifecycle {
            Some(lifecycle) => on(lifecycle),
            None if document.visibility_state() == VisibilityState::Visible => {
                on(Lifecycle::Resume)
            }
            None => {}
        });
        target.add_event_listener_with_callback(name, closure.as_ref().unchecked_ref())?;
        listeners.push((target.clone(), name, closure));
    }
    Ok(LifecycleListeners(listeners))
}
