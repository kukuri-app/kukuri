use super::replica_window::{
    IndexRange, RangeCheckResult, RangeReconcile, TIME_INDEX_FUTURE_ALLOWANCE_SECS,
    ensure_index_entries_projected,
};
use super::*;
use kukuri_docs_sync::{
    BucketReplica, BucketScope, DocKeyOrder, PostReplicaKind, TimeBucket, post_replica_kind,
    query_time_index_asc, query_time_index_desc, query_time_index_window,
};
use std::time::Duration;

pub(super) type RemotePostReplica = (ReplicaId, Option<(String, String)>);
pub(super) type SessionTargetReader = (ReplicaId, Arc<dyn DocsSync>, DocFetchPolicy);

impl AppService {
    /// Read the selected bucket page from one provider at a time. It shares
    /// the signed post/withdrawal projector with the local range check.
    pub(super) async fn reconcile_remote_index_range(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        anchor_seconds: Option<i64>,
        range: &IndexRange<'_>,
        max_entries: usize,
    ) -> Result<RangeReconcile> {
        if max_entries == 0 {
            return Ok(RangeReconcile::default());
        }
        let channel = match scope {
            TimelineScope::Public => PUBLIC_CHANNEL_ID,
            TimelineScope::Channel { channel_id } => channel_id.as_str(),
        };
        let Some(generation) = self
            .services
            .active_content_scope_generation(topic_id, channel)
            .await
        else {
            return Ok(RangeReconcile::default());
        };
        let ledger_key = format!(
            "remote-page:{topic_id}:{channel}:{}:{}:{}:{}",
            range.index_prefix,
            range.order == DocKeyOrder::Ascending,
            range
                .start
                .map(|cursor| format!("{}:{}", cursor.created_at, cursor.object_id))
                .unwrap_or_default(),
            anchor_seconds.unwrap_or_default(),
        );
        let now_ms = Utc::now().timestamp_millis();
        if self
            .services
            .range_checks
            .try_begin(&ledger_key, now_ms)
            .await
            .is_none()
        {
            let (missing, read_past) = self.services.range_checks.last_result(&ledger_key).await;
            return Ok(RangeReconcile {
                unavailable: missing,
                read_past,
                ..RangeReconcile::default()
            });
        }
        let candidates = self
            .remote_page_replicas(
                topic_id,
                scope,
                anchor_seconds,
                range.order == DocKeyOrder::Ascending,
                range.order == DocKeyOrder::Ascending,
            )
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut result = RangeReconcile::default();
        let mut remaining = max_entries;
        for (replica, epoch) in candidates {
            if remaining == 0
                || result.projected >= range.limit
                || tokio::time::Instant::now() >= deadline
            {
                break;
            }
            let readers = match self
                .services
                .until_content_invalid(
                    topic_id,
                    channel,
                    generation,
                    self.remote_post_readers(
                        topic_id,
                        (channel != PUBLIC_CHANNEL_ID).then_some(channel),
                        &replica,
                        epoch
                            .as_ref()
                            .map(|(id, secret)| (id.as_str(), secret.as_str())),
                    ),
                )
                .await
            {
                Some(Ok(readers)) => readers,
                Some(Err(error)) => {
                    warn!(%error, "remote page reader selection failed");
                    continue;
                }
                None => break,
            };
            for reader in readers {
                if remaining == 0 || tokio::time::Instant::now() >= deadline {
                    break;
                }
                let mut services = self.services.clone();
                services.docs_sync = reader;
                let read = async {
                    let page = match (range.order, range.start) {
                        (DocKeyOrder::Descending, None) => {
                            query_time_index_window(
                                services.docs_sync.as_ref(),
                                &replica,
                                range.index_prefix,
                                Utc::now()
                                    .timestamp()
                                    .saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS),
                                range.limit.min(remaining),
                            )
                            .await?
                        }
                        (DocKeyOrder::Descending, before) => {
                            query_time_index_desc(
                                services.docs_sync.as_ref(),
                                &replica,
                                range.index_prefix,
                                before,
                                range.limit.min(remaining),
                            )
                            .await?
                        }
                        (DocKeyOrder::Ascending, after) => {
                            query_time_index_asc(
                                services.docs_sync.as_ref(),
                                &replica,
                                range.index_prefix,
                                after,
                                range.limit.min(remaining),
                            )
                            .await?
                        }
                    };
                    let count = page.entries.len();
                    let mut reaction_targets_left = 0;
                    let outcome = ensure_index_entries_projected(
                        &services,
                        topic_id,
                        &replica,
                        &page.entries,
                        DocFetchPolicy::LocalThenRemote,
                        &mut reaction_targets_left,
                    )
                    .await?;
                    Ok::<_, anyhow::Error>((count, outcome, page.resume))
                };
                match self
                    .services
                    .until_content_invalid(
                        topic_id,
                        channel,
                        generation,
                        tokio::time::timeout_at(deadline, read),
                    )
                    .await
                {
                    Some(Ok(Ok((count, outcome, resume)))) => {
                        remaining = remaining.saturating_sub(count);
                        result.checked += count;
                        result.hydrated += outcome.hydrated;
                        result.projected += outcome.hydrated + outcome.present;
                        result.unavailable += outcome.missing;
                        result.read_past = result.read_past.or(resume);
                        if count > 0 {
                            break;
                        }
                    }
                    Some(Ok(Err(error))) => warn!(%error, "remote page read failed"),
                    Some(Err(_)) | None => break,
                }
            }
        }
        self.services
            .range_checks
            .record_finished(
                &ledger_key,
                now_ms,
                if result.unavailable == 0 {
                    RangeCheckResult::Complete
                } else if result.hydrated > 0 {
                    RangeCheckResult::Progressed
                } else {
                    RangeCheckResult::Stalled
                },
                result.unavailable,
            )
            .await;
        Ok(result)
    }

    pub(crate) async fn private_epoch_for_source(
        &self,
        topic_id: &str,
        channel_id: &str,
        replica: &ReplicaId,
        created_at: i64,
    ) -> Result<Option<(String, String)>> {
        Ok(self
            .remote_page_replicas(
                topic_id,
                &TimelineScope::Channel {
                    channel_id: ChannelId::new(channel_id),
                },
                Some(created_at),
                false,
                false,
            )
            .await?
            .into_iter()
            .find(|(source, _)| source == replica)
            .and_then(|(_, epoch)| epoch))
    }
    /// Legacy writer namespaces still supply local projections during the
    /// transition. Select a fixed window without cloning the epoch history.
    pub(super) async fn local_page_replicas(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        anchor_seconds: Option<i64>,
        ascending: bool,
    ) -> Result<Vec<ReplicaId>> {
        let TimelineScope::Channel { channel_id } = scope else {
            return Ok(vec![topic_replica_id(topic_id)]);
        };
        self.ensure_private_channel_access(topic_id, channel_id)
            .await?;
        let joined = self.joined_private_channels.lock().await;
        let state = joined
            .get(&joined_private_channel_key(topic_id, channel_id.as_str()))
            .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
        let mut replicas = vec![private_channel_replica_for_epoch(
            channel_id.as_str(),
            &state.current_epoch_id,
        )];
        let archived = &state.archived_epochs;
        let selected = anchor_seconds
            .filter(|_| !archived.is_empty())
            .map(|seconds| {
                let at = seconds.saturating_mul(1_000);
                archived
                    .partition_point(|epoch| {
                        epoch_start_millis(&epoch.epoch_id).unwrap_or(i64::MIN) <= at
                    })
                    .saturating_sub(1)
            });
        let candidates: Vec<_> = match selected {
            Some(index) => {
                let neighbor = if ascending {
                    index.saturating_add(1)
                } else {
                    index.saturating_sub(1)
                };
                [archived.get(index), archived.get(neighbor), archived.last()]
                    .into_iter()
                    .flatten()
                    .collect()
            }
            None => archived.iter().rev().take(3).collect(),
        };
        for epoch in candidates {
            let replica = private_channel_replica_for_epoch(channel_id.as_str(), &epoch.epoch_id);
            if !replicas.contains(&replica) {
                replicas.push(replica);
            }
        }
        Ok(replicas)
    }
    pub(super) async fn session_target_candidates(
        &self,
        topic_id: &str,
        source: Option<(&str, &ReplicaId, i64)>,
        id: &str,
    ) -> Result<Vec<RemotePostReplica>> {
        let (scope, anchor) = match source {
            Some((PUBLIC_CHANNEL_ID, _, updated_at)) => {
                (TimelineScope::Public, Some(updated_at / 1_000))
            }
            Some((channel, _, updated_at)) => (
                TimelineScope::Channel {
                    channel_id: ChannelId::new(channel),
                },
                Some(updated_at / 1_000),
            ),
            None => {
                let millis = id
                    .split('-')
                    .nth(1)
                    .and_then(|part| part.parse::<i64>().ok());
                (TimelineScope::Public, millis.map(|value| value / 1_000))
            }
        };
        let mut candidates = self
            .remote_page_replicas(topic_id, &scope, anchor, false, false)
            .await?;
        if let Some((_, replica, _)) = source {
            candidates.retain(|(item, _)| item != replica);
            candidates.insert(
                0,
                (
                    replica.clone(),
                    candidates.first().and_then(|(_, epoch)| epoch.clone()),
                ),
            );
            candidates.truncate(4);
        }
        Ok(candidates)
    }

    pub(super) async fn session_target_readers(
        &self,
        topic_id: &str,
        source: Option<(&str, &ReplicaId, i64)>,
        id: &str,
    ) -> Result<Vec<SessionTargetReader>> {
        let mut readers = Vec::new();
        for (replica, epoch) in self.session_target_candidates(topic_id, source, id).await? {
            let channel = source.map_or(PUBLIC_CHANNEL_ID, |(channel, _, _)| channel);
            if replica == topic_replica_id(topic_id)
                || (channel != PUBLIC_CHANNEL_ID && !replica.as_str().starts_with("bucket::"))
            {
                readers.push((
                    replica.clone(),
                    self.services.docs_sync.clone(),
                    DocFetchPolicy::LocalOnly,
                ));
            } else if channel == PUBLIC_CHANNEL_ID {
                readers.push((
                    replica.clone(),
                    Arc::new(LocalSourceReader {
                        docs: self.services.docs_sync.clone(),
                        replica: replica.clone(),
                    }) as Arc<dyn DocsSync>,
                    DocFetchPolicy::LocalOnly,
                ));
            }
            match self
                .remote_post_readers(
                    topic_id,
                    (channel != PUBLIC_CHANNEL_ID).then_some(channel),
                    &replica,
                    epoch
                        .as_ref()
                        .map(|(id, secret)| (id.as_str(), secret.as_str())),
                )
                .await
            {
                Ok(remote) => readers.extend(
                    remote
                        .into_iter()
                        .map(|reader| (replica.clone(), reader, DocFetchPolicy::LocalThenRemote)),
                ),
                Err(error) => warn!(%error, "session target reader selection failed"),
            }
        }
        Ok(readers)
    }
    /// The visible page selects at most two time buckets and its legacy writer
    /// namespace. A cursor can name an old bucket without enumerating history.
    pub(super) async fn remote_page_replicas(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        anchor_seconds: Option<i64>,
        ascending: bool,
        include_current: bool,
    ) -> Result<Vec<RemotePostReplica>> {
        let bucket = TimeBucket::from_unix_seconds(
            anchor_seconds.unwrap_or_else(|| Utc::now().timestamp()),
        )?;
        let adjacent = if ascending {
            TimeBucket::from_index(bucket.index().saturating_add(1)).ok()
        } else {
            bucket.previous()
        };
        let current = include_current
            .then(|| TimeBucket::from_unix_seconds(Utc::now().timestamp()))
            .transpose()?;
        let mut candidates = Vec::with_capacity(4);
        match scope {
            TimelineScope::Public => {
                for time in [Some(bucket), adjacent, current].into_iter().flatten() {
                    let replica = BucketReplica::new(
                        BucketScope::Topic {
                            topic_id: topic_id.to_owned(),
                        },
                        time,
                    )?
                    .replica_id();
                    if !candidates.iter().any(|(item, _)| item == &replica) {
                        candidates.push((replica, None));
                    }
                }
                candidates.push((topic_replica_id(topic_id), None));
            }
            TimelineScope::Channel { channel_id } => {
                let joined = self.joined_private_channels.lock().await;
                let state = joined
                    .get(&joined_private_channel_key(topic_id, channel_id.as_str()))
                    .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
                let anchor_epoch = private_epochs_for_bucket(state, bucket)
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("private epoch missing"))?;
                for time in [Some(bucket), adjacent, current].into_iter().flatten() {
                    for epoch in private_epochs_for_bucket(state, time) {
                        let replica = BucketReplica::new(
                            BucketScope::PrivateChannel {
                                channel_id: channel_id.as_str().to_owned(),
                                epoch_id: epoch.0.clone(),
                            },
                            time,
                        )?
                        .replica_id();
                        if !candidates.iter().any(|(item, _)| item == &replica) {
                            candidates.push((replica, Some(epoch)));
                        }
                        if candidates.len() == 3 {
                            break;
                        }
                    }
                    if candidates.len() == 3 {
                        break;
                    }
                }
                candidates.push((
                    private_channel_replica_for_epoch(channel_id.as_str(), &anchor_epoch.0),
                    Some(anchor_epoch),
                ));
            }
        }
        Ok(candidates)
    }
    /// One bounded provider window for a verified post replica. The caller
    /// supplies the selected epoch capability, never an untrusted search hit.
    pub(crate) async fn remote_post_readers(
        &self,
        topic_id: &str,
        channel_id: Option<&str>,
        replica: &ReplicaId,
        private_epoch: Option<(&str, &str)>,
    ) -> Result<Vec<Arc<dyn DocsSync>>> {
        let kind = post_replica_kind(replica)
            .ok_or_else(|| anyhow::anyhow!("source is not a post replica"))?;
        let (secret, peers) = match (kind, channel_id, private_epoch) {
            (PostReplicaKind::PublicTopic { topic_id: source }, None, None)
                if source == topic_id =>
            {
                (None, Vec::new())
            }
            (
                PostReplicaKind::PrivateChannel { channel_id: source },
                Some(channel),
                Some((epoch_id, secret_hex)),
            ) if source == channel => {
                self.ensure_private_channel_access(topic_id, &ChannelId::new(channel))
                    .await?;
                let mut epoch_secret = [0_u8; 32];
                hex::decode_to_slice(secret_hex, &mut epoch_secret)?;
                let secret = if replica.as_str().starts_with("bucket::") {
                    let bucket = BucketReplica::parse(replica)?;
                    anyhow::ensure!(
                        matches!(bucket.scope(), BucketScope::PrivateChannel { channel_id, epoch_id: source_epoch } if channel_id == channel && source_epoch == epoch_id),
                        "private bucket scope changed"
                    );
                    bucket.derive_private_secret(&epoch_secret)?
                } else {
                    anyhow::ensure!(
                        replica == &private_channel_replica_for_epoch(channel, epoch_id),
                        "private replica epoch changed"
                    );
                    epoch_secret
                };
                let peers = self
                    .hint_transport()
                    .topic_read_candidates(&private_channel_hint_topic(channel))
                    .await?;
                (Some(secret), peers)
            }
            _ => anyhow::bail!("source replica is outside the selected scope"),
        };
        self.services
            .docs_sync
            .remote_readers(replica, secret, peers)
            .await
    }
}

