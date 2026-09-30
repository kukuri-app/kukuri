//! wire prefix 定数(凍結境界)。
//!
//! これらの値は gossip topic 名・診断表示・topic 除外判定として複数 crate に
//! またがる契約であり、変更はネットワーク分断・診断不一致になる。
//! 値そのものの凍結は src/tests/wire_constants.rs が生リテラル比較で担う
//! (このモジュールを参照せずに固定するのが目的なので、テスト側の期待値を
//! この定数に置き換えてはならない)。

use crate::TopicId;

/// gossip hint 配送 topic の prefix。実 topic 名の前置により hint 用の
/// 並行 gossip スワームを分離する。
pub const HINT_TOPIC_PREFIX: &str = "hint/";

/// private channel の hint topic prefix(docs-sync の replica 命名と対)。
/// 診断表示からの除外判定にも使われる。
pub const PRIVATE_CHANNEL_TOPIC_PREFIX: &str = "private/";

/// pairwise DM topic の prefix(HKDF 派生 hex 64 桁が続く)。
/// 診断表示からの除外判定にも使われる。
pub const DM_TOPIC_PREFIX: &str = "kukuri:dm:";

/// 本人の端末間の account 同期の hint topic の prefix（アカウントの秘密鍵から導出した hex 64 桁が続く。
/// ADR 0061）。公開の topic・通知の受信 route（`receive::v1::`）と区別する。
pub const ACCOUNT_SYNC_TOPIC_PREFIX: &str = "kukuri:account:";

/// 本人の端末間の account 同期の docs の replica の prefix（hex 64 桁が続く。ADR 0061）。
/// 公開の namespace の導出の対象外。
pub const ACCOUNT_SYNC_REPLICA_PREFIX: &str = "account::v1::";

/// topic に対応する gossip hint topic id を返す。
pub fn hint_topic_id(topic: &TopicId) -> TopicId {
    TopicId::new(format!("{HINT_TOPIC_PREFIX}{}", topic.as_str()))
}
