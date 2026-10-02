#[test]
fn namespace_secret_hex_requires_exact_length() {
    let error = crate::access::parse_namespace_secret_hex("abcd")
        .expect_err("short namespace secret should fail");
    assert!(error.to_string().contains("32 bytes"));
}

/// 参加中の private channel の世代の秘密を返す参照(app-api は store の世代の鍵の行から引く。ADR 0061 §9)。
struct OneEpoch;

#[async_trait::async_trait]
impl crate::PrivateEpochSecrets for OneEpoch {
    async fn epoch_secret_hex(&self, channel_id: &str, epoch_id: &str) -> Option<String> {
        (channel_id == "room" && ["legacy", "epoch-1"].contains(&epoch_id))
            .then(|| hex::encode([5; 32]))
    }
}

/// 登録簿に無い世代の replica(旧形式・epoch・private の bucket)は、入れた参照で秘密を引いて開く。
/// 参照に無い世代・他の channel は開けない。
#[tokio::test]
async fn unregistered_epoch_replicas_resolve_through_the_installed_epoch_secrets() {
    use crate::{
        BucketReplica, BucketScope, DocQuery, DocsSync, MemoryDocsSync, TimeBucket,
        private_channel_epoch_replica_id, private_channel_replica_id,
    };
    use kukuri_core::ReplicaId;

    let docs = MemoryDocsSync::default();
    let bucket = |epoch: &str| -> ReplicaId {
        BucketReplica::new(
            BucketScope::PrivateChannel {
                channel_id: "room".into(),
                epoch_id: epoch.into(),
            },
            TimeBucket::from_unix_seconds(86_400).expect("bucket"),
        )
        .expect("replica")
        .replica_id()
    };
    let readable = [
        private_channel_replica_id("room"),
        private_channel_epoch_replica_id("room", "epoch-1"),
        bucket("epoch-1"),
    ];
    let unreadable = [
        private_channel_epoch_replica_id("room", "epoch-2"),
        private_channel_epoch_replica_id("other", "epoch-1"),
        bucket("epoch-2"),
    ];
    let reads = |replica: ReplicaId| {
        let docs = docs.clone();
        async move {
            docs.query_replica(&replica, DocQuery::Exact("k".into()))
                .await
                .is_ok()
        }
    };
    for replica in readable.iter().chain(&unreadable) {
        assert!(
            !reads(replica.clone()).await,
            "{replica:?} without a source"
        );
    }
    docs.install_private_epoch_secrets(std::sync::Arc::new(OneEpoch))
        .await
        .expect("install");
    for replica in &readable {
        assert!(reads(replica.clone()).await, "{replica:?}");
    }
    for replica in &unreadable {
        assert!(!reads(replica.clone()).await, "{replica:?}");
    }
}
