//! 投稿の反映の検証(#1248)の契約を固定する test。修正前の再現は `hydration_integrity.rs`。
//!
//! Issue #1248 の TR-3・7・9・10・12・15・16、AC-4・AC-7・AC-9、INV-2 に対応する。

use super::hydration_integrity::{
    integrity_fixture, public_timeline_after_settle, signed_post, wait_for_row,
    write_object_entries,
};
use super::*;

/// 同じ key に複数の record を返す docs(iroh-docs は、同じ key の entry を docs author ごとに持つ)。
/// `shadows` に入れた値を、その key の正しい record より先に返す。
#[derive(Clone, Default)]
struct ShadowingDocsSync {
    inner: MemoryDocsSync,
    shadows: Arc<TokioMutex<HashMap<String, Vec<Vec<u8>>>>>,
    /// この key の読み出しを失敗させる(I/O の失敗)。
    failing_key: Arc<TokioMutex<Option<String>>>,
}

impl ShadowingDocsSync {
    async fn shadow(&self, key: &str, value: serde_json::Value) {
        self.shadows
            .lock()
            .await
            .entry(key.to_string())
            .or_default()
            .push(serde_json::to_vec(&value).expect("shadow json"));
    }
}

#[async_trait]
impl DocsSync for ShadowingDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        if let DocQuery::Exact(key) = &query
            && self.failing_key.lock().await.as_deref() == Some(key.as_str())
        {
            anyhow::bail!("simulated docs read failure");
        }
        let records = self
            .inner
            .query_replica_with_policy(replica_id, query, policy)
            .await?;
        let shadows = self.shadows.lock().await;
        let mut merged = Vec::new();
        for record in records {
            for value in shadows.get(record.key.as_str()).into_iter().flatten() {
                merged.push(kukuri_docs_sync::DocRecord {
                    key: record.key.clone(),
                    content_hash: kukuri_docs_sync::value_hash(value),
                    content_len: value.len() as u64,
                    value: value.clone(),
                });
            }
            merged.push(record);
        }
        Ok(merged)
    }

    // #1239: タイムラインの取得は、ページの範囲を時系列の索引と照合する(key だけの上限つきの読み出し)。
    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

fn app_over_docs(docs_sync: Arc<dyn DocsSync>) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    (app, store)
}

fn honest_header(envelope: &KukuriEnvelope) -> CanonicalPostHeader {
    envelope
        .to_post_object()
        .expect("post object")
        .expect("post object")
}

// TR-7 / AC-7: 同じ key に、読めない record と別の投稿の envelope が先に並んでいても、検証に通る record から反映する。
#[tokio::test]
async fn invalid_records_placed_before_the_signed_envelope_do_not_hide_the_post() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:integrity-shadowed");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let envelope = signed_post(
        &author_keys,
        &topic,
        "the real words",
        ObjectVisibility::Public,
        None,
    );
    let other = signed_post(
        &generate_keys(),
        &topic,
        "an envelope of another post under this key",
        ObjectVisibility::Public,
        None,
    );
    let envelope_key = stable_key("objects", &format!("{}/envelope", envelope.id.as_str()));
    docs_sync
        .shadow(
            envelope_key.as_str(),
            serde_json::json!({ "not": "an envelope" }),
        )
        .await;
    docs_sync
        .shadow(
            envelope_key.as_str(),
            serde_json::to_value(&other).expect("other envelope json"),
        )
        .await;
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&envelope),
        &honest_header(&envelope),
    )
    .await;

    // key 指定の反映。
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &envelope.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate"),
        "the signed envelope behind invalid records must be projected"
    );
    let row = ObjectProjectionStore::get_object_projection(store.as_ref(), &envelope.id)
        .await
        .expect("projection")
        .expect("row");
    assert_eq!(row.author_pubkey, author_keys.public_key_hex());
    assert_eq!(row.content.as_deref(), Some("the real words"));

    // 全件走査でも同じ結果になる。#1239 の T5a の後、タイムラインの取得は全件走査をしない(ページの範囲を
    // 時系列の索引と照合する)ので、購読タスクが使う全件走査を直接呼ぶ。
    ObjectProjectionStore::rebuild_object_projections(store.as_ref(), Vec::new())
        .await
        .expect("clear projection");
    hydrate_subscription_state(
        &app.services,
        topic.as_str(),
        &replica,
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("full scan");
    let view = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == envelope.id.as_str())
        .expect("the post is listed after a full scan");
    assert_eq!(item.author_pubkey, author_keys.public_key_hex());
    assert_eq!(item.content, "the real words");
}

