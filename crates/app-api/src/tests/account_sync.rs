//! 本人の端末間の account 同期の起動と停止（#1218 AC-2、ADR 0061 §6）。

use super::*;

#[tokio::test]
async fn account_sync_starts_privately_and_stops_with_the_runtime() {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(MemoryDocsSync::default());
    let transport = Arc::new(FakeTransport::new("account-sync", FakeNetwork::default()));
    let keys = generate_keys();
    let account = keys.derive_account_sync();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport.clone(),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        keys,
    );
    let hint = kukuri_core::wire::hint_topic_id(account.hint_topic())
        .as_str()
        .to_string();
    let read = || docs.query_replica(account.replica_id(), DocQuery::All);

    assert!(read().await.is_err(), "登録前は公開の導出で開かない");
    app.start_account_sync()
        .await
        .expect("account 同期を始める");
    assert!(read().await.is_ok(), "登録した秘密で開ける");
    let subscribed = transport.subscribed_topics().await.expect("購読の一覧");
    assert!(subscribed.contains(&hint), "hint の topic を購読する");
    assert!(
        normalize_topics(subscribed).is_empty(),
        "診断の topic の一覧に出ない"
    );
    assert!(
        app.leased_topics().await.is_empty(),
        "公開の topic の lease にならない"
    );

    app.shutdown().await;
    assert!(
        !transport
            .subscribed_topics()
            .await
            .expect("購読の一覧")
            .contains(&hint),
        "停止で hint の購読を抜ける"
    );
}
