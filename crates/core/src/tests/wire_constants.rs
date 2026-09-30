//! wire prefix 定数の値凍結テスト(WP-S3 T5)。
//!
//! 期待値は生リテラル直書き(wire モジュールの定数を期待値側に使うと、
//! 定数と一緒に書き換えられて凍結の意味が失われる)。

use crate::TopicId;
use crate::wire::{
    ACCOUNT_SYNC_REPLICA_PREFIX, ACCOUNT_SYNC_TOPIC_PREFIX, DM_TOPIC_PREFIX, HINT_TOPIC_PREFIX,
    PRIVATE_CHANNEL_TOPIC_PREFIX, hint_topic_id,
};

#[test]
fn wire_prefixes_match_frozen_literals() {
    assert_eq!(HINT_TOPIC_PREFIX, "hint/");
    assert_eq!(PRIVATE_CHANNEL_TOPIC_PREFIX, "private/");
    assert_eq!(DM_TOPIC_PREFIX, "kukuri:dm:");
    assert_eq!(ACCOUNT_SYNC_TOPIC_PREFIX, "kukuri:account:");
    assert_eq!(ACCOUNT_SYNC_REPLICA_PREFIX, "account::v1::");
}

#[test]
fn hint_topic_id_prepends_frozen_prefix() {
    assert_eq!(
        hint_topic_id(&TopicId::new("kukuri:topic:demo")).as_str(),
        "hint/kukuri:topic:demo"
    );
}