// AC-7: 1 つの key で調べる record 数は定数。上限を超える不正な record を積まれた投稿は反映しない(best effort)。
#[tokio::test]
async fn records_beyond_the_per_key_limit_are_not_examined() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let (app, _store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:integrity-per-key-limit");
    let replica = topic_replica_id(topic.as_str());
    let envelope = signed_post(
        &generate_keys(),
        &topic,
        "buried",
        ObjectVisibility::Public,
        None,
    );
    let envelope_key = stable_key("objects", &format!("{}/envelope", envelope.id.as_str()));
    for index in 0..MAX_ENVELOPE_RECORDS_PER_OBJECT {
        docs_sync
            .shadow(
                envelope_key.as_str(),
                serde_json::json!({ "garbage": index }),
            )
            .await;
    }
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&envelope),
        &honest_header(&envelope),
    )
    .await;

    assert!(
        !hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &envelope.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate")
    );
}

// TR-3 / AC-2: header だけが先に届いた投稿は反映せず、envelope が届いた時点で反映する。
#[tokio::test]
async fn post_is_projected_when_the_envelope_arrives_after_the_header() {
    let fixture = integrity_fixture("envelope-after-header").await;
    let author_keys = generate_keys();
    let envelope = signed_post(
        &author_keys,
        &fixture.topic,
        "header first, envelope later",
        ObjectVisibility::Public,
        None,
    );
    let header = honest_header(&envelope);
    write_object_entries(fixture.docs_sync.as_ref(), &fixture.replica, None, &header).await;
    sleep(Duration::from_millis(300)).await;
    assert!(
        ObjectProjectionStore::get_object_projection(fixture.viewer_store.as_ref(), &envelope.id)
            .await
            .expect("projection")
            .is_none(),
        "a header alone must not be projected"
    );

    fixture
        .docs_sync
        .apply_doc_op(
            &fixture.replica,
            DocOp::SetJson {
                key: stable_key("objects", &format!("{}/envelope", envelope.id.as_str())),
                value: serde_json::to_value(&envelope).expect("envelope json"),
            },
        )
        .await
        .expect("write the envelope later");
    assert!(
        wait_for_row(&fixture.viewer_store, &envelope.id).await,
        "the envelope event must project the post"
    );
    let row =
        ObjectProjectionStore::get_object_projection(fixture.viewer_store.as_ref(), &envelope.id)
            .await
            .expect("projection")
            .expect("row");
    assert_eq!(row.author_pubkey, author_keys.public_key_hex());
}

// INV-2: gossip hint の個別反映も同じ検証を通る。
#[tokio::test]
async fn hint_does_not_project_a_header_with_a_forged_author() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:integrity-hint");
    let replica = topic_replica_id(topic.as_str());
    let victim_pubkey = generate_keys().public_key_hex();
    // header だけの投稿(envelope なし)と、envelope の署名者と違う author の header。
    let header_only = signed_post(
        &generate_keys(),
        &topic,
        "header only",
        ObjectVisibility::Public,
        None,
    );
    let mut forged_header_only = honest_header(&header_only);
    forged_header_only.author = Pubkey::from(victim_pubkey.as_str());
    write_object_entries(docs_sync.as_ref(), &replica, None, &forged_header_only).await;
    let attacker_keys = generate_keys();
    let signed_by_attacker = signed_post(
        &attacker_keys,
        &topic,
        "signed by the attacker",
        ObjectVisibility::Public,
        None,
    );
    let mut forged = honest_header(&signed_by_attacker);
    forged.author = Pubkey::from(victim_pubkey.as_str());
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&signed_by_attacker),
        &forged,
    )
    .await;

    let hydrated = hydrate_subscription_hint(
        &app.services,
        topic.as_str(),
        &replica,
        &GossipHint::TopicObjectsChanged {
            topic_id: topic.clone(),
            objects: vec![
                HintObjectRef {
                    object_id: header_only.id.as_str().to_string(),
                    object_kind: "post".into(),
                },
                HintObjectRef {
                    object_id: signed_by_attacker.id.as_str().to_string(),
                    object_kind: "post".into(),
                },
            ],
        },
    )
    .await
    .expect("hint");
    assert_eq!(
        hydrated, 1,
        "only the post backed by a signed envelope is projected"
    );
    assert!(
        ObjectProjectionStore::get_object_projection(store.as_ref(), &header_only.id)
            .await
            .expect("projection")
            .is_none()
    );
    let row = ObjectProjectionStore::get_object_projection(store.as_ref(), &signed_by_attacker.id)
        .await
        .expect("projection")
        .expect("row");
    assert_eq!(
        row.author_pubkey,
        attacker_keys.public_key_hex(),
        "the author is the signer of the envelope, not the header's claim"
    );
}

