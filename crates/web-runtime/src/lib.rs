//! Web クライアントの入口と、ブラウザ専用の保存 adapter（ADR 0056 §2）。ブラウザの main thread で動く。
#![cfg(target_family = "wasm")]
// tokio・std の時刻と task を直接使わない（ADR 0056 §3）。
#![cfg_attr(not(test), warn(clippy::disallowed_methods))]

mod account;
mod actor;
mod client;
mod content_cache;
mod idb;
mod lifecycle;
mod rows;
mod tab_lock;
mod vault;

#[cfg(test)]
mod account_tests;
#[cfg(test)]
mod browser_tests;
#[cfg(test)]
mod client_tests;

pub use content_cache::IndexedDbCache;
pub use idb::StorageFailure;
pub use vault::BrowserStorage;
