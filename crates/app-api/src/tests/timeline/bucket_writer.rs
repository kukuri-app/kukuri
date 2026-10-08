//! #1221 R5-H AC-1〜3: 切替後の writer は ADR 0054 §1・§2 の bucket へ書き、旧 replica へは書かない。
//! bucket は署名した時刻から決め、outbox の再開は積んだ時の宛先へ書く。

use super::super::*;
use kukuri_docs_sync::{BucketReplica, BucketScope, DocKeyOrder, DocKeyQuery, TimeBucket};
use kukuri_store::PostWithdrawalStore;

const TOPIC: &str = "kukuri:topic:bucket-writer";

fn topic_bucket(created_at: i64) -> ReplicaId {
    BucketReplica::new(
        BucketScope::Topic {
            topic_id: TOPIC.into(),
        },
        TimeBucket::from_unix_seconds(created_at).expect("bucket"),
    )
    .expect("replica")
    .replica_id()
}

fn author_bucket(author: &str, created_at: i64) -> ReplicaId {
    BucketReplica::new(
        BucketScope::Author {
            author_pubkey: author.into(),
        },
        TimeBucket::from_unix_seconds(created_at).expect("bucket"),
    )
    .expect("replica")
    .replica_id()
}

async fn has_key(docs: &MemoryDocsSync, replica: &ReplicaId, key: &str) -> bool {
    !docs
        .query_replica_with_policy(
            replica,
            DocQuery::Exact(key.to_string()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .unwrap_or_default()
        .is_empty()
}

async fn has_prefix(docs: &MemoryDocsSync, replica: &ReplicaId, prefix: &str) -> bool {
    docs.query_replica_keys(
        replica,
        DocKeyQuery {
            prefix: prefix.to_string(),
            order: DocKeyOrder::Ascending,
            limit: 4,
        },
    )
    .await
    .is_ok_and(|page| !page.entries.is_empty())
}

#[tokio::test]
async fn the_switched_writer_puts_each_record_in_its_bucket_and_not_in_the_legacy_replica() {
    let (app, store, docs, _) = local_app_with_memory_services();
    app.switch_writer(1);
    let me = app.current_author_pubkey();
    let legacy_topic = topic_replica_id(TOPIC);
    let legacy_author = author_replica_id(&me);

    let object_id = app.create_post(TOPIC, "hello", None).await.expect("post");
    let created_at = store
        .get_envelope(&EnvelopeId::from(object_id.as_str()))
        .await
        .expect("envelope")
        .expect("stored")
        .created_at;
    let bucket = topic_bucket(created_at);
    for key in [
        format!("objects/{object_id}/state"),
        format!("objects/{object_id}/envelope"),
    ] {
        assert!(has_key(&docs, &bucket, &key).await, "{key} in the bucket");
        assert!(
            !has_key(&docs, &legacy_topic, &key).await,
            "{key} not legacy"
        );
    }
    assert!(has_prefix(&docs, &bucket, "indexes/timeline/").await);
    assert!(has_prefix(&docs, &bucket, "indexes/thread/").await);
    let profile_key = format!("profile/posts/{object_id}");
    assert!(has_key(&docs, &author_bucket(&me, created_at), &profile_key).await);
    assert!(!has_key(&docs, &legacy_author, &profile_key).await);

    app.toggle_reaction(
        TOPIC,
        &object_id,
        ReactionKeyV1::Emoji {
            emoji: "👍".into()
        },
        Some(ChannelRef::Public),
    )
    .await
    .expect("reaction");
    let reactions = format!("reactions/{object_id}/");
    let now = Utc::now().timestamp();
    assert!(has_prefix(&docs, &topic_bucket(now), &reactions).await);
    assert!(!has_prefix(&docs, &legacy_topic, &reactions).await);

    app.withdraw_post(
        TOPIC,
        &object_id,
        ChannelRef::Public,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .await
    .expect("withdraw");
    let withdrawal = format!("withdrawals/{object_id}/state");
    assert!(has_key(&docs, &bucket, &withdrawal).await);
    assert!(!has_key(&docs, &legacy_topic, &withdrawal).await);
    assert!(
        store
            .pending_withdrawal_writes(8)
            .await
            .expect("outbox")
            .is_empty()
    );

    let profile = app
        .set_my_profile(ProfileInput {
            name: Some("bucket".into()),
            display_name: None,
            about: None,
            picture_upload: None,
            clear_picture: false,
            nip05: None,
        })
        .await
        .expect("profile");
    // 現在値は制御領域の key、更新の event は author bucket。
    assert!(has_key(&docs, &legacy_author, "profile/latest").await);
    assert!(has_prefix(&docs, &author_bucket(&me, now), "envelopes/").await);
    assert!(profile.updated_at > 0);

    let session_id = app
        .create_live_session(
            TOPIC,
            CreateLiveSessionInput {
                title: "live".into(),
                description: String::new(),
            },
        )
        .await
        .expect("live");
    let session = format!("sessions/live/{session_id}/state");
    assert!(has_key(&docs, &topic_bucket(now), &session).await);
    assert!(!has_key(&docs, &legacy_topic, &session).await);
}

#[tokio::test]
async fn private_posts_go_to_the_current_epoch_bucket_with_the_derived_capability() {
    let (app, store, docs, _) = local_app_with_memory_services();
    app.switch_writer(1);
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "bucket".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("channel");
    let object_id = app
        .create_post_in_channel(
            TOPIC,
            ChannelRef::PrivateChannel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
            "secret",
            None,
        )
        .await
        .expect("private post");
    let created_at = store
        .get_envelope(&EnvelopeId::from(object_id.as_str()))
        .await
        .expect("envelope")
        .expect("stored")
        .created_at;
    let bucket = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: channel.channel_id.clone(),
            epoch_id: channel.current_epoch_id.clone(),
        },
        TimeBucket::from_unix_seconds(created_at).expect("bucket"),
    )
    .expect("replica")
    .replica_id();
    let key = format!("objects/{object_id}/envelope");
    assert!(has_key(&docs, &bucket, &key).await);
    let legacy = private_channel_replica_for_epoch(&channel.channel_id, &channel.current_epoch_id);
    assert!(!has_key(&docs, &legacy, &key).await);
    // epoch の鍵の行を消すと、導出した bucket も読めない(ADR 0061 §9)。
    app.services
        .projection_store
        .delete_private_channel_epochs(&channel.channel_id, 8)
        .await
        .expect("remove");
    assert!(
        docs.query_replica_with_policy(&bucket, DocQuery::Exact(key), DocFetchPolicy::LocalOnly)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn the_bucket_follows_the_signed_time_and_queued_writes_keep_their_destination() {
    let (app, store, docs, _) = local_app_with_memory_services();
    // 切替前は旧 replica。bucket は操作の時刻ではなく、渡した署名の時刻から決まる。
    let boundary = 86_400 * 20_000;
    assert_eq!(
        app.services
            .scope_write_replica(TOPIC, None, boundary - 1)
            .expect("legacy"),
        topic_replica_id(TOPIC)
    );
    app.switch_writer(1);
    app.switch_writer(2);
    assert_eq!(app.writer_switched_at(), Some(1));
    assert_eq!(
        app.services
            .scope_write_replica(TOPIC, None, boundary - 1)
            .expect("before"),
        topic_bucket(boundary - 1)
    );
    assert_eq!(
        app.services
            .scope_write_replica(TOPIC, None, boundary)
            .expect("after"),
        topic_bucket(boundary)
    );

    // 切替前に積んだ旧 replica の行と、昨日の bucket の行は、記録した宛先へ書く。capability の無い private の行は
    // 残り、書けた行だけが消える。同じ再開を繰り返しても二重に書かない。
    let object_id = app.create_post(TOPIC, "target", None).await.expect("post");
    let envelope = store
        .get_envelope(&EnvelopeId::from(object_id.as_str()))
        .await
        .expect("envelope")
        .expect("stored");
    let private = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: "missing".into(),
            epoch_id: "e".into(),
        },
        TimeBucket::from_unix_seconds(boundary).expect("bucket"),
    )
    .expect("replica")
    .replica_id();
    let row = |replica: ReplicaId| WithdrawalWriteRow {
        withdrawal_envelope_id: EnvelopeId::from("withdrawal-1"),
        replica_id: replica,
        target_object_id: EnvelopeId::from(object_id.as_str()),
        envelope: envelope.clone(),
        target_replica_id: None,
    };
    store
        .queue_withdrawal_writes(vec![
            row(topic_replica_id(TOPIC)),
            row(topic_bucket(boundary - 1)),
            row(private.clone()),
        ])
        .await
        .expect("queue");
    assert!(app.resume_withdrawal_writes().await.is_err());
    let key = format!("withdrawals/{object_id}/state");
    assert!(has_key(&docs, &topic_replica_id(TOPIC), &key).await);
    assert!(has_key(&docs, &topic_bucket(boundary - 1), &key).await);
    let pending = store.pending_withdrawal_writes(8).await.expect("pending");
    assert_eq!(
        pending
            .iter()
            .map(|row| row.replica_id.clone())
            .collect::<Vec<_>>(),
        vec![private]
    );
}
