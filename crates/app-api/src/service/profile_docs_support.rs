use super::*;
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};

pub(crate) async fn persist_profile_doc(
    docs_sync: &dyn DocsSync,
    profile: &Profile,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    let replica = author_replica_id(profile.pubkey.as_str());
    docs_sync.open_replica(&replica).await?;
    let latest = serde_json::to_value(AuthorProfileDocV1 {
        author_pubkey: profile.pubkey.clone(),
        name: profile.name.clone(),
        display_name: profile.display_name.clone(),
        about: profile.about.clone(),
        picture_asset: profile.picture_asset.clone(),
        updated_at: profile.updated_at,
        envelope_id: envelope.id.clone(),
    })?;
    for (key, value) in [
        (stable_key("profile", "latest"), latest),
        (
            stable_key("envelopes", envelope.id.as_str()),
            serde_json::to_value(envelope)?,
        ),
    ] {
        let records = docs_sync
            .query_replica_with_policy(
                &replica,
                DocQuery::Exact(key.clone()),
                DocFetchPolicy::LocalOnly,
            )
            .await?;
        let already_saved = records.iter().any(|record| {
            serde_json::from_slice::<serde_json::Value>(&record.value)
                .ok()
                .as_ref()
                == Some(&value)
        });
        if !already_saved {
            docs_sync
                .apply_doc_op(&replica, DocOp::SetJson { key, value })
                .await?;
        }
    }
    Ok(())
}

