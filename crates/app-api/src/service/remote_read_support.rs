use super::projection_support::legacy_epoch_id;
use super::replica_window::{
    IndexRange, RangeCheckResult, RangeReconcile, TIME_INDEX_FUTURE_ALLOWANCE_SECS,
    ensure_index_entries_projected,
};
use super::*;
use kukuri_docs_sync::{
    BucketReplica, BucketScope, DocKeyOrder, PostReplicaKind, TimeBucket, TimeIndexCursor,
    post_replica_kind, query_time_index_asc, query_time_index_desc, query_time_index_window,
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
        let candidates = self
            .remote_page_replicas(
                topic_id,
                scope,
                anchor_seconds,
                range.order == DocKeyOrder::Ascending,
                range.order == DocKeyOrder::Ascending,
            )
            .await?;
        let mut sources = Vec::new();
        for (replica, epoch) in candidates {
            let selection = self
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
                .await;
            match selection {
                Some(Ok(readers)) => sources.extend(
                    readers
                        .into_iter()
                        .enumerate()
                        .map(|(slot, reader)| (replica.clone(), reader, slot)),
                ),
                Some(Err(error)) => warn!(%error, "remote page reader selection failed"),
                None => return Ok(RangeReconcile::default()),
            }
        }
        let count_sources = sources.len();
        if count_sources == 0 {
            return Ok(RangeReconcile::default());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut total = RangeReconcile::default();
        for (source_index, (replica, reader, provider_slot)) in sources.into_iter().enumerate() {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let budget = max_entries
                .saturating_sub(total.checked)
                .div_ceil(count_sources - source_index);
            if budget == 0 {
                break;
            }
            let provider = reader
                .remote_reader_id()
                .unwrap_or_else(|| format!("slot-{provider_slot}"));
            let ledger_key = format!(
                "remote-page:{topic_id}:{channel}:{}:{}:{}:{}:{}:{provider}",
                replica.as_str(),
                range.index_prefix,
                range.order == DocKeyOrder::Ascending,
                range
                    .start
                    .map(|cursor| format!("{}:{}", cursor.created_at, cursor.object_id))
                    .unwrap_or_default(),
                anchor_seconds.unwrap_or_default(),
            );
            let now_ms = Utc::now().timestamp_millis();
            let Some(resume_from) = self
                .services
                .range_checks
                .try_begin(&ledger_key, now_ms)
                .await
            else {
                let (missing, read_past) =
                    self.services.range_checks.last_result(&ledger_key).await;
                total.merge_from(
                    RangeReconcile {
                        unavailable: missing,
                        read_past,
                        ..RangeReconcile::default()
                    },
                    range.order,
                );
                continue;
            };
            let mut services = self.services.clone();
            services.docs_sync = reader;
            let read = async {
                let mut count = 0;
                let mut position = resume_from.or_else(|| range.start.cloned());
                let mut outcome = super::replica_window::RangeCheckOutcome::default();
                let mut read_past = None;
                let mut reaction_targets_left = 0;
                loop {
                    let wanted = range
                        .limit
                        .saturating_sub(outcome.hydrated + outcome.present)
                        .max(1)
                        .min(budget.saturating_sub(count));
                    if wanted == 0 {
                        read_past = position;
                        break;
                    }
                    let page = match (range.order, position.as_ref()) {
                        (DocKeyOrder::Descending, None) => {
                            query_time_index_window(
                                services.docs_sync.as_ref(),
                                &replica,
                                range.index_prefix,
                                Utc::now()
                                    .timestamp()
                                    .saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS),
                                wanted,
                            )
                            .await?
                        }
                        (DocKeyOrder::Descending, cursor) => {
                            query_time_index_desc(
                                services.docs_sync.as_ref(),
                                &replica,
                                range.index_prefix,
                                cursor,
                                wanted,
                            )
                            .await?
                        }
                        (DocKeyOrder::Ascending, cursor) => {
                            query_time_index_asc(
                                services.docs_sync.as_ref(),
                                &replica,
                                range.index_prefix,
                                cursor,
                                wanted,
                            )
                            .await?
                        }
                    };
                    let page_count = page.entries.len();
                    count += page_count;
                    if let Some(last) = page.entries.last() {
                        position = Some(TimeIndexCursor {
                            created_at: last.created_at,
                            object_id: last.object_id.clone(),
                        });
                    }
                    if page_count > 0 {
                        outcome.merge(
                            ensure_index_entries_projected(
                                &services,
                                topic_id,
                                &replica,
                                &page.entries,
                                DocFetchPolicy::LocalThenRemote,
                                &mut reaction_targets_left,
                            )
                            .await?,
                        );
                    }
                    if outcome.hydrated + outcome.present >= range.limit {
                        break;
                    }
                    if let Some(resume) = page.resume {
                        read_past = Some(resume);
                        break;
                    }
                    if page_count < wanted {
                        break;
                    }
                    if count >= budget {
                        read_past = position;
                        break;
                    }
                }
                Ok::<_, anyhow::Error>((count, outcome, read_past))
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
                Some(Ok(Ok((count, outcome, read_past)))) => {
                    total.merge_from(
                        RangeReconcile {
                            checked: count,
                            hydrated: outcome.hydrated,
                            projected: outcome.hydrated + outcome.present,
                            unavailable: outcome.missing,
                            read_past: read_past.clone(),
                        },
                        range.order,
                    );
                    if let Some(cursor) = read_past {
                        self.services
                            .range_checks
                            .record_resume(&ledger_key, cursor, outcome.missing)
                            .await;
                    } else {
                        self.services
                            .range_checks
                            .record_finished(&ledger_key, now_ms, outcome.result(), outcome.missing)
                            .await;
                    }
                }
                Some(Ok(Err(error))) => {
                    warn!(%error, "remote page read failed");
                    self.services
                        .range_checks
                        .record_finished(&ledger_key, now_ms, RangeCheckResult::Stalled, 0)
                        .await;
                }
                Some(Err(_)) | None => break,
            }
        }
        Ok(total)
    }

    pub(crate) async fn private_epoch_for_source(
        &self,
        topic_id: &str,
        channel_id: &str,
        replica: &ReplicaId,
    ) -> Result<Option<(String, String)>> {
        let epoch_id = if replica.as_str().starts_with("bucket::") {
            let bucket = BucketReplica::parse(replica)?;
            let BucketScope::PrivateChannel {
                channel_id: source,
                epoch_id,
            } = bucket.scope()
            else {
                return Ok(None);
            };
            if source != channel_id {
                return Ok(None);
            }
            epoch_id.clone()
        } else if replica == &private_channel_replica_id(channel_id) {
            legacy_epoch_id().to_owned()
        } else {
            let prefix = format!("channel::{channel_id}::epoch::");
            let Some(epoch) = replica.as_str().strip_prefix(&prefix) else {
                return Ok(None);
            };
            if epoch.is_empty() || epoch.contains("::") {
                return Ok(None);
            }
            epoch.to_owned()
        };
        let joined = self.joined_private_channels.lock().await;
        let state = joined
            .get(&joined_private_channel_key(topic_id, channel_id))
            .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
        if epoch_id == state.current_epoch_id {
            return Ok(Some((epoch_id, state.current_epoch_secret_hex.clone())));
        }
        let start = epoch_start_millis(&epoch_id).unwrap_or(i64::MIN);
        let index = state
            .archived_epochs
            .partition_point(|item| epoch_start_millis(&item.epoch_id).unwrap_or(i64::MIN) < start);
        Ok(state
            .archived_epochs
            .get(index)
            .filter(|item| item.epoch_id == epoch_id)
            .map(|item| (item.epoch_id.clone(), item.namespace_secret_hex.clone())))
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
            if let Some(index) = candidates.iter().position(|(item, _)| item == replica) {
                let selected = candidates.remove(index);
                candidates.insert(0, selected);
            } else if matches!(scope, TimelineScope::Public) {
                candidates.insert(0, (replica.clone(), None));
            } else if let TimelineScope::Channel { channel_id } = &scope
                && let Some(epoch) = self
                    .private_epoch_for_source(topic_id, channel_id.as_str(), replica)
                    .await?
            {
                candidates.insert(0, (replica.clone(), Some(epoch)));
            }
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
