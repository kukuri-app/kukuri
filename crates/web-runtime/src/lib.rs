//! Web クライアントの入口と、ブラウザ専用の保存 adapter（ADR 0056 §2）。ブラウザの main thread で動く。
#![cfg(target_family = "wasm")]
// tokio・std の時刻と task を直接使わない（ADR 0056 §3）。
#![cfg_attr(not(test), warn(clippy::disallowed_methods))]

mod content_cache;
mod idb;

#[cfg(test)]
mod browser_tests;

pub use content_cache::IndexedDbCache;
