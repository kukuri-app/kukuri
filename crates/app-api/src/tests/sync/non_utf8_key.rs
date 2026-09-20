//! public topic の replica に UTF-8 でない key の entry が 1 件あっても、表示と操作が止まらないことを固定する。
//!
//! iroh-docs の key は任意の byte 列で、public topic の replica は topic id を知る誰もが書ける。
//! `DocsSync` は key を `String` で扱うので、この entry は iroh-docs へ直接書く。

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_utf8_key_does_not_stop_the_timeline() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack = TestIrohStack::new(&dir.path().join("non-utf8-key")).await;
    let app = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack);
    let topic = "kukuri:topic:non-utf8-key";

    app.create_post(topic, "visible post", None)
        .await
        .expect("create post");

    // この node が開いている replica は topic の replica だけではないので、全部に 1 件ずつ置く。
    let docs = stack._node.docs();
    let author = docs.author_default().await.expect("default author");
    let namespaces = docs
        .list()
        .await
        .expect("list replicas")
        .collect::<Vec<_>>()
        .await;
    assert!(!namespaces.is_empty());
    for namespace in namespaces {
        let (namespace_id, _) = namespace.expect("replica entry");
        let doc = docs
            .open(namespace_id)
            .await
            .expect("open replica")
            .expect("replica exists");
        doc.set_bytes(author, b"objects/\xff\xfe/state".to_vec(), b"x".to_vec())
            .await
            .expect("write a key that is not utf8");
    }

    // 反映が空の利用者(同じ replica を読む別の store)は、空ページの走査で `objects/` を読む。
    let fresh = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack);
    let timeline = fresh
        .list_timeline(topic, None, 20)
        .await
        .expect("list_timeline must not fail because of one non-utf8 key");
    assert!(
        timeline
            .items
            .iter()
            .any(|post| post.content == "visible post"),
        "the readable post stays visible"
    );
}

// #1257: UTF-8 でない key が、余裕を超えて時系列の索引の新しい側を埋めていても、反映が空の利用者の
// 取得側の照合(`list_timeline` が使う `reconcile_timeline_range`)は、読める投稿を索引から反映する。
// UTF-8 でない key は読み出しの中で飛ばされて件数に入らないので、「索引が尽きたか」を返った件数で判定すると、
// 索引が空だと誤判定していた。購読タスクの全件走査に結果を埋めさせないよう、照合を直接呼ぶ。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_utf8_keys_on_the_newest_side_of_the_time_index_do_not_hide_the_timeline() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack = TestIrohStack::new(&dir.path().join("non-utf8-index")).await;
    let app = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack);
    let topic = "kukuri:topic:non-utf8-index";

    for index in 0..3 {
        app.create_post(topic, format!("visible post {index}").as_str(), None)
            .await
            .expect("create post");
    }

    // `0xff` は数字より後ろに並ぶので、降順の読み出しでは必ず先頭に来る。読み出しの余裕(64 件)を超える数を置く。
    let docs = stack._node.docs();
    let author = docs.author_default().await.expect("default author");
    let namespaces = docs
        .list()
        .await
        .expect("list replicas")
        .collect::<Vec<_>>()
        .await;
    for namespace in namespaces {
        let (namespace_id, _) = namespace.expect("replica entry");
        let doc = docs
            .open(namespace_id)
            .await
            .expect("open replica")
            .expect("replica exists");
        for index in 0..120usize {
            let mut key = b"indexes/timeline/\xff\xfe".to_vec();
            key.extend_from_slice(format!("{index:04}").as_bytes());
            doc.set_bytes(author, key, b"x".to_vec())
                .await
                .expect("write a key that is not utf8");
        }
    }

    // 反映が空の利用者(同じ replica を読む別の store)。購読タスクの全件走査より先に、取得側の照合が
    // 索引の新しい側を読む。
    let fresh = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack);
    let timeline = fresh
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, 20)
        .await
        .expect("the reconcile must not fail because of non-utf8 keys");
    assert_eq!(
        timeline, 3,
        "the readable posts are reflected from the index"
    );
}