pub(crate) async fn persist_profile_post_doc(
    docs_sync: &dyn DocsSync,
    profile_post: &ProfilePost,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    let replica = author_replica_id(profile_post.author_pubkey.as_str());
    docs_sync.open_replica(&replica).await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("profile/posts", profile_post.object_id.as_str()),
                value: serde_json::to_value(AuthorProfilePostDocV1 {
                    author_pubkey: profile_post.author_pubkey.clone(),
                    profile_topic_id: profile_post.profile_topic_id.clone(),
                    published_topic_id: profile_post.published_topic_id.clone(),
                    object_id: profile_post.object_id.clone(),
                    created_at: profile_post.created_at,
                    object_kind: profile_post.object_kind.clone(),
                    content: profile_post.content.clone(),
                    attachments: profile_post.attachments.clone(),
                    reply_to_object_id: profile_post.reply_to_object_id.clone(),
                    root_id: profile_post.root_id.clone(),
                    content_labels: profile_post.content_labels.clone(),
                    envelope_id: envelope.id.clone(),
                })?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("envelopes", envelope.id.as_str()),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_profile_repost_doc(
    docs_sync: &dyn DocsSync,
    profile_repost: &ProfileRepost,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    let replica = author_replica_id(profile_repost.author_pubkey.as_str());
    docs_sync.open_replica(&replica).await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("profile/reposts", profile_repost.object_id.as_str()),
                value: serde_json::to_value(AuthorProfileRepostDocV1 {
                    author_pubkey: profile_repost.author_pubkey.clone(),
                    profile_topic_id: profile_repost.profile_topic_id.clone(),
                    published_topic_id: profile_repost.published_topic_id.clone(),
                    object_id: profile_repost.object_id.clone(),
                    created_at: profile_repost.created_at,
                    commentary: profile_repost.commentary.clone(),
                    repost_of: profile_repost.repost_of.clone(),
                    envelope_id: envelope.id.clone(),
                })?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("envelopes", envelope.id.as_str()),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_follow_edge_doc(
    docs_sync: &dyn DocsSync,
    edge: &FollowEdge,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    let replica = author_replica_id(edge.subject_pubkey.as_str());
    docs_sync.open_replica(&replica).await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("graph/follows", edge.target_pubkey.as_str()),
                value: serde_json::to_value(FollowEdgeDocV1 {
                    subject_pubkey: edge.subject_pubkey.clone(),
                    target_pubkey: edge.target_pubkey.clone(),
                    status: edge.status.clone(),
                    updated_at: edge.updated_at,
                    envelope_id: edge.envelope_id.clone(),
                })?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("envelopes", envelope.id.as_str()),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_block_edge_doc(
    docs_sync: &dyn DocsSync,
    edge: &BlockEdge,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    let replica = author_replica_id(edge.subject_pubkey.as_str());
    docs_sync.open_replica(&replica).await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("graph/blocks", edge.target_pubkey.as_str()),
                value: serde_json::to_value(BlockEdgeDocV1 {
                    subject_pubkey: edge.subject_pubkey.clone(),
                    target_pubkey: edge.target_pubkey.clone(),
                    status: edge.status,
                    updated_at: edge.updated_at,
                    envelope_id: edge.envelope_id.clone(),
                })?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("envelopes", envelope.id.as_str()),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_custom_reaction_asset_doc(
    docs_sync: &dyn DocsSync,
    asset: &CustomReactionAssetDocV1,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    let replica = author_replica_id(asset.author_pubkey.as_str());
    docs_sync.open_replica(&replica).await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("reactions/assets", &format!("{}/state", asset.asset_id)),
                value: serde_json::to_value(asset)?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("reactions/assets", &format!("{}/envelope", asset.asset_id)),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("envelopes", envelope.id.as_str()),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_reaction_doc(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    reaction: &ReactionDocV1,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "reactions",
                    &format!(
                        "{}/{}/state",
                        reaction.target_object_id.as_str(),
                        reaction.reaction_id.as_str()
                    ),
                ),
                value: serde_json::to_value(reaction)?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "reactions",
                    &format!(
                        "{}/{}/envelope",
                        reaction.target_object_id.as_str(),
                        reaction.reaction_id.as_str()
                    ),
                ),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("envelopes", envelope.id.as_str()),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

/// author replica の follow・block の edge を、起動時と追いつきで読む key の数の上限(#1239)。
///
/// author replica は、その author の follow・block の数だけ key を持つ。全件は読まない。上限を超える edge は、
/// その key の docs の event が届いたときに反映する(best effort)。
pub(crate) const AUTHOR_EDGE_KEYS: usize = 512;
/// 自分の custom reaction の asset を読む数の上限(#1239)。
pub(crate) const AUTHOR_REACTION_ASSETS: usize = 512;

/// author replica の反映の結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AuthorHydration {
    /// 読めて検証に通った record の数。
    pub(crate) reflected: usize,
    /// そのうち、手元に無かった envelope の数(関係の再計算と、追いつきの間隔に使う)。
    pub(crate) changed: usize,
}

impl AuthorHydration {
    fn reflected(changed: bool) -> Self {
        Self {
            reflected: 1,
            changed: usize::from(changed),
        }
    }

    fn add(&mut self, other: Self) {
        self.reflected += other.reflected;
        self.changed += other.changed;
    }
}

/// 起動時と追いつきで読む author replica の key。`profile/latest` と、`graph/follows/`・`graph/blocks/` の
/// key の上限つきの一覧(それぞれ `AUTHOR_EDGE_KEYS` 件)。値は読まない。
async fn author_state_keys(docs_sync: &dyn DocsSync, replica: &ReplicaId) -> Result<Vec<String>> {
    let mut keys = vec![stable_key("profile", "latest")];
    for prefix in ["graph/follows/", "graph/blocks/"] {
        let page = docs_sync
            .query_replica_keys(
                replica,
                DocKeyQuery {
                    prefix: prefix.to_string(),
                    order: DocKeyOrder::Ascending,
                    limit: AUTHOR_EDGE_KEYS,
                },
            )
            .await?;
        keys.extend(page.entries.into_iter().map(|entry| entry.key));
    }
    Ok(keys)
}

/// author の状態(profile・follow・block)を、上限つきで反映する(#1239)。replica は走査しない。
///
/// 起動時と復旧で使う。読むのは `author_state_keys` の key と、その record だけ。関係は、反映の有無に
/// かかわらず再計算する(起動時に、手元の edge から関係を作り直す)。戻り値は読めた record の数。
pub(crate) async fn hydrate_author_state(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<usize> {
    let replica = author_replica_id(author_pubkey);
    let mut outcome = AuthorHydration::default();
    for key in author_state_keys(services.docs_sync.as_ref(), &replica).await? {
        outcome.add(hydrate_author_record(services, author_pubkey, &replica, &key, policy).await?);
    }
    rebuild_author_relationships(
        services.store.as_ref(),
        services.projection_store.as_ref(),
        local_author_pubkey,
    )
    .await?;
    Ok(outcome.reflected)
}

/// 同期の区切りと取りこぼしの後の追いつき(#1239)。読む範囲は `hydrate_author_state` と同じ。関係の再計算は、
/// 手元に無かった envelope が入ったときだけ行う。
pub(crate) async fn catch_up_author_state(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let replica = author_replica_id(author_pubkey);
    let mut outcome = AuthorHydration::default();
    for key in author_state_keys(services.docs_sync.as_ref(), &replica).await? {
        outcome.add(hydrate_author_record(services, author_pubkey, &replica, &key, policy).await?);
    }
    if outcome.changed > 0 {
        rebuild_author_relationships(
            services.store.as_ref(),
            services.projection_store.as_ref(),
            local_author_pubkey,
        )
        .await?;
    }
    Ok(outcome)
}

/// docs の event が指す author replica の key を 1 つ反映する(#1239)。replica は走査しない。
/// 関係の再計算は、手元に無かった envelope が入ったときだけ行う。
pub(crate) async fn hydrate_author_key(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    key: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let replica = author_replica_id(author_pubkey);
    let outcome = hydrate_author_record(services, author_pubkey, &replica, key, policy).await?;
    if outcome.changed > 0 {
        rebuild_author_relationships(
            services.store.as_ref(),
            services.projection_store.as_ref(),
            local_author_pubkey,
        )
        .await?;
    }
    Ok(outcome)
}

/// `profile/latest`・`graph/follows/<target>`・`graph/blocks/<target>` の key を 1 つ読んで反映する。
/// それ以外の key は何もしない。読めない record と、検証に通らない record は飛ばす(warn)。
async fn hydrate_author_record(
    services: &ServiceHandles,
    author_pubkey: &str,
    replica: &ReplicaId,
    key: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let docs_sync = services.docs_sync.as_ref();
    let store = services.store.as_ref();
    let projection_store = services.projection_store.as_ref();
    let is_profile = key == stable_key("profile", "latest");
    let is_follow = key.starts_with("graph/follows/");
    if !(is_profile || is_follow || key.starts_with("graph/blocks/")) {
        return Ok(AuthorHydration::default());
    }
    let Some(record) = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Exact(key.to_string()),
        policy,
    )
    .await?
    .into_iter()
    .next() else {
        return Ok(AuthorHydration::default());
    };
    if is_profile {
        match serde_json::from_slice::<AuthorProfileDocV1>(record.value.as_slice()) {
            Ok(doc) if doc.author_pubkey.as_str() == author_pubkey => {
                if let Some(envelope) =
                    fetch_author_envelope_by_id(docs_sync, replica, &doc.envelope_id, policy)
                        .await?
                {
                    let changed = store.get_envelope(&envelope.id).await?.is_none();
                    store.put_envelope(envelope.clone()).await?;
                    if let Some(profile) = parse_profile(&envelope)? {
                        projection_store.upsert_profile_cache(profile).await?;
                    }
                    return Ok(AuthorHydration::reflected(changed));
                }
            }
            Ok(_) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    "ignoring profile doc with mismatched author"
                );
            }
            Err(error) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    error = %error,
                    "failed to decode author profile doc"
                );
            }
        }
        return Ok(AuthorHydration::default());
    }
    if is_follow {
        match serde_json::from_slice::<FollowEdgeDocV1>(record.value.as_slice()) {
            Ok(doc) if doc.subject_pubkey.as_str() == author_pubkey => {
                if let Some(envelope) =
                    fetch_author_envelope_by_id(docs_sync, replica, &doc.envelope_id, policy)
                        .await?
                    && let Some(edge) = parse_follow_edge(&envelope)?
                    && edge.target_pubkey == doc.target_pubkey
                    && edge.status == doc.status
                {
                    let changed = store.get_envelope(&envelope.id).await?.is_none();
                    store.put_envelope(envelope).await?;
                    return Ok(AuthorHydration::reflected(changed));
                }
            }
            Ok(_) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    "ignoring follow doc with mismatched subject"
                );
            }
            Err(error) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    error = %error,
                    "failed to decode follow edge doc"
                );
            }
        }
        return Ok(AuthorHydration::default());
    }
    match serde_json::from_slice::<BlockEdgeDocV1>(record.value.as_slice()) {
        Ok(doc) if doc.subject_pubkey.as_str() == author_pubkey => {
            if let Some(envelope) =
                fetch_author_envelope_by_id(docs_sync, replica, &doc.envelope_id, policy).await?
                && let Some(edge) = parse_block_edge(&envelope)?
                && edge.target_pubkey == doc.target_pubkey
                && edge.status == doc.status
            {
                let changed = store.get_envelope(&envelope.id).await?.is_none();
                store.put_envelope(envelope).await?;
                return Ok(AuthorHydration::reflected(changed));
            }
        }
        Ok(_) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                "ignoring block doc with mismatched subject"
            );
        }
        Err(error) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                error = %error,
                "failed to decode block edge doc"
            );
        }
    }
    Ok(AuthorHydration::default())
}

