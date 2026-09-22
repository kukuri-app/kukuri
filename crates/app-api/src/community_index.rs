use crate::service::*;
#[path = "community_index_source_reader.rs"]
mod reader;
use reader::LocalSourceReader;

impl AppService {
    pub async fn resolve_community_index_posts(
        &self,
        inputs: Vec<CommunityIndexPostResolveInput>,
    ) -> Result<CommunityIndexPostResolveResponse> {
        if inputs.len() > 100 {
            anyhow::bail!("community index post resolve accepts at most 100 entries");
        }

        let mut ordered_keys = Vec::new();
        let mut seen_keys = BTreeSet::new();
        let mut results = HashMap::<String, CommunityIndexResolvedPostView>::new();
        let mut groups = BTreeMap::<
            (String, Option<String>),
            (String, TimelineScope, Vec<CommunityIndexPostResolveInput>),
        >::new();
        for input in inputs {
            if !seen_keys.insert(input.key.clone()) {
                continue;
            }
            ordered_keys.push(input.key.clone());
            let topic = input.topic.trim().to_string();
            let object_id = input.object_id.trim();
            let author_pubkey = input.author_pubkey.trim();
            if input.key.is_empty()
                || topic.is_empty()
                || object_id.is_empty()
                || author_pubkey.is_empty()
                || input.source_replica_id.as_deref().is_some_and(|source| {
                    !valid_public_index_source(&topic, &input.channel_ref, source)
                })
            {
                results.insert(
                    input.key.clone(),
                    CommunityIndexResolvedPostView {
                        key: input.key,
                        post: None,
                        capabilities: CommunityIndexPostActionCapabilitiesView::default(),
                    },
                );
                continue;
            }
            let (group_key, scope) = match &input.channel_ref {
                ChannelRef::Public => (format!("{topic}\0public"), TimelineScope::Public),
                ChannelRef::PrivateChannel { channel_id } => (
                    format!("{topic}\0private\0{}", channel_id.as_str()),
                    TimelineScope::Channel {
                        channel_id: channel_id.clone(),
                    },
                ),
            };
            groups
                .entry((group_key, input.source_replica_id.clone()))
                .or_insert_with(|| (topic, scope, Vec::new()))
                .2
                .push(input);
        }

        for ((_, source), (topic, scope, entries)) in groups {
            let mut failed_entries = HashSet::new();
            // #1239: scope を走査しない。解決対象の object だけを、projection に無いときに key 指定で反映する。
            let mut scope_ready = source.is_some()
                || self
                    .ensure_scope_subscriptions(topic.as_str(), &scope)
                    .await
                    .is_ok();
            if scope_ready {
                for input in &entries {
                    let resolved = if let Some(source) = source.as_deref() {
                        self.resolve_public_index_source(&topic, source, &input.object_id)
                            .await
                    } else {
                        self.ensure_object_projection(
                            topic.as_str(),
                            &scope,
                            &EnvelopeId::from(input.object_id.clone()),
                            DocFetchPolicy::LocalOnly,
                        )
                        .await
                    };
                    if resolved.is_err() {
                        if source.is_some() {
                            // locatorは結果1件の手がかり。不正な1件で同じbucketの正常な結果を消さない。
                            failed_entries.insert(input.key.clone());
                            continue;
                        }
                        scope_ready = false;
                        break;
                    }
                }
            }
            if !scope_ready {
                for input in entries {
                    results.insert(
                        input.key.clone(),
                        CommunityIndexResolvedPostView {
                            key: input.key,
                            post: None,
                            capabilities: CommunityIndexPostActionCapabilitiesView::default(),
                        },
                    );
                }
                continue;
            }

            let write_allowed = match &scope {
                TimelineScope::Channel { channel_id } => self
                    .private_channel_write_state(topic.as_str(), channel_id)
                    .await
                    .is_ok(),
                TimelineScope::Public => true,
            };
            for input in entries {
                let unresolved = || CommunityIndexResolvedPostView {
                    key: input.key.clone(),
                    post: None,
                    capabilities: CommunityIndexPostActionCapabilitiesView::default(),
                };
                if failed_entries.contains(&input.key) {
                    results.insert(input.key.clone(), unresolved());
                    continue;
                }
                let projection = match self
                    .services
                    .projection_store
                    .get_object_projection(&EnvelopeId::from(input.object_id.clone()))
                    .await
                {
                    Ok(Some(projection)) => projection,
                    Ok(None) | Err(_) => {
                        results.insert(input.key.clone(), unresolved());
                        continue;
                    }
                };
                let channel_matches = match &input.channel_ref {
                    ChannelRef::Public => projection.channel_id == PUBLIC_CHANNEL_ID,
                    ChannelRef::PrivateChannel { channel_id } => {
                        projection.channel_id == channel_id.as_str()
                    }
                };
                if projection.topic_id != topic
                    || projection.author_pubkey != input.author_pubkey
                    || !channel_matches
                    || !matches!(
                        projection.object_kind.as_str(),
                        "post" | "comment" | "repost"
                    )
                {
                    results.insert(input.key.clone(), unresolved());
                    continue;
                }
                if source.is_some()
                    && self
                        .refresh_local_index_reference_withdrawals(&projection)
                        .await
                        .is_err()
                {
                    results.insert(input.key.clone(), unresolved());
                    continue;
                }
                let mut view = match self
                    .page_to_view_with_policy(
                        Page {
                            items: vec![projection],
                            next_cursor: None,
                        },
                        if source.is_some() {
                            DocFetchPolicy::LocalOnly
                        } else {
                            DocFetchPolicy::LocalThenRemote
                        },
                    )
                    .await
                {
                    Ok(view) => view,
                    Err(_) => {
                        results.insert(input.key.clone(), unresolved());
                        continue;
                    }
                };
                let Some(post) = view.items.pop() else {
                    results.insert(input.key.clone(), unresolved());
                    continue;
                };
                let active = post.withdrawal.is_none();
                let post_or_comment = matches!(post.object_kind.as_str(), "post" | "comment");
                let public = post.channel_id.is_none();
                let capabilities = CommunityIndexPostActionCapabilitiesView {
                    open_thread: active && post.is_threadable,
                    reply: active && post.is_threadable && write_allowed,
                    repost: active && public && post_or_comment,
                    quote_repost: active && public && post_or_comment,
                    react: active && post_or_comment && write_allowed,
                    copy_link: true,
                    bookmark: active
                        && matches!(post.object_kind.as_str(), "post" | "comment" | "repost"),
                    withdraw: active
                        && write_allowed
                        && post.author_pubkey == self.current_author_pubkey(),
                };
                results.insert(
                    input.key.clone(),
                    CommunityIndexResolvedPostView {
                        key: input.key,
                        post: Some(post),
                        capabilities,
                    },
                );
            }
        }

        Ok(CommunityIndexPostResolveResponse {
            entries: ordered_keys
                .into_iter()
                .filter_map(|key| results.remove(key.as_str()))
                .collect(),
        })
    }

