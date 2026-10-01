//! #1407: 旧 store の退役の後に、更新前の版が読取りで作った空の namespace を 1 回 128 件以内で回収し、終端の後は
//! 列挙しない。

use super::*;

use futures_util::StreamExt;
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{DocFetchPolicy, DocOp, DocQuery, DocsSync};
use kukuri_store::EMPTY_NAMESPACES_KIND;

use crate::accounts::ensure_accounts_initialized;
use crate::runtime::LegacyStoreProgress;

async fn open_runtime(db: &Path) -> DesktopRuntime {
    DesktopRuntime::new_with_config_and_identity(
        db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime")
}

async fn namespace_count(runtime: &DesktopRuntime) -> usize {
    let node = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("stack")
        .node
        .clone();
    node.docs().list().await.expect("list").count().await
}

/// 更新前の版の読取りが作った形の、空で閉じた namespace を作る。
async fn empty_namespace(runtime: &DesktopRuntime, name: &str) -> ReplicaId {
    let docs = runtime.iroh_stack.docs_sync.clone();
    let replica = ReplicaId::new(format!("author::{name}"));
    docs.open_replica(&replica).await.expect("open");
    docs.close_replica(&replica).await.expect("close");
    replica
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_namespaces_are_reclaimed_in_bounded_steps_and_once() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let source = tempdir().expect("source dir");
    let db =
        ensure_accounts_initialized(source.path(), IdentityStorageMode::FileOnly).expect("account");
    let runtime = open_runtime(&db).await;
    let docs = runtime.iroh_stack.docs_sync.clone();
    let written = ReplicaId::new("author::written-before-the-update");
    docs.apply_doc_op(
        &written,
        DocOp::SetBytes {
            key: "profile/latest".into(),
            value: b"kept".to_vec(),
        },
    )
    .await
    .expect("write");
    docs.close_replica(&written).await.expect("close");
    for index in 0..130 {
        empty_namespace(&runtime, &format!("empty-{index}")).await;
    }
    let before = namespace_count(&runtime).await;

    // 1 ステップで調べるのは 128 件以内で、位置を保存する。
    assert!(matches!(
        runtime.legacy_store_step().await.expect("first step"),
        LegacyStoreProgress::More
    ));
    let (cursor, done) = runtime
        .sqlite
        .legacy_store_position(EMPTY_NAMESPACES_KIND)
        .await
        .expect("position");
    assert!(!done && !cursor.is_empty(), "the first page is saved");
    assert!(before - namespace_count(&runtime).await <= 128);
    runtime.shutdown().await;
    drop(runtime);

    // 再起動しても保存した位置から続け、終端へ達する。
    let runtime = open_runtime(&db).await;
    let mut steps = 0;
    while !matches!(
        runtime.legacy_store_step().await.expect("step"),
        LegacyStoreProgress::Retired
    ) {
        steps += 1;
        assert!(steps < 10, "reclaim does not converge");
    }
    let docs = runtime.iroh_stack.docs_sync.clone();
    assert!(
        before - namespace_count(&runtime).await >= 130,
        "every empty namespace is reclaimed"
    );
    let records = docs
        .query_replica_with_policy(
            &written,
            DocQuery::Exact("profile/latest".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read");
    assert_eq!(records.len(), 1, "a namespace with entries is kept");

    // 終端の後は列挙しないので、後から置いた空の namespace は残る。位置を先頭へ戻しても、終端の記録があれば読まない
    // (位置より後ろだけを調べる列挙では、後から置いた namespace は位置より前に並びうるため)。
    runtime
        .sqlite
        .finish_legacy_store_page(EMPTY_NAMESPACES_KIND, "", true)
        .await
        .expect("keep the end");
    empty_namespace(&runtime, "after-the-reclaim").await;
    let after = namespace_count(&runtime).await;
    assert!(matches!(
        runtime
            .legacy_store_step()
            .await
            .expect("step after the end"),
        LegacyStoreProgress::Retired
    ));
    assert_eq!(namespace_count(&runtime).await, after);
    runtime.shutdown().await;
}
