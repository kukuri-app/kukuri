//! 診断 topic 正規化の凍結テスト(WP-S3 T5)。
//!
//! hint/ の strip と private/・kukuri:dm: の除外は診断表示の契約。
//! 期待値の prefix は生リテラル(core::wire の定数を期待値側に使わない)。

use super::*;

#[test]
fn normalize_topic_name_strips_hint_prefix() {
    assert_eq!(
        normalize_topic_name("hint/kukuri:topic:demo".to_string()),
        Some("kukuri:topic:demo".to_string())
    );
}

#[test]
fn normalize_topic_name_passes_plain_topics_through() {
    assert_eq!(
        normalize_topic_name("kukuri:topic:demo".to_string()),
        Some("kukuri:topic:demo".to_string())
    );
}

#[test]
fn normalize_topic_name_excludes_private_channel_and_dm_topics() {
    assert_eq!(normalize_topic_name("private/chan-1".to_string()), None);
    assert_eq!(normalize_topic_name("kukuri:dm:abc123".to_string()), None);
    // hint/ を剥がした後に private/・kukuri:dm: 判定が効くことも契約。
    assert_eq!(
        normalize_topic_name("hint/private/chan-1".to_string()),
        None
    );
    assert_eq!(
        normalize_topic_name("hint/kukuri:dm:abc123".to_string()),
        None
    );
}

#[test]
fn normalize_topic_name_excludes_account_sync_topics() {
    assert_eq!(
        normalize_topic_name("kukuri:account:abc123".to_string()),
        None
    );
    assert_eq!(
        normalize_topic_name("hint/kukuri:account:abc123".to_string()),
        None
    );
}

#[test]
fn normalize_topics_dedupes_and_preserves_order() {
    let normalized = normalize_topics(vec![
        "hint/kukuri:topic:a".to_string(),
        "kukuri:topic:a".to_string(),
        "private/chan-1".to_string(),
        "kukuri:topic:b".to_string(),
    ]);
    assert_eq!(
        normalized,
        vec!["kukuri:topic:a".to_string(), "kukuri:topic:b".to_string()]
    );
}