// TR-9: 攻撃者が自分の投稿を正しく取り下げても、header が申告する他人の著者の「取り下げ済みの投稿」はできない。
#[tokio::test]
async fn withdrawal_of_a_post_with_a_forged_header_does_not_create_a_row_for_the_claimed_author() {
    let fixture = integrity_fixture("withdrawn-forged").await;
    let victim_pubkey = generate_keys().public_key_hex();
    let attacker_keys = generate_keys();
    let envelope = signed_post(
        &attacker_keys,
        &fixture.topic,
        "withdrawn by the attacker",
        ObjectVisibility::Public,
        None,
    );
    let mut header = honest_header(&envelope);
    header.author = Pubkey::from(victim_pubkey.as_str());
    write_object_entries(
        fixture.docs_sync.as_ref(),
        &fixture.replica,
        Some(&envelope),
        &header,
    )
    .await;
    let withdrawal = build_post_withdrawal_envelope(
        &attacker_keys,
        &envelope,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("withdrawal envelope");
    fixture
        .docs_sync
        .apply_doc_op(
            &fixture.replica,
            DocOp::SetJson {
                key: stable_key("withdrawals", &format!("{}/state", envelope.id.as_str())),
                value: serde_json::to_value(&withdrawal).expect("withdrawal json"),
            },
        )
        .await
        .expect("write the withdrawal");

    let view = public_timeline_after_settle(&fixture).await;
    assert!(
        view.items
            .iter()
            .all(|item| item.author_pubkey != victim_pubkey),
        "a withdrawn post was attributed to the author that the header claims"
    );
    let item = view
        .items
        .iter()
        .find(|item| item.object_id == envelope.id.as_str())
        .expect("the withdrawn post is listed under its signer");
    assert_eq!(item.author_pubkey, attacker_keys.public_key_hex());
    assert!(item.withdrawal.is_some());
    assert!(item.content.is_empty());
}

// TR-10: view の生成で返信先を反映するときも、署名つき envelope から作る。
#[tokio::test]
async fn reply_preview_row_is_built_from_the_signed_envelope() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let (app, _store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:integrity-reply-preview");
    let replica = topic_replica_id(topic.as_str());
    let victim_pubkey = generate_keys().public_key_hex();
    let attacker_keys = generate_keys();
    let parent = signed_post(
        &attacker_keys,
        &topic,
        "the parent",
        ObjectVisibility::Public,
        None,
    );
    let mut forged = honest_header(&parent);
    forged.author = Pubkey::from(victim_pubkey.as_str());
    write_object_entries(docs_sync.as_ref(), &replica, Some(&parent), &forged).await;
    let header_only = signed_post(
        &generate_keys(),
        &topic,
        "no envelope",
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        None,
        &honest_header(&header_only),
    )
    .await;

    let row = app
        .hydrate_reply_preview_row(&parent.id, Some((&replica, topic.as_str())))
        .await
        .expect("reply preview row")
        .expect("the parent has a signed envelope");
    assert_eq!(row.author_pubkey, attacker_keys.public_key_hex());
    assert!(
        app.hydrate_reply_preview_row(&header_only.id, Some((&replica, topic.as_str())))
            .await
            .expect("reply preview row")
            .is_none(),
        "a parent without a signed envelope has no preview"
    );
    // 別の topic の文脈で読んだ replica からは反映しない。
    let other_parent = signed_post(
        &generate_keys(),
        &topic,
        "another parent",
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&other_parent),
        &honest_header(&other_parent),
    )
    .await;
    assert!(
        app.hydrate_reply_preview_row(
            &other_parent.id,
            Some((&replica, "kukuri:topic:integrity-another-topic")),
        )
        .await
        .expect("reply preview row")
        .is_none()
    );
}

