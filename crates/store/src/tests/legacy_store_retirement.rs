//! #1221 R5-I: 旧 iroh store の退役の台帳。課金の外の他人の projection 行と旧保護の成人向けの hash を、1 回 128 行以内で
//! 台帳へ移すか回収し、本人の行の hash は残す。退役の判定は writer の切替の後の保護移行の終端と全 kind の終端。

use super::*;
use crate::{
    LEGACY_STORE_KINDS, LEGACY_STORE_PAGE, ObjectProjectionRow, PROTECTED_MIGRATION_KINDS,
};
use kukuri_core::{AssetRef, AssetRole};

const LOCAL: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn row(index: usize, author: &str, derived_at: i64) -> ObjectProjectionRow {
    let body = BlobHash::new(format!("{:064x}", index * 2 + 1));
    let attachment = BlobHash::new(format!("{:064x}", index * 2 + 2));
    ObjectProjectionRow {
        object_id: EnvelopeId::from(format!("post-{index:05}")),
        topic_id: "retirement".into(),
        channel_id: "public".into(),
        author_pubkey: author.into(),
        created_at: index as i64,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::BlobText {
            hash: body.clone(),
            mime: "text/plain".into(),
            bytes: 1,
        },
        content: Some(format!("body {index}")),
        attachments: vec![AssetRef {
            hash: attachment,
            mime: "image/png".into(),
            bytes: 1,
            role: AssetRole::ImageOriginal,
        }],
        repost_of: None,
        content_labels: vec!["adult".into()],
        source_replica_id: ReplicaId::new("topic::retirement"),
        source_key: format!("objects/post-{index:05}/state"),
        source_envelope_id: EnvelopeId::from(format!("post-{index:05}")),
        source_blob_hash: Some(body),
        source_docs_author: None,
        derived_at,
        projection_version: 2,
    }
}

async fn ledger_last_used(store: &SqliteStore, object_id: &str) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT last_used_at FROM remote_content_cache WHERE kind = 'projection' AND cache_key = ?1",
    )
    .bind(object_id)
    .fetch_optional(store.pool())
    .await
    .expect("ledger row")
}

async fn marker(store: &SqliteStore, hash: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT is_protected FROM adult_media_hashes WHERE blob_hash = ?1")
        .bind(hash)
        .fetch_optional(store.pool())
        .await
        .expect("marker")
}

