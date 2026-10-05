use super::replica_window::{
    IndexRange, RangeCheckResult, RangeReconcile, TIME_INDEX_FUTURE_ALLOWANCE_SECS,
    ensure_index_entries_projected,
};
use super::*;
use kukuri_docs_sync::{
    BucketReplica, BucketScope, DocKeyOrder, PostReplicaKind, TimeBucket, TimeIndexCursor,
    post_replica_kind, query_time_index_asc, query_time_index_desc, query_time_index_window,
};
use kukuri_store::PrivateChannelEpochRange;
use std::time::Duration;

pub(super) type RemotePostReplica = (ReplicaId, Option<(String, String)>);
pub(super) type SessionTargetReader = (ReplicaId, Arc<dyn DocsSync>, DocFetchPolicy);

/// 1 操作の remote 読取りの期限(R5-B と同じ)。
pub(crate) const REMOTE_READ_DEADLINE: Duration = Duration::from_secs(30);
const WRITER_PROVIDERS: usize = 4;
const WRITER_DESTINATION_TIMEOUT: Duration = Duration::from_secs(5);

/// 1 対象を手元(`LocalOnly`)から読み、無ければ `readers` の provider を順に読む(#1221 R5-C)。
///
/// provider は手元で読めなかったときだけ選ぶ。provider の失敗は次の provider へ進み、手元の読取りの失敗は返す。
/// 全体の期限は `REMOTE_READ_DEADLINE`。`read` には source を所有して渡し、借用の lifetime を future へ持ち込まない
/// (呼び出し側の future が `Send` のまま spawn できるように)。
pub(crate) async fn read_local_then_remote<T, Fut>(
    local: Arc<dyn DocsSync>,
    readers: impl Future<Output = Vec<Arc<dyn DocsSync>>>,
    read: impl Fn(Arc<dyn DocsSync>, DocFetchPolicy) -> Fut,
) -> Result<Option<T>>
where
    Fut: Future<Output = Result<Option<T>>>,
{
    if let Some(value) = read(local, DocFetchPolicy::LocalOnly).await? {
        return Ok(Some(value));
    }
    let deadline = n0_future::time::Instant::now() + REMOTE_READ_DEADLINE;
    for reader in readers.await {
        let result = crate::timeout_at(
            deadline,
            read(reader.clone(), DocFetchPolicy::LocalThenRemote),
        )
        .await;
        reader.finish_remote_object().await;
        match result {
            Ok(Ok(Some(value))) => return Ok(Some(value)),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => warn!(%error, "object read from a provider failed"),
            Err(_) => break,
        }
    }
    Ok(None)
}

/// author と private の制御参照を読む provider(#1221 R5-C)。最大 4 件。
///
/// 書き手(author 本人、channel owner、token の発行者)の検証済み宛先(R4-A の account 宛先解決)を先に置き、
/// 呼び出し側が渡す `scope` の peer(private は当該 channel の gossip scope、公開 author の profile は投稿が載る
/// topic の参加者。#1419)を続け、公開は全体の候補窓で残りを埋める。
/// private(`secret` あり)の要求は capability の証明つきで、書き手と scope の peer だけへ送る。
pub(crate) async fn writer_readers(
    services: &ServiceHandles,
    replica: &ReplicaId,
    writers: &[&str],
    secret: Option<[u8; 32]>,
    scope: Vec<SeedPeer>,
) -> Vec<Arc<dyn DocsSync>> {
    let mut peers: Vec<SeedPeer> = Vec::new();
    for writer in writers {
        let resolved = n0_future::time::timeout(
            WRITER_DESTINATION_TIMEOUT,
            services
                .hint_transport
                .resolve_receive_destination(&Pubkey::from(*writer)),
        )
        .await;
        if let Ok(Ok(Some(address))) = resolved {
            peers.push(SeedPeer {
                endpoint_id: address.id.to_string(),
                addr_hint: address.ip_addrs().next().map(ToString::to_string),
            });
        }
    }
    for peer in scope {
        if !peers
            .iter()
            .any(|known| known.endpoint_id == peer.endpoint_id)
        {
            peers.push(peer);
        }
    }
    peers.truncate(WRITER_PROVIDERS);
    let mut readers = Vec::new();
    if !peers.is_empty() {
        match services
            .docs_sync
            .remote_readers(replica, secret, peers)
            .await
        {
            Ok(selected) => readers.extend(selected),
            Err(error) => warn!(%error, "writer provider selection failed"),
        }
    }
    if secret.is_none() && readers.len() < WRITER_PROVIDERS {
        match services
            .docs_sync
            .remote_readers(replica, None, Vec::new())
            .await
        {
            Ok(window) => {
                for reader in window {
                    if readers.len() < WRITER_PROVIDERS
                        && !readers.iter().any(|known: &Arc<dyn DocsSync>| {
                            known.remote_reader_id() == reader.remote_reader_id()
                        })
                    {
                        readers.push(reader);
                    }
                }
            }
            Err(error) => warn!(%error, "public provider window selection failed"),
        }
    }
    readers
}