pub(crate) async fn fetch_author_envelope_by_id(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    policy: DocFetchPolicy,
) -> Result<Option<KukuriEnvelope>> {
    let Some(record) = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Exact(stable_key("envelopes", envelope_id.as_str())),
        policy,
    )
    .await?
    .into_iter()
    .next() else {
        return Ok(None);
    };
    let envelope: KukuriEnvelope = serde_json::from_slice(record.value.as_slice())?;
    envelope.verify()?;
    Ok(Some(envelope))
}

pub(crate) async fn load_custom_reaction_assets_from_author_replica(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
) -> Result<Vec<CustomReactionAssetDocV1>> {
    // #1239: replica は走査しない。asset 1 件につき `state` と `envelope` の 2 key があるので、
    // `AUTHOR_REACTION_ASSETS` 件の asset ぶんの key を一覧し、`state` の key だけを読む。上限を超える asset は
    // 返さない(best effort)。
    let replica = author_replica_id(author_pubkey);
    let page = docs_sync
        .query_replica_keys(
            &replica,
            DocKeyQuery {
                prefix: stable_key("reactions/assets", ""),
                order: DocKeyOrder::Ascending,
                limit: AUTHOR_REACTION_ASSETS.saturating_mul(2),
            },
        )
        .await?;
    let mut items = Vec::new();
    for entry in page.entries {
        if !entry.key.ends_with("/state") {
            continue;
        }
        let Some(record) = docs_sync
            .query_replica(&replica, DocQuery::Exact(entry.key))
            .await?
            .into_iter()
            .next()
        else {
            continue;
        };
        let doc: CustomReactionAssetDocV1 = serde_json::from_slice(record.value.as_slice())?;
        if doc.author_pubkey.as_str() == author_pubkey {
            items.push(doc);
        }
    }
    Ok(items)
}

