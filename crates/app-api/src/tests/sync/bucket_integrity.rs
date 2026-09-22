use super::hydration_integrity::signed_post;
use super::*;
use crate::service::post_integrity::PostRejection;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

#[test]
fn bucket_post_must_match_its_signed_creation_time_and_scope() {
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:bucket-integrity");
    let channel = ChannelId::new("bucket-channel");
    for private in [false, true] {
        let envelope = signed_post(
            &keys,
            &topic,
            "bucket post",
            if private {
                ObjectVisibility::Private
            } else {
                ObjectVisibility::Public
            },
            private.then_some(&channel),
        );
        let time = TimeBucket::from_unix_seconds(envelope.created_at).unwrap();
        let scope = if private {
            BucketScope::PrivateChannel {
                channel_id: channel.as_str().into(),
                epoch_id: "e1".into(),
            }
        } else {
            BucketScope::Topic {
                topic_id: topic.as_str().into(),
            }
        };
        let correct = BucketReplica::new(scope.clone(), time)
            .unwrap()
            .replica_id();
        assert_eq!(
            VerifiedPost::verify_local(envelope.clone(), &correct).map(|_| ()),
            Ok(())
        );
        let wrong_time =
            BucketReplica::new(scope, TimeBucket::from_index(time.index() + 1).unwrap())
                .unwrap()
                .replica_id();
        assert_eq!(
            VerifiedPost::verify_local(envelope.clone(), &wrong_time).map(|_| ()),
            Err(PostRejection::ScopeMismatch),
            "a valid signature does not authorize writing a post into another time bucket"
        );
        let wrong_scope = BucketReplica::new(
            BucketScope::Topic {
                topic_id: "other".into(),
            },
            time,
        )
        .unwrap()
        .replica_id();
        assert!(VerifiedPost::verify_local(envelope, &wrong_scope).is_err());
    }
}
