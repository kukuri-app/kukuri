//! 公開参照の索引（#1632 AC-6、ADR 0063 §8）。Community Node へ保持端末の検索を頼める hash（`is_public_blob`）が、
//! native と同じ操作列で同じに決まり、remote の行の回収で外れること。導入前の行を小分けに取り込み、reload の後も続きから
//! 進めること。版 1 の database を開くと、既存の行を残して索引の store を足し、版 1 の接続が閉じるまで移行中として待つこと。
//!
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer` から実行する。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use kukuri_core::EnvelopeId;
use kukuri_store::parity::public_refs::{check_public_blob_refs, hash, post, public};
use kukuri_store::{ContentCacheStore, ObjectProjectionRow, ObjectProjectionStore};
use wasm_bindgen_test::wasm_bindgen_test;

use crate::IndexedDbCache;
use crate::account_tests::account_id;
use crate::content_cache::{META, OBJECTS, PUBLIC_REFS, create_stores};
use crate::idb::{self, Mode, js_error};
use crate::rows::{self, Txn, text};

#[wasm_bindgen_test]
async fn public_refs_follow_the_native_store() {
    let cache = IndexedDbCache::open(&account_id()).await.expect("cache");
    check_public_blob_refs(&cache).await;
}

/// remote の投稿の行を回収すると、その投稿の公開参照も外れる。
#[wasm_bindgen_test]
async fn a_reclaimed_remote_post_leaves_the_public_refs() {
    // remote の投稿の行 1 件と blob 1 件が収まらない容量。
    let cache = IndexedDbCache::start(&account_id(), Some(4 * 1024))
        .await
        .expect("cache");
    cache
        .put_remote_object_projection(post("p1", "public", &hash(1), &[&hash(2)]))
        .await
        .expect("remote post");
    assert_eq!(public(&cache, &[1, 2]).await, [true, true]);
    // 新しい remote の内容を置くと、古い非保護の行（remote の投稿）から回収する。
    assert!(
        cache
            .put_remote_content("blob", "big", "blob", &[0; 3 * 1024])
            .await
            .unwrap()
    );
    assert!(
        cache
            .get_object_projection(&EnvelopeId::from("p1"))
            .await
            .unwrap()
            .is_none(),
        "the remote post is reclaimed"
    );
    assert_eq!(public(&cache, &[1, 2]).await, [false, false]);
}

/// 導入前の行（索引の無い行）は 1 回 128 行までの取込みで種類ごとに進み、reload の後も続きから進む。取込みの回数は
/// 行数を 128 で割った分と種類の数だけで、1 回に読む行は増えない。
#[wasm_bindgen_test]
async fn old_rows_are_taken_in_bounded_steps_and_resume_after_a_reload() {
    for total in [20_u32, 200, 2_000] {
        let account = account_id();
        let cache = IndexedDbCache::open(&account).await.expect("cache");
        let posts = (0..total)
            .map(|index| {
                let id = format!("old-{index:05}");
                post(&id, "public", &hash(index * 2), &[&hash(index * 2 + 1)])
            })
            .collect();
        cache.put_object_projections(posts).await.expect("posts");
        // 導入前の database と同じく、索引も取込みの位置も無い状態にする。
        cache
            .run(|db| async move {
                let tx = Txn::begin(&db.idb, &[PUBLIC_REFS, META], Mode::Write)?;
                rows::store(&tx, PUBLIC_REFS)?.clear().map_err(js_error)?;
                rows::delete(&tx, META, &text("public_refs_backfill"))?;
                tx.commit().await
            })
            .await
            .expect("forget the index");
        assert_eq!(public(&cache, &[1]).await, [false]);

        assert!(!cache.backfill_public_blob_refs_step(128).await.unwrap());
        drop(cache);
        let cache = IndexedDbCache::open(&account).await.expect("reload");
        let mut steps = 2;
        while !cache.backfill_public_blob_refs_step(128).await.unwrap() {
            steps += 1;
        }
        assert_eq!(steps, total / 128 + 3, "{total} rows");
        let last = total - 1;
        assert_eq!(
            public(&cache, &[0, 1, last * 2, last * 2 + 1]).await,
            [true; 4],
            "{total} rows"
        );
    }
}

/// 版 1 の database（索引の store が無い）を開くと、既存の行を残して索引の store を足し、取込みで公開参照が入る。
/// 版 1 の接続（更新前の tab）が残る間は失敗にせず、移行中と知らせて閉じるまで待つ（ADR 0059 §1）。新しく作る database
/// は移行に数えない。
#[wasm_bindgen_test]
async fn a_version_1_database_gains_the_index_and_keeps_its_rows() {
    let migrations = Rc::new(RefCell::new(Vec::new()));
    idb::watch_migrations(Some(Rc::new({
        let migrations = migrations.clone();
        move |migrating| migrations.borrow_mut().push(migrating)
    })));
    let account = account_id();
    let db = idb::open(&format!("kukuri-cache-v1-{account}"), 1, |db| {
        create_stores(db)?;
        db.delete_object_store(PUBLIC_REFS)
    })
    .await
    .expect("version 1");
    let tx = Txn::begin(&db, &[OBJECTS], Mode::Write).expect("tx");
    rows::put(
        &tx,
        OBJECTS,
        &post("legacy", "public", &hash(1), &[&hash(2)]),
        &[],
    )
    .expect("row");
    tx.commit().await.expect("commit");
    assert!(migrations.borrow().is_empty(), "a new database");

    let opened = Rc::new(RefCell::new(None));
    wasm_bindgen_futures::spawn_local({
        let (opened, account) = (opened.clone(), account.clone());
        async move { *opened.borrow_mut() = Some(IndexedDbCache::open(&account).await) }
    });
    n0_future::time::sleep(Duration::from_millis(300)).await;
    assert!(
        opened.borrow().is_none(),
        "waits for the version 1 connection"
    );
    assert_eq!(*migrations.borrow(), [true]);
    db.close();
    let cache = loop {
        if let Some(opened) = opened.borrow_mut().take() {
            break opened.expect("upgrade");
        }
        n0_future::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(*migrations.borrow(), [true, false]);
    idb::watch_migrations(None);
    let kept = cache
        .run(|db| async move {
            let tx = Txn::begin(&db.idb, &[OBJECTS], Mode::Read)?;
            rows::get::<ObjectProjectionRow>(&tx, OBJECTS, &text("legacy")).await
        })
        .await
        .expect("read");
    assert!(kept.is_some(), "the version 1 row is kept");
    assert_eq!(public(&cache, &[1, 2]).await, [false, false]);
    while !cache.backfill_public_blob_refs_step(128).await.unwrap() {}
    assert_eq!(public(&cache, &[1, 2]).await, [true, true]);
}
