//! 接続の経過と警告を browser の console へ出す（#2043）。Web では tracing の出力先が他に無い。

use std::fmt::{Debug, Write};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use wasm_bindgen::JsValue;

const PREFIX: &str = "[kukuri.runtime]";

/// `kukuri_connectivity` の経過と、kukuri と iroh-docs の警告以上だけを出す（iroh 本体の経路の log は量が多い）。
fn is_logged(metadata: &Metadata<'_>) -> bool {
    let target = metadata.target();
    target == "kukuri_connectivity"
        || (*metadata.level() <= Level::WARN
            && (target.starts_with("kukuri") || target.starts_with("iroh_docs")))
}

struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        let _ = if field.name() == "message" {
            write!(self.0, " {value:?}")
        } else {
            write!(self.0, " {}={value:?}", field.name())
        };
    }
}

struct ConsoleLayer;

impl<S: Subscriber> Layer<S> for ConsoleLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        let mut line = Line(format!(
            "{PREFIX} {} {}",
            metadata.level(),
            metadata.target()
        ));
        event.record(&mut line);
        let line = JsValue::from_str(&line.0);
        if *metadata.level() <= Level::WARN {
            web_sys::console::warn_1(&line);
        } else {
            web_sys::console::info_1(&line);
        }
    }
}

/// 起動の入口から呼ぶ。2 回目以降は何もしない。
pub(crate) fn install() {
    let _ = tracing_subscriber::registry()
        .with(ConsoleLayer.with_filter(filter_fn(is_logged)))
        .try_init();
}