/// 課金の外の他人の行は、最近のものを台帳へ移し(内容は呼出し元が写す)、古いものを回収する。本人の行と台帳にある行は
/// そのまま。旧保護の成人向けの hash は、参照の残るものだけが残る。1 ページは 128 行以内で、開き直すと続きから読む。
#[tokio::test]
async fn legacy_projections_move_to_the_ledger_or_are_reclaimed_in_pages() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("kukuri.db");
    let store = SqliteStore::connect_file(&path).await?;
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    let other = "f".repeat(64);
    let mut rows = Vec::new();
    for index in 0..300 {
        let (author, derived_at) = match index % 3 {
            0 => (LOCAL, now - 30 * DAY_MS),
            1 => (other.as_str(), now - DAY_MS),
            _ => (other.as_str(), now - 30 * DAY_MS),
        };
        rows.push(row(index, author, derived_at));
    }
    // 旧同期と本人の書込みは、課金の外の行として入った。
    store.put_object_projections(rows.clone()).await?;
    // R5-A の後に入った他人の行は台帳にある。
    let charged = row(300, &other, now - 30 * DAY_MS);
    store.put_remote_object_projection(charged.clone()).await?;

    let first = store.retire_legacy_projection_page(LOCAL, "").await?;
    assert!(!first.done);
    store
        .finish_legacy_store_page("legacy_projection", &first.cursor, false)
        .await?;
    store.close().await;
    let store = SqliteStore::connect_file(&path).await?;
    let mut blobs = first.blobs;
    let mut pages = 1;
    loop {
        let (cursor, done) = store.legacy_store_position("legacy_projection").await?;
        assert!(!done);
        let page = store.retire_legacy_projection_page(LOCAL, &cursor).await?;
        pages += 1;
        blobs.extend(page.blobs);
        store
            .finish_legacy_store_page("legacy_projection", &page.cursor, page.done)
            .await?;
        if page.done {
            break;
        }
    }
    assert!(
        pages >= 3,
        "301 rows need at least three pages of {LEGACY_STORE_PAGE}"
    );
    for (index, row) in rows.iter().enumerate() {
        // 表示(読取り)は最後の利用の時刻を進めるので、台帳の行を先に読む。
        let last_used = ledger_last_used(&store, row.object_id.as_str()).await;
        let kept = store.get_object_projection(&row.object_id).await?;
        let (body, attachment) = (
            format!("{:064x}", index * 2 + 1),
            format!("{:064x}", index * 2 + 2),
        );
        match index % 3 {
            0 => {
                assert!(kept.is_some(), "own row {index} is kept");
                assert_eq!(last_used, None);
            }
            1 => {
                assert!(kept.is_some(), "recent row {index} moves to the ledger");
                assert_eq!(
                    last_used,
                    Some(row.derived_at),
                    "the last use is when it was derived"
                );
                assert!(blobs.contains(&body) && blobs.contains(&attachment));
            }
            _ => {
                assert!(kept.is_none(), "old row {index} is reclaimed");
                assert!(!blobs.contains(&body));
            }
        }
    }
    assert!(
        store
            .get_object_projection(&charged.object_id)
            .await?
            .is_some(),
        "a row already in the ledger stays"
    );

    // 旧保護の成人向けの hash は、行の参照が残るものだけを非保護で残す。
    loop {
        let (cursor, _) = store.legacy_store_position("adult_marker").await?;
        let (next, done) = store.retire_legacy_adult_marker_page(&cursor).await?;
        store
            .finish_legacy_store_page("adult_marker", &next, done)
            .await?;
        if done {
            break;
        }
    }
    for index in 0..300 {
        let attachment = format!("{:064x}", index * 2 + 2);
        match index % 3 {
            0 => assert_eq!(marker(&store, &attachment).await, Some(0), "own {index}"),
            1 => assert_eq!(marker(&store, &attachment).await, Some(0), "recent {index}"),
            _ => assert_eq!(marker(&store, &attachment).await, None, "old {index}"),
        }
    }
    let protected: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM adult_media_hashes WHERE is_protected = 1")
            .fetch_one(store.pool())
            .await?;
    assert_eq!(
        protected, 0,
        "no legacy protected marker is left outside the ledger"
    );
    store.close().await;
    Ok(())
}

/// 退役は、writer の切替の後に保護移行の全 kind が終端へ達し、この台帳の全 kind が終端へ達したときだけ。
/// 旧 store の無い account は、移行を済んだものとして切り替える。
#[tokio::test]
async fn retirement_waits_for_the_migration_after_the_switch() -> anyhow::Result<()> {
    let store = SqliteStore::connect_memory().await?;
    assert!(!store.legacy_store_retirable().await?);
    for kind in PROTECTED_MIGRATION_KINDS {
        store
            .finish_protected_migration_page(kind, "", true)
            .await?;
    }
    let switched = store.switch_writer_if_migrated().await?.expect("switched");
    for kind in LEGACY_STORE_KINDS {
        store.finish_legacy_store_page(kind, "", true).await?;
    }
    // 保護移行の終端が切替より前なら、まだ退役させない。
    sqlx::query("UPDATE protected_migration SET caught_up_at = ?1")
        .bind(switched)
        .execute(store.pool())
        .await?;
    assert!(!store.legacy_store_retirable().await?);
    for kind in PROTECTED_MIGRATION_KINDS {
        sqlx::query("UPDATE protected_migration SET caught_up_at = ?1 WHERE kind = ?2")
            .bind(switched + 1)
            .bind(kind)
            .execute(store.pool())
            .await?;
    }
    assert!(store.legacy_store_retirable().await?);
    store
        .finish_legacy_store_page("own_entries", "e00", false)
        .await?;
    assert!(!store.legacy_store_retirable().await?);

    let fresh = SqliteStore::connect_memory().await?;
    assert!(fresh.settle_without_legacy_store().await?.is_some());
    assert!(fresh.protected_migration_caught_up_at().await?.is_some());
    assert!(fresh.legacy_store_position("own_entries").await?.1);
    Ok(())
}
