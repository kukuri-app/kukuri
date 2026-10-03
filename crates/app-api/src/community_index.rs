use crate::service::*;
use crate::timeout_at;
use n0_future::time::{Instant, timeout};
use std::time::Duration;

/// 1 候補 peer への試行の上限。batch 全体の 30 秒の内に、候補 4 peer を順に試せる長さ(ADR 0055 §3)。
const PROVIDER_READ_TIMEOUT: Duration = Duration::from_secs(7);

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
        let remote_deadline = Instant::now() + Duration::from_secs(30);
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
                || input
                    .source_replica_id
                    .as_deref()
                    .is_some_and(|source| !valid_index_source(&topic, &input.channel_ref, source))
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

        // #1403: 手元で確かめられる entry は、期限の外で先に解決する。remote から読む entry は group をまたいで
        // 1 本の stream にまとめ、ある group の読取りの待ちが後ろの group の読取りを止めないようにする。
        let mut failed_entries = HashSet::new();
        let mut remote_requests = Vec::new();
        let mut ready_groups = Vec::new();
        for ((_, source), (topic, scope, entries)) in groups {
            // #1239: scope を走査しない。解決対象の object だけを、projection に無いときに key 指定で反映する。
            let mut scope_ready = source.is_some()
                || self
                    .allowed_channel_id_for_scope(topic.as_str(), &scope)
                    .await
                    .is_ok();
            if scope_ready && let Some(source) = source.as_deref() {
                for input in &entries {
                    match self
                        .resolve_index_source(&topic, &scope, source, &input.object_id, false)
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => remote_requests.push((
                            input.key.clone(),
                            topic.clone(),
                            scope.clone(),
                            source.to_string(),
                            input.object_id.clone(),
                        )),
                        // A bad locator does not hide another result from the same bucket.
                        Err(_) => {
                            failed_entries.insert(input.key.clone());
                        }
                    }
                }
            } else if scope_ready {
                for input in &entries {
                    if self
                        .ensure_object_projection(
                            topic.as_str(),
                            &scope,
                            &EnvelopeId::from(input.object_id.clone()),
                            DocFetchPolicy::LocalOnly,
                        )
                        .await
                        .is_err()
                    {
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
            ready_groups.push((source, topic, scope, entries));
        }

        let mut unverified = remote_requests
            .iter()
            .map(|(key, ..)| key.clone())
            .collect::<HashSet<_>>();
        let mut reads = futures_util::stream::iter(remote_requests.into_iter().map(
            |(key, topic, scope, source, object_id)| async move {
                let resolved = self
                    .resolve_index_source(&topic, &scope, &source, &object_id, true)
                    .await;
                (key, resolved)
            },
        ))
        .buffer_unordered(8);
        while let Ok(Some((key, resolved))) = timeout_at(remote_deadline, reads.next()).await {
            unverified.remove(&key);
            if !matches!(resolved, Ok(true)) {
                failed_entries.insert(key);
            }
        }
        drop(reads);
        failed_entries.extend(unverified);

        for (source, topic, scope, entries) in ready_groups {
            let write_allowed = match &scope {
                TimelineScope::Channel { channel_id } => self
                    .private_channel_state_for_owner_action(
                        topic.as_str(),
                        channel_id,
                        PrivateChannelOwnerAction::WriteCheck,
                    )
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
                    && matches!(input.channel_ref, ChannelRef::Public)
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

    /// `remote` が false なら手元(projection と手元の docs)だけで確かめ、network I/O をしない。false を返した
    /// entry は、呼出元が `remote` を true にして候補 peer から読む。
    async fn resolve_index_source(
        &self,
        topic: &str,
        scope: &TimelineScope,
        source: &str,
        object_id: &str,
        remote: bool,
    ) -> Result<bool> {
        let object_id = EnvelopeId::from(object_id.to_string());
        let (channel, channel_ref) = match scope {
            TimelineScope::Public => (PUBLIC_CHANNEL_ID, ChannelRef::Public),
            TimelineScope::Channel { channel_id } => (
                channel_id.as_str(),
                ChannelRef::PrivateChannel {
                    channel_id: channel_id.clone(),
                },
            ),
        };
        let Some(scope_generation) = self
            .services
            .active_content_scope_generation(topic, channel)
            .await
        else {
            return Ok(false);
        };
        let replica = ReplicaId::new(source);
        let mut services = self.services.clone();
        if !remote {
            if let Some(cached) = self
                .services
                .projection_store
                .get_object_projection(&object_id)
                .await?
            {
                anyhow::ensure!(
                    cached.topic_id == topic
                        && cached.channel_id == channel
                        && valid_index_source(
                            topic,
                            &channel_ref,
                            cached.source_replica_id.as_str()
                        ),
                    "cached post does not belong to the requested source"
                );
                if matches!(scope, TimelineScope::Public) {
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
                }
                return Ok(true);
            }
            if matches!(scope, TimelineScope::Public) {
                services.docs_sync = Arc::new(LocalSourceReader {
                    docs: self.services.docs_sync.clone(),
                    replica: replica.clone(),
                });
                return Ok(hydrate_object_in_topic_with(
                    &services,
                    topic,
                    &replica,
                    &object_id,
                    None,
                    DocFetchPolicy::LocalOnly,
                    BodyFetch::LocalOnly,
                )
                .await?
                    == ObjectHydration::Hydrated);
            }
            return Ok(false);
        }
        let private_epoch = match scope {
            TimelineScope::Public => None,
            TimelineScope::Channel { channel_id } => Some(
                self.private_channel_indexing_capability_known(topic, channel_id.as_str())
                    .await?,
            ),
        };
        let Some(readers) = self
            .services
            .until_content_invalid(
                topic,
                channel,
                scope_generation,
                self.remote_post_readers(
                    topic,
                    (channel != PUBLIC_CHANNEL_ID).then_some(channel),
                    &replica,
                    private_epoch
                        .as_ref()
                        .map(|(epoch, secret)| (epoch.as_str(), secret.as_str())),
                ),
            )
            .await
        else {
            return Ok(false);
        };
        for reader in readers? {
            services.docs_sync = reader;
            let Some(result) = self
                .services
                .until_content_invalid(
                    topic,
                    channel,
                    scope_generation,
                    // #1403: 応答しない候補で batch の期限を使い切らず、次の候補へ進む。
                    timeout(
                        PROVIDER_READ_TIMEOUT,
                        hydrate_object_in_topic_with(
                            &services,
                            topic,
                            &replica,
                            &object_id,
                            None,
                            DocFetchPolicy::LocalThenRemote,
                            BodyFetch::LocalOnly,
                        ),
                    ),
                )
                .await
            else {
                return Ok(false);
            };
            match result {
                Ok(Ok(ObjectHydration::Hydrated)) => return Ok(true),
                Ok(Ok(ObjectHydration::Missing | ObjectHydration::Invalid)) => {}
                Ok(Err(error)) => warn!(%error, "source provider read failed"),
                Err(_) => warn!("source provider read timed out"),
            }
        }
        Ok(false)
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
                    || !valid_index_source(
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

fn valid_index_source(topic: &str, channel: &ChannelRef, source: &str) -> bool {
    let replica = ReplicaId::new(source);
    match channel {
        ChannelRef::Public => {
            source == topic_replica_id(topic).as_str()
                || kukuri_docs_sync::BucketReplica::parse(&replica).is_ok_and(|bucket| {
                    matches!(bucket.scope(), kukuri_docs_sync::BucketScope::Topic { topic_id } if topic_id == topic)
                })
        }
        ChannelRef::PrivateChannel { channel_id } => {
            matches!(
                kukuri_docs_sync::post_replica_kind(&replica),
                Some(kukuri_docs_sync::PostReplicaKind::PrivateChannel { channel_id: source })
                    if source == channel_id.as_str()
            )
        }
    }
}
