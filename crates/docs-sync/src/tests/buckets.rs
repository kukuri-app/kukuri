use anyhow::Result;
use kukuri_core::ReplicaId;

use crate::{
    BucketReplica, BucketScope, DocFetchPolicy, DocQuery, DocsSync, MemoryDocsSync, TimeBucket,
};

#[test]
fn bucket_boundaries_and_live_window_do_not_depend_on_elapsed_history() -> Result<()> {
    assert!(TimeBucket::from_unix_seconds(-1).is_err());
    assert!(TimeBucket::from_index(u64::MAX).is_err());
    let zero = TimeBucket::from_unix_seconds(0)?;
    assert_eq!(zero.live_window().count(), 1);
    assert!(zero.contains(86_399));
    assert!(!zero.contains(86_400));
    assert!(!zero.contains(-1));
    for index in [1, 10, 100, 1_000, 10_000, i64::MAX as u64 / 86_400] {
        let bucket = TimeBucket::from_index(index)?;
        let window: Vec<_> = bucket.live_window().map(TimeBucket::index).collect();
        assert_eq!(window, vec![index, index - 1]);
        assert_eq!(
            TimeBucket::from_unix_seconds(bucket.start_seconds() as i64)?,
            bucket
        );
    }
    assert!(TimeBucket::from_unix_seconds(i64::MAX)?.contains(i64::MAX));
    Ok(())
}

#[test]
fn bucket_ids_roundtrip_without_scope_delimiter_collisions() -> Result<()> {
    let bucket = TimeBucket::from_index(20718)?;
    for scope in [
        BucketScope::Topic {
            topic_id: "demo".into(),
        },
        BucketScope::Topic {
            topic_id: "日本語::epoch::x::0".into(),
        },
        BucketScope::PrivateChannel {
            channel_id: "a::b".into(),
            epoch_id: "c".into(),
        },
        BucketScope::PrivateChannel {
            channel_id: "a".into(),
            epoch_id: "b::c".into(),
        },
        BucketScope::Author {
            author_pubkey: "abc".into(),
        },
    ] {
        let replica = BucketReplica::new(scope, bucket)?;
        assert_eq!(BucketReplica::parse(&replica.replica_id())?, replica);
    }
    let topic = BucketReplica::new(
        BucketScope::Topic {
            topic_id: "demo".into(),
        },
        bucket,
    )?;
    assert_eq!(
        topic.replica_id().as_str(),
        "bucket::v1::topic::64656d6f::20718"
    );
    assert!(
        BucketReplica::new(
            BucketScope::Topic {
                topic_id: String::new()
            },
            bucket
        )
        .is_err()
    );
    assert!(
        BucketReplica::new(
            BucketScope::Topic {
                topic_id: "x".repeat(1025)
            },
            bucket
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn bucket_parser_rejects_aliases_unknown_versions_and_unbounded_input() {
    for raw in [
        "bucket::v0::topic::61::1",
        "bucket::v2::channel::61::62::1",
        "bucket::v1::device::61::1",
        "bucket::v1::topic::61::+1",
        "bucket::v1::topic::61::01",
        "bucket::v1::topic::61::-1",
        "bucket::v1::topic::61::18446744073709551615",
        "bucket::v1::topic::61::1::extra",
        "bucket::v1::topic::6A::1",
        "bucket::v1::topic::a::1",
        "bucket::v1::topic::ff::1",
        "bucket::v1::topic::::1",
        "bucket::v1::channel::61::::1",
        "topic::demo",
    ] {
        assert!(BucketReplica::parse(&ReplicaId::new(raw)).is_err(), "{raw}");
    }
    assert!(
        BucketReplica::parse(&ReplicaId::new(format!(
            "bucket::v1::topic::{}::1",
            "61".repeat(1025)
        )))
        .is_err()
    );
}

#[test]
fn private_bucket_keys_are_separated_by_scope_epoch_time_and_secret() -> Result<()> {
    let private = |channel: &str, epoch: &str, bucket| {
        BucketReplica::new(
            BucketScope::PrivateChannel {
                channel_id: channel.into(),
                epoch_id: epoch.into(),
            },
            TimeBucket::from_index(bucket)?,
        )
    };
    let replica = private("a", "e1", 1)?;
    let secret = replica.derive_private_secret(&[7; 32])?;
    // 独立したBLAKE3入力から固定したwire golden。contextやIDの変更でnamespaceを分断しない。
    assert_eq!(
        hex::encode(secret),
        "a584509a92dac793b8a28b259ef74d85685d8f50339c672ecba5d4c4612cecd5"
    );
    assert_eq!(secret, replica.derive_private_secret(&[7; 32])?);
    assert_ne!(secret, replica.derive_private_secret(&[8; 32])?);
    for other in [
        private("b", "e1", 1)?,
        private("a", "e2", 1)?,
        private("a", "e1", 2)?,
    ] {
        assert_ne!(secret, other.derive_private_secret(&[7; 32])?);
    }
    assert!(
        BucketReplica::new(
            BucketScope::Topic {
                topic_id: "a".into()
            },
            TimeBucket::from_index(1)?
        )?
        .derive_private_secret(&[7; 32])
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn bucket_access_requires_exact_private_capability_and_rejects_unknown_version() -> Result<()>
{
    let docs = MemoryDocsSync::default();
    let replica = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: "a".into(),
            epoch_id: "e1".into(),
        },
        TimeBucket::from_index(1)?,
    )?;
    let id = replica.replica_id();
    assert!(docs.open_replica(&id).await.is_err());
    let secret = hex::encode(replica.derive_private_secret(&[9; 32])?);
    docs.register_private_replica_secret(&id, &secret).await?;
    docs.open_replica(&id).await?;
    docs.remove_private_replica_secret(&id).await?;
    assert!(
        docs.query_replica_with_policy(&id, DocQuery::All, DocFetchPolicy::LocalOnly)
            .await
            .is_err()
    );
    let unknown = ReplicaId::new("bucket::v2::channel::61::6531::1");
    docs.register_private_replica_secret(&unknown, &secret)
        .await?;
    assert!(docs.open_replica(&unknown).await.is_err());
    Ok(())
}