impl AppService {
    /// Read the selected bucket page from one provider at a time. It shares
    /// the signed post/withdrawal projector with the local range check.
    ///
    /// `ledger` が偽の読み直し(lease の開始・再接続・日の境界。#1221 R5-H)は、照合の台帳を使わず、毎回各 bucket の
    /// 先頭(新しい側)から 1 ページを読む。読み残しの位置を持ち越さず、記録もしない。
    pub(super) async fn reconcile_remote_index_range(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        anchor_seconds: Option<i64>,
        range: &IndexRange<'_>,
        max_entries: usize,
        ledger: bool,
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
        let deadline = n0_future::time::Instant::now() + Duration::from_secs(30);
        let mut total = RangeReconcile::default();
        for (source_index, (replica, reader, provider_slot)) in sources.into_iter().enumerate() {
            if n0_future::time::Instant::now() >= deadline {
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
            let begun = match ledger {
                true => {
                    self.services
                        .range_checks
                        .try_begin(&ledger_key, now_ms)
                        .await
                }
                false => Some(None),
            };
            let Some(resume_from) = begun else {
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
                let mut reaction_targets_left =
                    super::replica_window::REMOTE_RANGE_CHECK_REACTION_TARGETS;
                // 既に projection にある投稿の reaction の窓は、台帳つきの表示の照合だけが読み直す(#1567 AC-2)。
                // 起動時・日境界・溢れの読み直し(台帳なし)では読まない(読む量を変えない)。
                let mut present_reaction_targets_left = if ledger {
                    super::replica_window::REMOTE_RANGE_CHECK_REACTION_TARGETS
                } else {
                    0
                };
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
                                &mut present_reaction_targets_left,
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
                    crate::timeout_at(deadline, read),
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
                    if let Some(cursor) = read_past.filter(|_| ledger) {
                        self.services
                            .range_checks
                            .record_resume(&ledger_key, cursor, outcome.missing)
                            .await;
                    } else if ledger {
                        self.services
                            .range_checks
                            .record_finished(&ledger_key, now_ms, outcome.result(), outcome.missing)
                            .await;
                    }
                }
                Some(Ok(Err(error))) => {
                    warn!(%error, "remote page read failed");
                    if ledger {
                        self.services
                            .range_checks
                            .record_finished(&ledger_key, now_ms, RangeCheckResult::Stalled, 0)
                            .await;
                    }
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
        let Some((source, epoch_id)) = kukuri_docs_sync::private_channel_epoch_of(replica) else {
            return Ok(None);
        };
        if source != channel_id {
            return Ok(None);
        }
        self.ensure_private_channel_access(topic_id, &ChannelId::new(channel_id))
            .await?;
        // その世代の鍵の行を key で 1 件読む(過去の世代をメモリに持たない。ADR 0061 §9)。
        Ok(self
            .services
            .private_channel_epoch_secret(channel_id, &epoch_id)
            .await?
            .map(|secret| (epoch_id, secret)))
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
        let state = self
            .joined_private_channel_state(topic_id, channel_id.as_str())
            .await
            .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
        let channel = channel_id.as_str();
        let mut replicas = vec![private_channel_replica_for_epoch(
            channel,
            &state.current_epoch_id,
        )];
        // 過去の世代は開始時刻の索引で、anchor に掛かる世代と隣・最新の過去の世代だけを読む(履歴を列挙しない)。
        let archived_before = epoch_started_at(&state.current_epoch_id).saturating_sub(1);
        let mut candidates = Vec::with_capacity(3);
        if let Some(seconds) = anchor_seconds {
            let at = seconds.saturating_mul(1_000).min(archived_before);
            candidates.extend(
                self.services
                    .private_channel_epoch_ids(
                        channel,
                        PrivateChannelEpochRange::AtOrBefore(at),
                        if ascending { 1 } else { 2 },
                    )
                    .await?,
            );
            if ascending {
                candidates.extend(
                    self.services
                        .private_channel_epoch_ids(channel, PrivateChannelEpochRange::After(at), 1)
                        .await?
                        .into_iter()
                        .filter(|epoch| epoch_started_at(epoch) <= archived_before),
                );
            }
        }
        candidates.extend(
            self.services
                .private_channel_epoch_ids(
                    channel,
                    PrivateChannelEpochRange::AtOrBefore(archived_before),
                    if anchor_seconds.is_some() { 1 } else { 3 },
                )
                .await?,
        );
        for epoch in candidates {
            let replica = private_channel_replica_for_epoch(channel, &epoch);
            if !replicas.contains(&replica) {
                replicas.push(replica);
            }
        }
        Ok(replicas)
    }
    /// `channel` は projection に行が無い session(hint・読み直し・表示で初めて読む)の scope。
    pub(super) async fn session_target_candidates(
        &self,
        topic_id: &str,
        channel: &str,
        source: Option<(&str, &ReplicaId, i64)>,
        id: &str,
        kind: &str,
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
                let scope = match channel {
                    PUBLIC_CHANNEL_ID => TimelineScope::Public,
                    channel => TimelineScope::Channel {
                        channel_id: ChannelId::new(channel),
                    },
                };
                (scope, millis.map(|value| value / 1_000))
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
        // #1221 R5-H: 現在と直前の bucket の locator が指す replica(日が変わった更新・回転や切替で移した session の
        // state)を先に読む。
        for located in self
            .session_locators(topic_id, &scope, kind, id)
            .await?
            .into_iter()
            .rev()
        {
            candidates.retain(|(replica, _)| *replica != located.0);
            candidates.insert(0, located);
        }
        Ok(candidates)
    }

    /// 現在と直前の bucket(private は現 epoch)に置かれた、session の state の replica を指す locator。同じ scope の
    /// bucket だけを受け取る。
    async fn session_locators(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        kind: &str,
        id: &str,
    ) -> Result<Vec<RemotePostReplica>> {
        let key = stable_key(&format!("sessions/{kind}"), &format!("{id}/locator"));
        let channel = match scope {
            TimelineScope::Public => None,
            TimelineScope::Channel { channel_id } => Some(channel_id.as_str()),
        };
        let mut located: Vec<RemotePostReplica> = Vec::new();
        for (bucket, epoch) in self
            .remote_page_replicas(topic_id, scope, None, false, false)
            .await?
            .into_iter()
            .filter(|(replica, _)| replica.as_str().starts_with("bucket::"))
        {
            let readers = async {
                self.remote_post_readers(
                    topic_id,
                    channel,
                    &bucket,
                    epoch
                        .as_ref()
                        .map(|(id, secret)| (id.as_str(), secret.as_str())),
                )
                .await
                .unwrap_or_default()
            };
            // 公開の bucket は namespace を作らない手元の読取り、private は capability を持つ手元の docs。
            let local: Arc<dyn DocsSync> = match channel {
                None => Arc::new(LocalSourceReader {
                    docs: self.services.docs_sync.clone(),
                    replica: bucket.clone(),
                }),
                Some(_) => self.services.docs_sync.clone(),
            };
            let found = read_local_then_remote(local, readers, |docs, policy| {
                let (bucket, key) = (bucket.clone(), key.clone());
                async move {
                    Ok(docs
                        .query_replica_with_policy(&bucket, DocQuery::Exact(key), policy)
                        .await?
                        .into_iter()
                        .next())
                }
            })
            .await
            .unwrap_or_else(|error| {
                warn!(%error, "session locator read failed");
                None
            });
            let Some(target) = found
                .and_then(|record| serde_json::from_slice::<String>(&record.value).ok())
                .map(ReplicaId::new)
            else {
                continue;
            };
            let Ok(parsed) = BucketReplica::parse(&target) else {
                continue;
            };
            let epoch = match (parsed.scope(), channel) {
                (BucketScope::Topic { topic_id: topic }, None) if topic == topic_id => None,
                (BucketScope::PrivateChannel { channel_id, .. }, Some(channel))
                    if channel_id == channel =>
                {
                    match self
                        .private_epoch_for_source(topic_id, channel, &target)
                        .await?
                    {
                        Some(epoch) => Some(epoch),
                        None => continue,
                    }
                }
                _ => continue,
            };
            if !located.iter().any(|(replica, _)| *replica == target) {
                located.push((target, epoch));
            }
        }
        Ok(located)
    }

    pub(super) async fn session_target_readers(
        &self,
        topic_id: &str,
        channel: &str,
        source: Option<(&str, &ReplicaId, i64)>,
        id: &str,
        kind: &str,
    ) -> Result<Vec<SessionTargetReader>> {
        let mut readers = Vec::new();
        let channel = source.map_or(channel, |(channel, _, _)| channel);
        for (replica, epoch) in self
            .session_target_candidates(topic_id, channel, source, id, kind)
            .await?
        {
            // private は旧 replica も bucket も、capability を持つ手元の docs から読む(自分が書いた session を、channel の
            // 他の参加者がいなくても読める。#1221 R5-H)。
            if replica == topic_replica_id(topic_id) || channel != PUBLIC_CHANNEL_ID {
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
                let state = self
                    .joined_private_channel_state(topic_id, channel_id.as_str())
                    .await
                    .ok_or_else(|| anyhow::anyhow!("private channel is not joined"))?;
                let anchor_epoch = private_epochs_for_bucket(&self.services, &state, bucket)
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("private epoch missing"))?;
                for time in [Some(bucket), adjacent, current].into_iter().flatten() {
                    for epoch in private_epochs_for_bucket(&self.services, &state, time).await? {
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
                // 同じ topic の参加者(gossip の neighbor)から読む。まだ居なければ全体の台帳から選ぶ(#1395)。
                let peers = self
                    .hint_transport()
                    .topic_read_candidates(&TopicId::new(topic_id))
                    .await?;
                (None, peers)
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

pub(super) fn epoch_start_millis(id: &str) -> Option<i64> {
    id.strip_prefix("epoch-")?.split('-').next()?.parse().ok()
}

/// bucket に掛かる世代(現在の世代か、bucket の終わりより前に始まった最新の過去の世代と、その直前の世代)。
/// 開始時刻の索引で 2 件まで読む(過去の世代をメモリに持たない。ADR 0061 §9)。
pub(super) async fn private_epochs_for_bucket(
    services: &ServiceHandles,
    state: &JoinedPrivateChannelState,
    bucket: TimeBucket,
) -> Result<Vec<(String, String)>> {
    let end = (bucket.end_seconds() as i64)
        .saturating_mul(1_000)
        .saturating_sub(1);
    let start = (bucket.start_seconds() as i64).saturating_mul(1_000);
    let channel = state.channel_id.as_str();
    let current_at = epoch_started_at(&state.current_epoch_id);
    let primary = if end >= current_at {
        Some((
            state.current_epoch_id.clone(),
            state.current_epoch_secret_hex.clone(),
        ))
    } else {
        services
            .private_channel_epochs(channel, PrivateChannelEpochRange::AtOrBefore(end), 1)
            .await?
            .pop()
    };
    let Some(primary) = primary else {
        return Ok(vec![(
            state.current_epoch_id.clone(),
            state.current_epoch_secret_hex.clone(),
        )]);
    };
    let primary_at = epoch_started_at(&primary.0);
    let mut epochs = vec![primary];
    if primary_at > start {
        epochs.extend(
            services
                .private_channel_epochs(
                    channel,
                    PrivateChannelEpochRange::AtOrBefore(primary_at.saturating_sub(1)),
                    1,
                )
                .await?,
        );
    }
    Ok(epochs)
}