// TR-12 / AC-6: repost の snapshot と bookmark は、検証に通る投稿の値からだけ作る。
#[tokio::test]
async fn repost_snapshot_and_bookmark_use_the_signed_values() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let (app, _store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:integrity-repost-source");
    let target_topic = "kukuri:topic:integrity-repost-target";
    let replica = topic_replica_id(topic.as_str());
    let victim_pubkey = generate_keys().public_key_hex();
    let attacker_keys = generate_keys();
    let source = signed_post(
        &attacker_keys,
        &topic,
        "repost me",
        ObjectVisibility::Public,
        None,
    );
    let mut forged = honest_header(&source);
    forged.author = Pubkey::from(victim_pubkey.as_str());
    forged.payload_ref = PayloadRef::InlineText {
        text: "words nobody signed".into(),
    };
    write_object_entries(docs_sync.as_ref(), &replica, Some(&source), &forged).await;
    let header_only = signed_post(
        &generate_keys(),
        &topic,
        "no envelope",
        ObjectVisibility::Public,
        None,
    );
    let mut forged_header_only = honest_header(&header_only);
    forged_header_only.author = Pubkey::from(victim_pubkey.as_str());
    write_object_entries(docs_sync.as_ref(), &replica, None, &forged_header_only).await;

    let repost_id = app
        .create_repost(target_topic, topic.as_str(), source.id.as_str(), None)
        .await
        .expect("repost of a post with a signed envelope");
    let repost_envelope = app
        .services
        .store
        .get_envelope(&EnvelopeId::from(repost_id.as_str()))
        .await
        .expect("repost envelope")
        .expect("repost envelope");
    let snapshot = repost_envelope
        .to_post_object()
        .expect("repost object")
        .expect("repost object")
        .repost_of
        .expect("repost snapshot");
    assert_eq!(
        snapshot.source_author_pubkey.as_str(),
        attacker_keys.public_key_hex(),
        "the signed snapshot must carry the signer, not the header's claim"
    );
    assert_eq!(snapshot.content, "repost me");

    assert!(
        app.create_repost(target_topic, topic.as_str(), header_only.id.as_str(), None)
            .await
            .is_err(),
        "a source without a signed envelope cannot be reposted"
    );
    assert!(
        app.bookmark_post(topic.as_str(), header_only.id.as_str())
            .await
            .is_err(),
        "a post without a signed envelope cannot be bookmarked"
    );
    let bookmark = app
        .bookmark_post(topic.as_str(), source.id.as_str())
        .await
        .expect("bookmark");
    assert_eq!(bookmark.post.author_pubkey, attacker_keys.public_key_hex());
    assert_eq!(bookmark.post.content, "repost me");
}

