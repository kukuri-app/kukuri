use anyhow::Result;

use crate::{PrivateIndexGrant, SqliteStore};

#[tokio::test]
async fn grants_round_robin_and_stop_without_loading_the_whole_table() -> Result<()> {
    let store = SqliteStore::connect_memory().await?;
    for id in ["a", "b", "c"] {
        store
            .save_private_index_grant(&PrivateIndexGrant {
                base_url: "https://node".into(),
                topic_id: "topic".into(),
                channel_id: id.into(),
                applied_epoch_id: "e1".into(),
            })
            .await?;
    }
    let first = store
        .next_private_index_grant("https://node", 1)
        .await?
        .unwrap();
    let second = store
        .next_private_index_grant("https://node", 2)
        .await?
        .unwrap();
    assert_eq!((&first.channel_id[..], &second.channel_id[..]), ("a", "b"));
    store.mark_private_index_grant_applied(&first, "e2").await?;
    store
        .stop_private_index_grant("https://node", "topic", "c")
        .await?;
    store
        .stop_private_index_grants_for_channel("topic", "b")
        .await?;
    assert_eq!(
        store
            .next_private_index_grant("https://node", 3)
            .await?
            .unwrap()
            .channel_id,
        "a"
    );
    assert_eq!(
        store
            .next_private_index_grant("https://node", 4)
            .await?
            .unwrap()
            .channel_id,
        "a",
        "the stopped channel is discarded instead of being sent to the CN"
    );
    store
        .stop_private_index_grants_for_node("https://node")
        .await?;
    assert!(
        store
            .next_private_index_grant("https://node", 5)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn private_grant_survives_restart_but_node_withdrawal_does_not_resume_it() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("account.db");
    let grant = PrivateIndexGrant {
        base_url: "https://node".into(),
        topic_id: "topic".into(),
        channel_id: "room".into(),
        applied_epoch_id: "e1".into(),
    };
    let store = SqliteStore::connect_file(&path).await?;
    store.save_private_index_grant(&grant).await?;
    drop(store);
    let store = SqliteStore::connect_file(&path).await?;
    assert_eq!(
        store.next_private_index_grant("https://node", 1).await?,
        Some(grant)
    );
    store
        .stop_private_index_grants_for_node("https://node")
        .await?;
    drop(store);
    let store = SqliteStore::connect_file(&path).await?;
    assert!(
        store
            .next_private_index_grant("https://node", 2)
            .await?
            .is_none()
    );
    Ok(())
}
