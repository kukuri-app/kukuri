//! クライアント側 P2P transport の部品層。
//!
//! ここに置くもの: gossip transport(`IrohGossipTransport`)と gossip hint 転送、
//! endpoint 構築部品(bind / builder)、独自 ticket(`<endpoint_id>@<host:port>`)、
//! ピア台帳(`peers` — docs-sync / blob-service が共有する接続候補とリトライ状態)、
//! テスト用 `FakeTransport`。
//!
//! ここに置かないもの: 実運用 iroh ノード全体(endpoint / gossip / router / blobs /
//! docs)の所有権と composition は `kukuri-iroh-node`(WP-H2)。本 crate はそこへ
//! 部品を貸す側で、ノードのライフサイクルを持たない。ピア台帳まわりの挙動差分
//! (bool 返却 / reapply 等)は呼び出し側 crate に残す(`peers.rs` の doc 参照)。
//! endpoint 構築部品を足すときは本 crate、ノードの起動・停止・再構成に関わるもの
//! を足すときは `kukuri-iroh-node` が置き場。

mod config;
mod diagnostics;
mod discovery;
mod fake;
mod iroh;
mod peers;
mod receive_binding;
mod status_changes;
#[cfg(test)]
mod test_support;
mod tickets;
mod traits;
pub mod work_admission;

pub use config::*;
pub use discovery::*;
pub use fake::*;
pub use iroh::*;
pub use peers::*;
pub use receive_binding::*;
pub use status_changes::*;
pub use tickets::*;
pub use traits::*;