pub(crate) async fn load_profile_posts_from_author_replica(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<Vec<ProfilePost>> {
    let author_pubkey = normalize_author_pubkey(author_pubkey)?;
    let replica = author_replica_id(author_pubkey.as_str());
    let expected_profile_topic_id = author_profile_topic_id(author_pubkey.as_str());
    let mut items = Vec::new();
    let mut seen_object_ids = BTreeSet::new();

    for record in query_replica_with_fetch_policy(
        docs_sync,
        &replica,
        DocQuery::Prefix("profile/posts/".into()),
        policy,
    )
    .await?
    {
        match serde_json::from_slice::<AuthorProfilePostDocV1>(record.value.as_slice()) {
            Ok(doc)
                if doc.author_pubkey.as_str() == author_pubkey
                    && doc.profile_topic_id == expected_profile_topic_id =>
            {
                if let Some(envelope) =
                    fetch_author_envelope_by_id(docs_sync, &replica, &doc.envelope_id, policy)
                        .await?
                {
                    match parse_profile_post(&envelope) {
                        Ok(Some(profile_post))
                            if profile_post.author_pubkey == doc.author_pubkey
                                && profile_post.profile_topic_id == doc.profile_topic_id
                                && profile_post.published_topic_id == doc.published_topic_id
                                && profile_post.object_id == doc.object_id
                                && profile_post.created_at == doc.created_at
                                && profile_post.object_kind == doc.object_kind
                                && profile_post.content == doc.content
                                && profile_post.attachments == doc.attachments
                                && profile_post.reply_to_object_id == doc.reply_to_object_id
                                && profile_post.root_id == doc.root_id =>
                        {
                            if seen_object_ids.insert(profile_post.object_id.clone()) {
                                items.push(profile_post);
                            }
                        }
                        Ok(Some(_)) | Ok(None) => {}
                        Err(error) => {
                            warn!(
                                author_pubkey = %author_pubkey,
                                key = %record.key,
                                envelope_id = %doc.envelope_id.as_str(),
                                error = %error,
                                "ignoring invalid profile post envelope"
                            );
                        }
                    }
                }
            }
            Ok(_) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    "ignoring profile post doc with mismatched author or topic"
                );
            }
            Err(error) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    error = %error,
                    "failed to decode profile post doc"
                );
            }
        }
    }

    Ok(items)
}

