//! 公開 channel の一覧の行（#1840）。他人が作った行は容量の不足で回収され、自分が作った行は残ること。版 2 の
//! database を開くと、既存の行を残して一覧の store を足すこと。
//!
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer` から実行する。

use kukuri_core::EnvelopeId;
use kukuri_store::parity::public_refs::{hash, post};
use kukuri_store::{
    ContentCacheStore, ObjectProjectionRow, PublicChannelOrder, PublicChannelRow,
    PublicChannelStore,
};
use wasm_bindgen_test::wasm_bindgen_test;

use crate::IndexedDbCache;
use crate::account_tests::account_id;
use crate::content_cache::{OBJECTS, PUBLIC_CHANNELS, create_stores};
use crate::idb::{self, Mode};
use crate::rows::{self, Txn, text};

fn channel(channel_id: &str, created_at: i64) -> PublicChannelRow {
    PublicChannelRow {
        channel_id: channel_id.into(),
        topic_id: "topic".into(),
        title: channel_id.into(),
        creator_pubkey: "a".repeat(64),
        created_at,
        first_post_id: None,
        last_post_at: created_at,
        revision: 0,
        deleted: false,
    }
}

async fn listed(cache: &IndexedDbCache) -> Vec<String> {
    cache
        .list_public_channels("topic", PublicChannelOrder::Created, None, 20)
        .await
        .expect("list")
        .items
        .into_iter()
        .map(|row| row.channel_id)
        .collect()
}

/// 他人が作った行は remote の内容として数え、新しい内容に場所を空けるときに回収する。自分が作った行は残る。
#[wasm_bindgen_test]
async fn a_remote_public_channel_row_is_reclaimed_and_an_own_row_stays() {
    // 他人の行 1 件（約 0.9 KiB）と blob 1 件（約 3.8 KiB）が収まらず、blob だけなら収まる容量。
    let cache = IndexedDbCache::start(&account_id(), Some(4 * 1024))
        .await
        .expect("cache");
    cache
        .put_public_channel(channel("pc-own", 1), false)
        .await
        .expect("own row");
    cache
        .put_public_channel(channel("pc-remote", 2), true)
        .await
        .expect("remote row");
    assert_eq!(listed(&cache).await, ["pc-remote", "pc-own"]);
    assert!(
        cache
            .put_remote_content("blob", "big", "blob", &[0; 3840])
            .await
            .unwrap()
    );
    assert!(
        cache
            .get_public_channel_row("pc-remote")
            .await
            .unwrap()
            .is_none(),
        "the remote row is reclaimed"
    );
    assert_eq!(listed(&cache).await, ["pc-own"]);
}

/// 版 2 の database（一覧の store が無い）を開くと、既存の行を残して一覧の store を足す。
#[wasm_bindgen_test]
async fn a_version_2_database_gains_the_public_channel_store_and_keeps_its_rows() {
    let account = account_id();
    let db = idb::open(&format!("kukuri-cache-v1-{account}"), 2, |db| {
        create_stores(db)?;
        db.delete_object_store(PUBLIC_CHANNELS)
    })
    .await
    .expect("version 2");
    let tx = Txn::begin(&db, &[OBJECTS], Mode::Write).expect("tx");
    rows::put(&tx, OBJECTS, &post("legacy", "public", &hash(1), &[]), &[]).expect("row");
    tx.commit().await.expect("commit");
    db.close();

    let cache = IndexedDbCache::open(&account).await.expect("upgrade");
    let kept = cache
        .run(|db| async move {
            let tx = Txn::begin(&db.idb, &[OBJECTS], Mode::Read)?;
            rows::get::<ObjectProjectionRow>(&tx, OBJECTS, &text("legacy")).await
        })
        .await
        .expect("read");
    assert_eq!(
        kept.map(|row| row.object_id),
        Some(EnvelopeId::from("legacy")),
        "the version 2 row is kept"
    );
    cache
        .put_public_channel(channel("pc-new", 1), false)
        .await
        .expect("a row in the new store");
    assert_eq!(listed(&cache).await, ["pc-new"]);
}