fn epoch_start_millis(id: &str) -> Option<i64> {
    id.strip_prefix("epoch-")?.split('-').next()?.parse().ok()
}

fn private_epochs_for_bucket(
    state: &JoinedPrivateChannelState,
    bucket: TimeBucket,
) -> Vec<(String, String)> {
    let end = (bucket.end_seconds() as i64)
        .saturating_mul(1_000)
        .saturating_sub(1);
    let start = (bucket.start_seconds() as i64).saturating_mul(1_000);
    let current_at = epoch_start_millis(&state.current_epoch_id).unwrap_or(i64::MIN);
    let archived = &state.archived_epochs;
    let position = archived
        .partition_point(|epoch| epoch_start_millis(&epoch.epoch_id).unwrap_or(i64::MIN) <= end);
    let mut epochs = Vec::with_capacity(2);
    if end >= current_at || archived.is_empty() {
        epochs.push((
            state.current_epoch_id.clone(),
            state.current_epoch_secret_hex.clone(),
        ));
        if current_at > start
            && let Some(previous) = archived.last()
        {
            epochs.push((
                previous.epoch_id.clone(),
                previous.namespace_secret_hex.clone(),
            ));
        }
    } else if let Some(primary) = archived.get(position.saturating_sub(1)) {
        epochs.push((
            primary.epoch_id.clone(),
            primary.namespace_secret_hex.clone(),
        ));
        if epoch_start_millis(&primary.epoch_id).is_some_and(|at| at > start)
            && let Some(previous) = position
                .checked_sub(2)
                .and_then(|index| archived.get(index))
        {
            epochs.push((
                previous.epoch_id.clone(),
                previous.namespace_secret_hex.clone(),
            ));
        }
    }
    epochs
}
