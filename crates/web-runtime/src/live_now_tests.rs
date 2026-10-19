//! 配信中の Live の行（#1879）。版 3 の database を開くと、移行中と知らせ、既存の Live の行を残して配信中の Live の store
//! を足すこと。
//! 配信中の一覧の中身は store の parity の scenario（`live_now_sessions_match_between_backends`）が確かめる。
//!
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer` から実行する。

use std::cell::RefCell;
use std::rc::Rc;

use kukuri_core::{BlobHash, LiveSessionStatus, ReplicaId};
use kukuri_store::{LiveGameProjectionStore, LiveSessionProjectionRow};
use wasm_bindgen_test::wasm_bindgen_test;

use crate::IndexedDbCache;
use crate::account_tests::account_id;
use crate::content_cache::{LIVE_NOW, LIVE_SESSIONS, create_stores};
use crate::idb::{self, Mode};
use crate::rows::{self, Txn};

fn session(session_id: &str, started_at: i64) -> LiveSessionProjectionRow {
    LiveSessionProjectionRow {
        session_id: session_id.into(),
        revision: 1,
        topic_id: "topic".into(),
        channel_id: "thread".into(),
        host_pubkey: "a".repeat(64),
        title: session_id.into(),
        description: String::new(),
        status: LiveSessionStatus::Live,
        started_at,
        ended_at: None,
        updated_at: started_at,
        source_replica_id: ReplicaId::new("topic::topic"),
        source_key: format!("live/{session_id}/manifest"),
        manifest_blob_hash: BlobHash::new("9".repeat(64)),
        derived_at: started_at,
        projection_version: 1,
        viewer_count: 0,
        start_post_id: None,
    }
}

/// 版 3 の database（配信中の Live の store が無い）を開くと、移行中と知らせ（画面は「データの移行中です」を出す。ADR 0059
/// §1）、既存の Live の行を残して store を足す。更新の前から配信中だった Live は、その行が次に書き換わるまで配信中の一覧に
/// 出ない（2026-10-19 のユーザー判断 4）。
#[wasm_bindgen_test]
async fn a_version_3_database_gains_the_live_now_store_and_keeps_its_rows() {
    let account = account_id();
    let db = idb::open(&format!("kukuri-cache-v1-{account}"), 3, |db| {
        create_stores(db)?;
        db.delete_object_store(LIVE_NOW)
    })
    .await
    .expect("version 3");
    let tx = Txn::begin(&db, &[LIVE_SESSIONS], Mode::Write).expect("tx");
    rows::put(&tx, LIVE_SESSIONS, &session("legacy", 1), &[]).expect("row");
    tx.commit().await.expect("commit");
    db.close();

    let migrations = Rc::new(RefCell::new(Vec::new()));
    idb::watch_migrations(Some(Rc::new({
        let migrations = migrations.clone();
        move |migrating| migrations.borrow_mut().push(migrating)
    })));
    let cache = IndexedDbCache::open(&account).await.expect("upgrade");
    idb::watch_migrations(None);
    assert_eq!(*migrations.borrow(), [true, false]);
    let ids = async |cache: &IndexedDbCache| {
        cache
            .list_channel_live_now_sessions("topic", "thread", 8)
            .await
            .expect("live now")
            .into_iter()
            .map(|row| row.session_id)
            .collect::<Vec<_>>()
    };
    assert!(
        cache
            .get_live_session("topic", "legacy")
            .await
            .expect("read")
            .is_some(),
        "the version 3 row is kept"
    );
    assert!(ids(&cache).await.is_empty());
    cache
        .upsert_live_session_cache(session("new", 2))
        .await
        .expect("a live row");
    assert_eq!(ids(&cache).await, ["new"]);
}