    async fn resolve_public_index_source(
        &self,
        topic: &str,
        source: &str,
        object_id: &str,
    ) -> Result<bool> {
        let object_id = EnvelopeId::from(object_id.to_string());
        if let Some(cached) = self
            .services
            .projection_store
            .get_object_projection(&object_id)
            .await?
        {
            anyhow::ensure!(
                cached.topic_id == topic
                    && cached.channel_id == PUBLIC_CHANNEL_ID
                    && valid_public_index_source(
                        topic,
                        &ChannelRef::Public,
                        cached.source_replica_id.as_str()
                    ),
                "cached post does not belong to the requested public source"
            );
            let reader = LocalSourceReader {
                docs: self.services.docs_sync.clone(),
                replica: cached.source_replica_id.clone(),
            };
            hydrate_post_withdrawal_for_object_with_hints(
                &reader,
                self.services.projection_store.as_ref(),
                &cached.source_replica_id,
                &object_id,
                WithdrawalReadHints {
                    target_docs_author: cached.source_docs_author.as_deref(),
                    writer_docs_author: None,
                },
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            return Ok(true);
        }
        let mut services = self.services.clone();
        services.docs_sync = Arc::new(LocalSourceReader {
            docs: self.services.docs_sync.clone(),
            replica: ReplicaId::new(source),
        });
        Ok(hydrate_object_in_topic_with(
            &services,
            topic,
            &ReplicaId::new(source),
            &object_id,
            None,
            DocFetchPolicy::LocalOnly,
            BodyFetch::LocalOnly,
        )
        .await?
            == ObjectHydration::Hydrated)
    }

    async fn refresh_local_index_reference_withdrawals(
        &self,
        row: &ObjectProjectionRow,
    ) -> Result<()> {
        // 返信先とrepost元の高々2件だけ。参照先の全bucketは探さず、viewの背景remoteも起動しない。
        let mut references = Vec::with_capacity(2);
        if let Some(target) = row.reply_to_object_id.as_ref() {
            references.push((row.topic_id.as_str(), target, row.source_replica_id.clone()));
        }
        if let Some(snapshot) = row.repost_of.as_ref() {
            references.push((
                snapshot.source_topic_id.as_str(),
                &snapshot.source_object_id,
                topic_replica_id(snapshot.source_topic_id.as_str()),
            ));
        }
        for (topic, target, fallback) in references {
            let cached = self
                .services
                .projection_store
                .get_object_projection(target)
                .await?;
            let (replica, author) = if let Some(cached) = cached {
                if cached.topic_id != topic
                    || cached.channel_id != PUBLIC_CHANNEL_ID
                    || !valid_public_index_source(
                        topic,
                        &ChannelRef::Public,
                        cached.source_replica_id.as_str(),
                    )
                {
                    continue;
                }
                (cached.source_replica_id, cached.source_docs_author)
            } else {
                (fallback, None)
            };
            let reader = LocalSourceReader {
                docs: self.services.docs_sync.clone(),
                replica: replica.clone(),
            };
            hydrate_post_withdrawal_for_object_with_hints(
                &reader,
                self.services.projection_store.as_ref(),
                &replica,
                target,
                WithdrawalReadHints {
                    target_docs_author: author.as_deref(),
                    writer_docs_author: None,
                },
                DocFetchPolicy::LocalOnly,
            )
            .await?;
        }
        Ok(())
    }
}

fn valid_public_index_source(topic: &str, channel: &ChannelRef, source: &str) -> bool {
    if !matches!(channel, ChannelRef::Public) {
        return false;
    }
    if source == topic_replica_id(topic).as_str() {
        return true;
    }
    kukuri_docs_sync::BucketReplica::parse(&ReplicaId::new(source)).is_ok_and(|bucket| {
        matches!(bucket.scope(), kukuri_docs_sync::BucketScope::Topic { topic_id } if topic_id == topic)
    })
}
