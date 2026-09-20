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
