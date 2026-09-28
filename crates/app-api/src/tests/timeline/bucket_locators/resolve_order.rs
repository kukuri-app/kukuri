//! #1403: CN 検索結果の解決で、手元の確認・group をまたぐ remote 読取り・候補 peer ごとの期限を確かめる。

use super::*;

/// 同じ group の remote 読取りが期限まで終わらなくても、手元の投稿は取り下げを確かめてから返す(#1403 判定 4)。
#[tokio::test(start_paused = true)]
async fn a_cached_post_is_rechecked_before_slow_remote_reads_in_its_group() {
    let (app, docs) = observed_app();
    let topic = TopicId::new("timeout-topic");
    let keys = generate_keys();
    let post = build_post_envelope(&keys, &topic, "old content", None).expect("post");
    let (replica, cached_input) = seed_bucket_post(&docs, &post).await;
    assert!(
        app.resolve_community_index_posts(vec![cached_input.clone()])
            .await
            .expect("initial cache")
            .entries[0]
            .post
            .is_some()
    );
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("withdrawal");
    docs.inner
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: format!("withdrawals/{}/state", post.id.as_str()),
                value: serde_json::to_value(withdrawal).expect("json"),
            },
        )
        .await
        .expect("arrived withdrawal");
    *docs.remote.lock().expect("remote source poisoned") = vec![Arc::new(PendingRemoteDocs)];
    let mut inputs = (0..8)
        .map(|index| CommunityIndexPostResolveInput {
            key: format!("slow-{index}"),
            topic: topic.as_str().into(),
            object_id: format!("missing-{index}"),
            author_pubkey: "author".into(),
            channel_ref: ChannelRef::Public,
            source_replica_id: Some(replica.as_str().into()),
        })
        .collect::<Vec<_>>();
    inputs.push(cached_input);
    let response = tokio::time::timeout(
        Duration::from_secs(31),
        app.resolve_community_index_posts(inputs),
    )
    .await
    .expect("batch should return by its deadline")
    .expect("resolve");
    let tombstone = response.entries[8]
        .post
        .as_ref()
        .expect("the cached post is resolved before the remote reads");
    assert!(
        tombstone.withdrawal.is_some() && tombstone.content.is_empty(),
        "the locally arrived withdrawal is applied before the post is returned"
    );
    assert!(
        response.entries[..8]
            .iter()
            .all(|entry| entry.post.is_none())
    );
}

fn topic_bucket(topic: &str) -> ReplicaId {
    BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.into(),
        },
        TimeBucket::from_unix_seconds(Utc::now().timestamp()).expect("time"),
    )
    .expect("bucket")
    .replica_id()
}

/// `topic` の bucket にあると CN が返した、誰も持たない投稿の入力。
fn missing_inputs(topic: &str, count: usize) -> Vec<CommunityIndexPostResolveInput> {
    let replica = topic_bucket(topic);
    (0..count)
        .map(|index| CommunityIndexPostResolveInput {
            key: format!("{topic}-missing-{index}"),
            topic: topic.into(),
            object_id: format!("{topic}-missing-{index}"),
            author_pubkey: "author".into(),
            channel_ref: ChannelRef::Public,
            source_replica_id: Some(replica.as_str().into()),
        })
        .collect()
}

/// provider だけが持つ投稿。
async fn provider_post(
    topic: &str,
    text: &str,
) -> (Arc<dyn DocsSync>, CommunityIndexPostResolveInput) {
    let provider = MemoryDocsSync::default();
    let envelope = build_post_envelope(&generate_keys(), &TopicId::new(topic), text, None)
        .expect("signed post");
    let (_, input) = seed_bucket_post_in(&provider, &envelope).await;
    (Arc::new(provider), input)
}

/// 別の topic の group の remote 読取りが期限まで終わらなくても、後ろの group の手元の投稿を解決する(#1403 判定 1)。
#[tokio::test(start_paused = true)]
async fn a_cached_post_resolves_while_an_earlier_group_waits_for_remote_reads() {
    let (app, docs) = observed_app();
    let post = build_post_envelope(
        &generate_keys(),
        &TopicId::new("b-local"),
        "local body",
        None,
    )
    .expect("post");
    let (_, cached_input) = seed_bucket_post(&docs, &post).await;
    *docs.remote.lock().expect("remote source poisoned") = vec![Arc::new(PendingRemoteDocs)];
    let mut inputs = missing_inputs("a-slow", 8);
    inputs.push(cached_input);
    let response = tokio::time::timeout(
        Duration::from_secs(31),
        app.resolve_community_index_posts(inputs),
    )
    .await
    .expect("batch should return by its deadline")
    .expect("resolve");
    assert_eq!(
        response.entries[8]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("local body")
    );
    assert_eq!(docs.forbidden.load(Ordering::SeqCst), 0);
}

/// 前の group の remote 読取りが終わるのを待たずに、後ろの group の投稿を provider から読む(#1403 判定 2)。
#[tokio::test(start_paused = true)]
async fn a_later_group_reads_its_provider_while_an_earlier_group_waits() {
    let (app, docs) = observed_app();
    let (provider, remote_input) = provider_post("b-remote", "remote body").await;
    let slow_inputs = missing_inputs("a-slow", 1);
    {
        let mut readers = docs
            .remote_by_replica
            .lock()
            .expect("remote source poisoned");
        // 1 peer の試行の期限を 5 回重ねても、batch の期限を越える。
        readers.insert(
            topic_bucket("a-slow").as_str().into(),
            (0..5)
                .map(|_| Arc::new(PendingRemoteDocs) as Arc<dyn DocsSync>)
                .collect(),
        );
        readers.insert(
            remote_input.source_replica_id.clone().expect("source"),
            vec![provider],
        );
    }
    let mut inputs = slow_inputs;
    inputs.push(remote_input);
    let response = tokio::time::timeout(
        Duration::from_secs(31),
        app.resolve_community_index_posts(inputs),
    )
    .await
    .expect("batch should return by its deadline")
    .expect("resolve");
    assert!(response.entries[0].post.is_none());
    assert_eq!(
        response.entries[1]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("remote body")
    );
}

/// 応答しない候補の試行を打ち切り、次の候補から読む(#1403 判定 3)。
#[tokio::test(start_paused = true)]
async fn an_unresponsive_provider_is_skipped_for_the_next_candidate() {
    let (app, docs) = observed_app();
    let (provider, input) = provider_post("skip-topic", "second provider body").await;
    *docs.remote.lock().expect("remote source poisoned") =
        vec![Arc::new(PendingRemoteDocs), provider];
    let response = tokio::time::timeout(
        Duration::from_secs(31),
        app.resolve_community_index_posts(vec![input]),
    )
    .await
    .expect("batch should return by its deadline")
    .expect("resolve");
    assert_eq!(
        response.entries[0]
            .post
            .as_ref()
            .map(|post| post.content.as_str()),
        Some("second provider body")
    );
}