// TR-15 / INVAR-6: private channel の現在の epoch と過去の epoch の投稿は、docs からの反映でも両方入る。
#[tokio::test]
async fn private_posts_of_current_and_archived_epochs_are_projected_from_docs() {
    let (app, store, _docs_sync, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:integrity-epochs";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "epochs".into(),
            audience_kind: ChannelAudienceKind::FriendOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let channel_ref = ChannelRef::PrivateChannel {
        channel_id: channel_id.clone(),
    };
    let before = app
        .create_post_in_channel(topic, channel_ref.clone(), "before rotate", None)
        .await
        .expect("post before rotate");
    app.rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("rotate");
    let after = app
        .create_post_in_channel(topic, channel_ref, "after rotate", None)
        .await
        .expect("post after rotate");

    // 購読タスクの反映が落ち着いてから、projection を空にして docs からの反映(全件走査)だけで戻す。
    // 走査の指紋(#1225)が残っていると、変化の無い replica は読み飛ばされるので、対象の replica ぶんを忘れさせる。
    sleep(Duration::from_millis(300)).await;
    let mut epoch_replicas = Vec::new();
    for object_id in [&before, &after] {
        let row = ObjectProjectionStore::get_object_projection(
            store.as_ref(),
            &EnvelopeId::from(object_id.as_str()),
        )
        .await
        .expect("projection")
        .expect("row written by the local post");
        epoch_replicas.push(row.source_replica_id);
    }
    ObjectProjectionStore::rebuild_object_projections(store.as_ref(), Vec::new())
        .await
        .expect("clear projection");
    for replica in &epoch_replicas {
        app.services
            .replica_scan_cache
            .forget(replica.as_str(), "objects/");
    }
    let view = app
        .list_timeline_scoped(
            topic,
            TimelineScope::Channel {
                channel_id: channel_id.clone(),
            },
            None,
            20,
        )
        .await
        .expect("private timeline");
    let listed = view
        .items
        .iter()
        .map(|item| item.object_id.clone())
        .collect::<Vec<_>>();
    assert!(listed.contains(&before), "archived epoch post: {listed:?}");
    assert!(listed.contains(&after), "current epoch post: {listed:?}");
    let before_row = ObjectProjectionStore::get_object_projection(
        store.as_ref(),
        &EnvelopeId::from(before.as_str()),
    )
    .await
    .expect("projection")
    .expect("row");
    let after_row = ObjectProjectionStore::get_object_projection(
        store.as_ref(),
        &EnvelopeId::from(after.as_str()),
    )
    .await
    .expect("projection")
    .expect("row");
    assert_ne!(before_row.source_replica_id, after_row.source_replica_id);
    assert_eq!(before_row.channel_id, channel_id.as_str());
    assert_eq!(after_row.channel_id, channel_id.as_str());
}

// AC-4: private channel の replica に置いた投稿が、別の topic や別の channel を申告していれば反映しない。
#[tokio::test]
async fn post_in_a_private_replica_must_claim_that_channel_and_its_topic() {
    let (app, store, docs_sync, _) = local_app_with_memory_services();
    let topic = TopicId::new("kukuri:topic:integrity-private-scope");
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: topic.clone(),
            label: "scope".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let member_post = app
        .create_post_in_channel(
            topic.as_str(),
            ChannelRef::PrivateChannel {
                channel_id: channel_id.clone(),
            },
            "from a member",
            None,
        )
        .await
        .expect("member post");
    let private_replica = ObjectProjectionStore::get_object_projection(
        store.as_ref(),
        &EnvelopeId::from(member_post.as_str()),
    )
    .await
    .expect("projection")
    .expect("row")
    .source_replica_id;

    let member_keys = generate_keys();
    let other_topic = TopicId::new("kukuri:topic:integrity-private-scope-other");
    let other_channel = ChannelId::new("channel-0-deadbeef");
    let claims = [
        // 同じ channel id を別の topic で申告する。
        signed_post(
            &member_keys,
            &other_topic,
            "another topic",
            ObjectVisibility::Private,
            Some(&channel_id),
        ),
        // 別の channel を申告する。
        signed_post(
            &member_keys,
            &topic,
            "another channel",
            ObjectVisibility::Private,
            Some(&other_channel),
        ),
        // public を申告する。
        signed_post(
            &member_keys,
            &topic,
            "public claim",
            ObjectVisibility::Public,
            None,
        ),
    ];
    for envelope in &claims {
        write_object_entries(
            docs_sync.as_ref(),
            &private_replica,
            Some(envelope),
            &honest_header(envelope),
        )
        .await;
        assert!(
            !hydrate_object_in_topic(
                &app.services,
                topic.as_str(),
                &private_replica,
                &envelope.id,
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("hydrate"),
            "a post that does not claim the channel of its replica was projected"
        );
    }
    // 正しく申告する投稿は反映される。
    let honest = signed_post(
        &member_keys,
        &topic,
        "honest member",
        ObjectVisibility::Private,
        Some(&channel_id),
    );
    write_object_entries(
        docs_sync.as_ref(),
        &private_replica,
        Some(&honest),
        &honest_header(&honest),
    )
    .await;
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &private_replica,
            &honest.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate")
    );
}

// TR-16 / AC-8: docs の読み出しの失敗は、検証の失敗(飛ばす)と区別してエラーで返す。
#[tokio::test]
async fn docs_read_failure_is_an_error_and_not_a_rejected_post() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let (app, _store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:integrity-io-failure");
    let replica = topic_replica_id(topic.as_str());
    let envelope = signed_post(
        &generate_keys(),
        &topic,
        "unreadable for now",
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&envelope),
        &honest_header(&envelope),
    )
    .await;
    *docs_sync.failing_key.lock().await = Some(stable_key(
        "objects",
        &format!("{}/envelope", envelope.id.as_str()),
    ));
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &envelope.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .is_err(),
        "a docs read failure must not be treated as an invalid post"
    );
    // 失敗が解けた後の契機で、同じ投稿を反映できる。
    *docs_sync.failing_key.lock().await = None;
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &envelope.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate after recovery")
    );
}

