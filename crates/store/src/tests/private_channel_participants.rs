//! #1221 R5-H: owner が受け取った private channel の参加・退出の表。新しい record だけで置き換え、
//! 退出は channel の全 epoch に及ぶ。参加中の pubkey は cursor で重複なく読む(両 backend で同じ結果)。

use super::*;
use anyhow::Result;

fn row(
    epoch: &str,
    pubkey: &str,
    left_at: Option<i64>,
    updated_at: i64,
) -> PrivateChannelParticipantRow {
    PrivateChannelParticipantRow {
        channel_id: "channel".into(),
        epoch_id: epoch.into(),
        participant_pubkey: pubkey.into(),
        left_at,
        updated_at,
    }
}

async fn participant_table_contract(store: &dyn SocialProjectionStore) -> Result<()> {
    for (epoch, pubkey) in [
        ("e1", "a"),
        ("e1", "b"),
        ("e1", "c"),
        ("e2", "a"),
        ("e2", "d"),
    ] {
        assert!(
            store
                .put_private_channel_participant(row(epoch, pubkey, None, 10))
                .await?
        );
    }
    // 古い record は置き換えない。
    assert!(
        !store
            .put_private_channel_participant(row("e1", "b", Some(5), 5))
            .await?
    );
    assert_eq!(
        store
            .count_private_channel_participants("channel", "e1")
            .await?,
        3
    );
    // 全 epoch の参加中の pubkey を重複なく、cursor で読む。
    let first = store
        .list_private_channel_participants("channel", None, "", 2)
        .await?;
    assert_eq!(first, vec!["a".to_string(), "b".to_string()]);
    let rest = store
        .list_private_channel_participants("channel", None, "b", 2)
        .await?;
    assert_eq!(rest, vec!["c".to_string(), "d".to_string()]);
    assert_eq!(
        store
            .list_private_channel_participants("channel", Some("e2"), "", 10)
            .await?,
        vec!["a".to_string(), "d".to_string()]
    );
    // 現 epoch での退出は channel からの退出で、古い epoch の行も退出にする。
    assert!(
        store
            .put_private_channel_participant(row("e2", "a", Some(20), 20))
            .await?
    );
    assert_eq!(
        store
            .list_private_channel_participants("channel", None, "", 10)
            .await?,
        vec!["b".to_string(), "c".to_string(), "d".to_string()]
    );
    assert_eq!(
        store
            .count_private_channel_participants("channel", "e1")
            .await?,
        2
    );
    // 退出より後の参加(再参加)は戻す。
    assert!(
        store
            .put_private_channel_participant(row("e2", "a", None, 30))
            .await?
    );
    assert_eq!(
        store
            .count_private_channel_participants("channel", "e2")
            .await?,
        2
    );
    assert!(
        store
            .list_private_channel_participants("other", None, "", 10)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn private_channel_participants_keep_the_newest_record_and_page_by_pubkey() -> Result<()> {
    participant_table_contract(&SqliteStore::connect_memory().await?).await?;
    participant_table_contract(&MemoryStore::default()).await
}