pub(crate) async fn load_profile_reposts_from_author_replica(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<Vec<ProfileRepost>> {
    let author_pubkey = normalize_author_pubkey(author_pubkey)?;
    let replica = author_replica_id(author_pubkey.as_str());
    let expected_profile_topic_id = author_profile_topic_id(author_pubkey.as_str());
    let mut items = Vec::new();
    let mut seen_object_ids = BTreeSet::new();

    for record in query_replica_with_fetch_policy(
        docs_sync,
        &replica,
        DocQuery::Prefix("profile/reposts/".into()),
        policy,
    )
    .await?
    {
        match serde_json::from_slice::<AuthorProfileRepostDocV1>(record.value.as_slice()) {
            Ok(doc)
                if doc.author_pubkey.as_str() == author_pubkey
                    && doc.profile_topic_id == expected_profile_topic_id =>
            {
                if let Some(envelope) =
                    fetch_author_envelope_by_id(docs_sync, &replica, &doc.envelope_id, policy)
                        .await?
                {
                    match parse_profile_repost(&envelope) {
                        Ok(Some(profile_repost))
                            if profile_repost.author_pubkey == doc.author_pubkey
                                && profile_repost.profile_topic_id == doc.profile_topic_id
                                && profile_repost.published_topic_id == doc.published_topic_id
                                && profile_repost.object_id == doc.object_id
                                && profile_repost.created_at == doc.created_at
                                && profile_repost.commentary == doc.commentary
                                && profile_repost.repost_of == doc.repost_of =>
                        {
                            if seen_object_ids.insert(profile_repost.object_id.clone()) {
                                items.push(profile_repost);
                            }
                        }
                        Ok(Some(_)) | Ok(None) => {}
                        Err(error) => {
                            warn!(
                                author_pubkey = %author_pubkey,
                                key = %record.key,
                                envelope_id = %doc.envelope_id.as_str(),
                                error = %error,
                                "ignoring invalid profile repost envelope"
                            );
                        }
                    }
                }
            }
            Ok(_) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    "ignoring profile repost doc with mismatched author or topic"
                );
            }
            Err(error) => {
                warn!(
                    author_pubkey = %author_pubkey,
                    key = %record.key,
                    error = %error,
                    "failed to decode profile repost doc"
                );
            }
        }
    }

    Ok(items)
}

/// follow の通知の起点(#1239)。replica は走査しない。
///
/// 通知になるのは、自分を指す follow(`graph/follows/<自分>`)だけなので、その key 1 件の key と content hash
/// だけを読む。値は読まない。
pub(crate) async fn snapshot_follow_notification_baseline(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    local_author_pubkey: &str,
) -> Result<NotificationDocEventBaseline> {
    let key = stable_key("graph/follows", local_author_pubkey);
    let page = docs_sync
        .query_replica_keys(
            replica,
            DocKeyQuery {
                prefix: key.clone(),
                order: DocKeyOrder::Ascending,
                limit: 1,
            },
        )
        .await?;
    Ok(NotificationDocEventBaseline::from_key_entries(
        page.entries.iter().filter(|entry| entry.key == key),
    ))
}

pub(crate) fn merge_seed_peers(
    configured_seed_peers: Vec<SeedPeer>,
    bootstrap_seed_peers: Vec<SeedPeer>,
) -> Vec<SeedPeer> {
    let mut deduped = BTreeMap::new();
    for seed_peer in configured_seed_peers
        .into_iter()
        .chain(bootstrap_seed_peers.into_iter())
    {
        let key = match seed_peer.addr_hint.as_deref() {
            Some(addr_hint) => format!("{}@{}", seed_peer.endpoint_id, addr_hint),
            None => seed_peer.endpoint_id.clone(),
        };
        deduped.insert(key, seed_peer);
    }
    deduped.into_values().collect()
}
