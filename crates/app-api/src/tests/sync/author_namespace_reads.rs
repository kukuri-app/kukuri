//! #1407: 他人の author を読む 3 つの経路(表示名の背景読取り・author の lease・プロフィールの列)は、
//! 手元に無い `author::<pubkey>` の namespace を作らない。

use super::*;
use crate::service::author_state_support::hydrate_author_profile;
use crate::service::profile_timeline_support::profile_timeline_page;

#[tokio::test]
async fn reading_another_author_does_not_create_its_namespace() {
    let node = kukuri_iroh_node::IrohDocsNode::memory()
        .await
        .expect("docs node");
    let docs = Arc::new(kukuri_docs_sync::IrohDocsSync::new(node.clone()));
    let local = generate_keys();
    let local_pubkey = local.public_key_hex();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        local,
    );
    let other = generate_keys().public_key_hex();

    hydrate_author_profile(&app.services, &local_pubkey, &other)
        .await
        .expect("background profile read");
    hydrate_author_state(
        &app.services,
        &local_pubkey,
        &other,
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("author lease read");
    profile_timeline_page(&app.services, &other, None, None, 20, &BTreeSet::new())
        .await
        .expect("profile column");

    assert_eq!(
        node.docs()
            .list()
            .await
            .expect("list namespaces")
            .count()
            .await,
        0
    );
    docs.shutdown().await;
    node.shutdown().await.expect("shutdown");
}
