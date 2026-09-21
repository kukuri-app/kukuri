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
    persist_profile_index_entry(
        docs_sync,
        &replica,
        profile_post.created_at,
        &profile_post.object_id,
        "post",
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
    persist_profile_index_entry(
        docs_sync,
        &replica,
        profile_repost.created_at,
        &profile_repost.object_id,
        "repost",
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

/// 自分の custom reaction の asset を読む数の上限(#1239)。
pub(crate) const AUTHOR_REACTION_ASSETS: usize = 512;

pub(crate) async fn fetch_author_envelope_by_id(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    policy: DocFetchPolicy,
) -> Result<Option<KukuriEnvelope>> {
    fetch_author_envelope(docs_sync, replica, envelope_id, None, policy).await
}

/// author replica の `envelopes/<id>` を読む。著者の docs author が分かっていれば、docs author と key の組で 1 件読む
/// (他の名義の record は何件あっても読まない。ADR 0053 §6)。無い・検証に通らないときは、key だけを指定した上限つきの
/// 読み出しに落とす(旧 record)。
pub(crate) async fn fetch_author_envelope(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    docs_author: Option<&str>,
    policy: DocFetchPolicy,
) -> Result<Option<KukuriEnvelope>> {
    let key = stable_key("envelopes", envelope_id.as_str());
    if let Some(docs_author) = docs_author
        && let Some(record) = docs_sync
            .query_replica_by_author(replica, docs_author, key.as_str(), policy)
            .await?
        && let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(record.value.as_slice())
        && envelope.id == *envelope_id
        && envelope.verify().is_ok()
    {
        return Ok(Some(envelope));
    }
    // 同じ key に docs author ごとの record がありうる(誰でも書ける replica)。先頭の 1 件だけを見ず、上限つきで調べ、
    // 署名が通り id が一致する最初の envelope を返す(#1239)。
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            key.as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    for record in records {
        let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(record.value.as_slice()) else {
            continue;
        };
        if envelope.id == *envelope_id && envelope.verify().is_ok() {
            return Ok(Some(envelope));
        }
    }
    Ok(None)
}

pub(crate) async fn load_custom_reaction_assets_from_author_replica(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
    docs_author: Option<&str>,
) -> Result<Vec<CustomReactionAssetDocV1>> {
    // #1239: replica は走査しない。asset 1 件につき `state` と `envelope` の 2 key があるので、
    // `AUTHOR_REACTION_ASSETS` 件の asset ぶんの key を一覧し、`state` の key だけを読む。上限を超える asset は
    // 返さない(best effort)。
    let replica = author_replica_id(author_pubkey);
    // 自分の docs author が分かれば、自分の名義の key だけを一覧し、組で 1 件読む(他の名義の key で窓を埋められず、
    // 他の名義の record で隠されない。ADR 0053 §6)。
    let query = DocKeyQuery {
        prefix: stable_key("reactions/assets", ""),
        order: DocKeyOrder::Ascending,
        limit: AUTHOR_REACTION_ASSETS.saturating_mul(2),
    };
    let page = match docs_author {
        Some(docs_author) => {
            docs_sync
                .query_replica_keys_by_author(&replica, docs_author, query)
                .await?
        }
        None => docs_sync.query_replica_keys(&replica, query).await?,
    };
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in page.entries {
        if !entry.key.ends_with("/state") {
            continue;
        }
        if !seen.insert(entry.key.clone()) {
            continue;
        }
        if let Some(docs_author) = docs_author {
            if let Some(record) = docs_sync
                .query_replica_by_author(
                    &replica,
                    docs_author,
                    entry.key.as_str(),
                    DocFetchPolicy::LocalOnly,
                )
                .await?
                && let Ok(doc) =
                    serde_json::from_slice::<CustomReactionAssetDocV1>(record.value.as_slice())
                && doc.author_pubkey.as_str() == author_pubkey
            {
                items.push(doc);
            }
            continue;
        }
        // 名義が分からない(docs author を持たない実装)。同じ key に docs author ごとの record がありうるので、
        // 先頭の 1 件だけを見ず、上限つきで調べる。
        for record in docs_sync
            .query_replica_exact_bounded(
                &replica,
                entry.key.as_str(),
                MAX_ENVELOPE_RECORDS_PER_OBJECT,
                DocFetchPolicy::LocalOnly,
            )
            .await?
        {
            if let Ok(doc) =
                serde_json::from_slice::<CustomReactionAssetDocV1>(record.value.as_slice())
                && doc.author_pubkey.as_str() == author_pubkey
            {
                items.push(doc);
                break;
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
    docs_author: Option<&str>,
) -> Result<NotificationDocEventBaseline> {
    let key = stable_key("graph/follows", local_author_pubkey);
    let query = DocKeyQuery {
        prefix: key.clone(),
        order: DocKeyOrder::Ascending,
        limit: 1,
    };
    // 相手の docs author が分かれば、その名義の entry だけを起点にする(他の名義の entry で 1 件の枠を埋められない)。
    let page = match docs_author {
        Some(docs_author) => {
            docs_sync
                .query_replica_keys_by_author(replica, docs_author, query)
                .await?
        }
        None => docs_sync.query_replica_keys(replica, query).await?,
    };
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
