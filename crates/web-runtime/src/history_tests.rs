//! #1211 AC-3: 移行の履歴の読み出し（保護参照の範囲の record を位置の次から読む）が、IndexedDB でも native と同じ順・
//! 同じ範囲になる。headless の Chromium で browser_tests と一緒に実行する。

use kukuri_store::ContentCacheStore;
use wasm_bindgen_test::wasm_bindgen_test;

use crate::IndexedDbCache;
use crate::browser_tests::{account, delete_database};
use crate::content_cache::test_hooks;

/// #1211 AC-3: 保護参照の範囲の record を、位置の次から (参照, replica, key, author) の順に native と同じく読む。保護の
/// 無い record・範囲の外の参照は返さず、1 回に読む行は件数の上限まで。
#[wasm_bindgen_test]
async fn protected_records_are_read_after_a_position_like_native() {
    use kukuri_core::AccountHistoryCursor;
    let account = account("history");
    let cache = IndexedDbCache::open(&account).await.expect("open");
    // Web の保存は payload の key・docs author が引数と合うことを確かめる。
    let payload = |key: &str, author: &str| {
        serde_json::to_vec(&serde_json::json!({
            "key": key, "value": "", "content_hash": format!("hash-{key}"),
            "content_len": 0, "docs_author": author,
        }))
        .expect("payload")
    };
    let at = |reference: &str, replica: &str, key: &str, author: &str| AccountHistoryCursor {
        reference: reference.into(),
        replica: replica.into(),
        key: key.into(),
        author: author.into(),
    };
    let own = [
        ("bucket::v1::author::6f776e::20000", "indexes/profile/1"),
        ("bucket::v1::topic::61::20000", "objects/b/envelope"),
        ("bucket::v1::topic::61::20001", "objects/a/envelope"),
        ("topic::x", "objects/c/envelope"),
    ];
    for (replica, key) in own {
        cache
            .put_owned_record(replica, key, "own", &payload(key, "own"))
            .await
            .expect("own");
    }
    let other = payload("objects/z/envelope", "other");
    let old = payload("objects/old/envelope", "legacy");
    let put = cache.put_remote_record(
        "bucket::v1::topic::61::20000",
        "objects/z/envelope",
        "other",
        &other,
    );
    assert!(put.await.expect("other"));
    let put = cache.put_remote_record("topic::x", "objects/old/envelope", "legacy", &old);
    assert!(put.await.expect("legacy"));
    cache
        .add_protected_ref(
            "own:old",
            "record",
            "topic::x\0objects/old/envelope\0legacy",
        )
        .await
        .expect("legacy ref");

    test_hooks::ROWS_READ.set(0);
    let first = cache
        .protected_records_after("own_docs", &at("own_docs", "", "", ""), 3)
        .await
        .expect("first");
    assert!(test_hooks::ROWS_READ.get() <= 3);
    assert_eq!(
        first
            .iter()
            .map(|(position, _)| (position.replica.as_str(), position.key.as_str()))
            .collect::<Vec<_>>(),
        own[..3]
    );
    assert_eq!(first[0].0, at("own_docs", own[0].0, own[0].1, "own"));
    assert_eq!(first[0].1, payload(own[0].1, "own"));
    let rest = cache
        .protected_records_after("own_docs", &first[2].0, 3)
        .await;
    let rest = rest.expect("rest");
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].0.replica, "topic::x");
    let end = cache
        .protected_records_after("own_docs", &rest[0].0, 3)
        .await;
    assert!(end.expect("end").is_empty());

    // replica の先頭への seek と、replica の後ろへの seek。
    let seek = cache
        .protected_records_after("own_docs", &at("own_docs", own[2].0, "", ""), 8)
        .await
        .expect("seek");
    assert_eq!(seek.len(), 2);
    let past = cache
        .protected_records_after("own_docs", &at("own_docs", own[1].0, "\u{FFFF}", ""), 8)
        .await
        .expect("past");
    assert_eq!(past[0].0.replica, own[2].0);

    let legacy = cache
        .protected_records_after("own:", &at("own:", "", "", ""), 8)
        .await
        .expect("legacy");
    assert_eq!(legacy.len(), 1);
    assert_eq!(
        legacy[0].0,
        at("own:old", "topic::x", "objects/old/envelope", "legacy")
    );
    drop(cache);
    delete_database(&account).await.expect("delete");
}
