//! 画像・動画のブックマークとキャッシュの一覧（#1949）。版 4 の database を開くと両方の store を足すこと（版 5 は既存の
//! database に bookmark の store を作らなかった）と、キャッシュの一覧が cache の内容とブックマークに従うこと。
//!
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer` から実行する。

use kukuri_core::{AssetRef, AssetRole, BlobHash, MediaOrigin};
use kukuri_store::{BookmarkedMediaRow, ContentCacheStore, MediaBookmarkStore, MediaShelf};
use wasm_bindgen_test::wasm_bindgen_test;

use crate::IndexedDbCache;
use crate::account_tests::account_id;
use crate::content_cache::{CACHED_MEDIA, MEDIA_BOOKMARKS, create_stores};
use crate::idb;

fn media(index: u64, role: AssetRole) -> BookmarkedMediaRow {
    let hash = format!("{index:064x}");
    BookmarkedMediaRow {
        hash: hash.clone(),
        source: MediaOrigin {
            topic_id: "topic".into(),
            object_id: format!("post-{index}"),
            author_pubkey: "a".repeat(64),
            created_at: 1,
        },
        bookmarked_from_object_id: format!("post-{index}"),
        attachments: vec![AssetRef {
            hash: BlobHash::new(hash),
            mime: "image/png".into(),
            bytes: 1,
            role,
            origin: None,
        }],
        content_labels: Vec::new(),
        bookmarked_at: 1,
    }
}

async fn cached(cache: &IndexedDbCache) -> Vec<String> {
    cache
        .list_bookmarked_media(MediaShelf::Cache, None, false)
        .await
        .expect("cache list")
        .into_iter()
        .map(|row| row.hash)
        .collect()
}

#[wasm_bindgen_test]
async fn a_version_4_database_gains_media_shelves_and_the_cache_list_follows_the_cache() {
    let account = account_id();
    let db = idb::open(&format!("kukuri-cache-v1-{account}"), 4, |db| {
        create_stores(db)?;
        db.delete_object_store(MEDIA_BOOKMARKS)?;
        db.delete_object_store(CACHED_MEDIA)
    })
    .await
    .expect("version 4");
    db.close();
    let cache = IndexedDbCache::open(&account).await.expect("upgrade");

    let image = media(1, AssetRole::ImageOriginal);
    cache
        .record_cached_media(image.clone())
        .await
        .expect("record");
    assert!(
        cached(&cache).await.is_empty(),
        "cache に無い画像は載せない"
    );
    cache
        .put_remote_content("blob", &image.hash, "blob", b"image")
        .await
        .expect("blob");
    cache
        .record_cached_media(image.clone())
        .await
        .expect("record");
    assert_eq!(cached(&cache).await, [image.hash.clone()]);

    cache
        .put_bookmarked_media(image.clone())
        .await
        .expect("bookmark");
    assert!(
        cached(&cache).await.is_empty(),
        "ブックマークするとキャッシュから外す"
    );
    assert_eq!(
        cache
            .list_bookmarked_media(MediaShelf::Bookmarks, None, false)
            .await
            .expect("bookmarks")
            .len(),
        1
    );
    cache
        .remove_bookmarked_media(&image.hash)
        .await
        .expect("unbookmark");
    assert_eq!(
        cached(&cache).await,
        [image.hash.clone()],
        "外すとキャッシュへ戻す"
    );

    let mut video = media(2, AssetRole::VideoManifest);
    let poster = media(3, AssetRole::VideoPoster).attachments.remove(0);
    let poster_hash = poster.hash.as_str().to_owned();
    video.attachments.push(poster);
    cache
        .put_remote_content("blob", &poster_hash, "blob", b"poster")
        .await
        .expect("poster");
    cache
        .record_cached_media(video.clone())
        .await
        .expect("video");
    assert_eq!(
        cached(&cache).await.len(),
        2,
        "poster だけ置いた動画も載せる"
    );
    cache
        .remove_cached_media(&video.hash)
        .await
        .expect("remove video");
    assert_eq!(cached(&cache).await, [image.hash.clone()]);
    assert!(
        !cache
            .has_remote_content("blob", &poster_hash)
            .await
            .expect("poster read"),
        "削除で保護の無い poster も消す"
    );
}