// AC-9: 投稿 1 件の反映で読む docs の record 数は定数で、replica の総 entry 数に依存しない。
#[tokio::test]
async fn records_read_to_project_one_post_do_not_grow_with_the_replica() {
    let mut reads = Vec::new();
    for posts in [20usize, 400] {
        let docs_sync = Arc::new(CountingDocsSync::default());
        let (app, _store) = app_over_docs(docs_sync.clone());
        let topic = TopicId::new(format!("kukuri:topic:integrity-reads-{posts}").as_str());
        let replica = topic_replica_id(topic.as_str());
        let keys = generate_keys();
        let mut target = None;
        for index in 0..posts {
            let envelope = persist_test_post(
                docs_sync.as_ref(),
                None,
                &keys,
                &topic,
                PayloadRef::InlineText {
                    text: format!("post {index}"),
                },
                Vec::new(),
                None,
            )
            .await;
            target.get_or_insert(envelope);
        }
        let target = target.expect("target");
        docs_sync.clear_queries().await;
        docs_sync.reset_records_returned();
        assert!(
            hydrate_object_in_topic(
                &app.services,
                topic.as_str(),
                &replica,
                &target.id,
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("hydrate")
        );
        let queries = docs_sync.queries().await;
        assert!(
            queries
                .iter()
                .all(|(_, query)| matches!(query, DocQuery::Exact(_))),
            "projecting one post may only read keys, got {queries:?}"
        );
        reads.push((queries.len(), docs_sync.records_returned()));
    }
    assert_eq!(
        reads[0], reads[1],
        "reads must not depend on the replica size"
    );
    // 取り下げの key(record なし)と envelope の key(1 件)。
    assert_eq!(reads[0], (2, 1));
}

// `VerifiedPost::verify` の拒否理由(docs を読まない純粋な検証)。
#[test]
fn verification_rejects_each_kind_of_mismatch() {
    use crate::service::post_integrity::PostRejection;
    let topic = TopicId::new("kukuri:topic:integrity-unit");
    let replica = topic_replica_id(topic.as_str());
    let scope = ReplicaPostScope::for_replica(&replica, topic.as_str()).expect("public scope");
    let keys = generate_keys();
    let envelope = signed_post(&keys, &topic, "unit", ObjectVisibility::Public, None);
    let verify = |envelope: KukuriEnvelope, object_id: &EnvelopeId| {
        VerifiedPost::verify(envelope, object_id, &replica, &scope).map(|_| ())
    };

    assert_eq!(verify(envelope.clone(), &envelope.id), Ok(()));
    // 署名の対象(pubkey・content・tags・created_at)を 1 つでも変えると通らない。
    let mut other_author = envelope.clone();
    other_author.pubkey = Pubkey::from(generate_keys().public_key_hex().as_str());
    assert_eq!(
        verify(other_author, &envelope.id),
        Err(PostRejection::InvalidSignature)
    );
    let mut other_content = envelope.clone();
    other_content.content = other_content.content.replace("unit", "tampered");
    assert_eq!(
        verify(other_content, &envelope.id),
        Err(PostRejection::InvalidSignature)
    );
    // 正しく署名された別の投稿を、この object id の key に置いても通らない。
    let another = signed_post(&keys, &topic, "another", ObjectVisibility::Public, None);
    assert_eq!(
        verify(another, &envelope.id),
        Err(PostRejection::ObjectIdMismatch)
    );
    // 投稿ではない envelope。
    let withdrawal = build_post_withdrawal_envelope(
        &keys,
        &envelope,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal");
    assert_eq!(
        verify(withdrawal.clone(), &withdrawal.id),
        Err(PostRejection::NotAPost)
    );
    // public の replica に、private を申告する投稿・別の topic の投稿・公開範囲が public でない投稿。
    let channel = ChannelId::new("channel-1-abcdef01");
    for mismatched in [
        signed_post(
            &keys,
            &topic,
            "private claim",
            ObjectVisibility::Private,
            Some(&channel),
        ),
        signed_post(
            &keys,
            &TopicId::new("kukuri:topic:integrity-unit-other"),
            "other topic",
            ObjectVisibility::Public,
            None,
        ),
        signed_post(&keys, &topic, "not public", ObjectVisibility::Private, None),
    ] {
        let object_id = mismatched.id.clone();
        assert_eq!(
            verify(mismatched, &object_id),
            Err(PostRejection::ScopeMismatch)
        );
    }
    // 投稿を置かない replica と、購読の topic と合わない public replica には scope が無い。
    assert!(
        ReplicaPostScope::for_replica(&author_replica_id("a".repeat(64).as_str()), topic.as_str())
            .is_none()
    );
    assert!(ReplicaPostScope::for_replica(&replica, "kukuri:topic:integrity-unit-other").is_none());
}
