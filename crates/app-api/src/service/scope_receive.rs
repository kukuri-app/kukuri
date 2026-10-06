//! #1221 R5-H: 旧 sync なしの受信。lease の task は docs の replica を開かず購読もしない。
//!
//! - gossip hint を受けた対象だけを exact に読む(手元 → 有界な provider)。
//! - lease の開始(task の作成)・endpoint の世代の変化(task の作り直し)・日の境界で、現在と直前の bucket を
//!   1 ページ(最大 200 行)だけ読み直す。続きを全件読みに行かない。
//! - content hint(短命種以外)の受付は窓あたり `HINT_WINDOW_BUDGET` 件まで。超えた分は読まずに捨て、窓の終わりに同じ
//!   読み直しを 1 回行う(#1567 AC-1)。topic の活動量が増えても、1 つの lease が読む量は窓あたりの定数に収まる。
//! - 取り込んだ投稿から、従来どおり通知を作る(`ServiceHandles::notify_remote_posts`)。

use super::replica_window::RANGE_CHECK_REACTIONS_PER_OBJECT;
use super::subscription_catch_up::REPLICA_WINDOW_ENTRIES;
use super::*;
use kukuri_docs_sync::{BUCKET_SECONDS_V1, DocKeyOrder, DocKeyQuery};
use tracing::debug;

/// account 同期の scope の holder。
const ACCOUNT_SYNC_HOLDER: &str = "account-sync";

/// lease の読み直しで、replica ごとに種類ごとに読む session の索引の上限(判断 5)。
const SESSION_REREAD_LIMIT: usize = 64;

/// lease の task が 1 つの窓で受け付ける content hint(短命種以外)の上限(#1567 AC-1)。件数の上限であって「足りる」の
/// 根拠ではない。超えた分は peer の学習も docs の読みもせずに捨て、窓の終わりの読み直し 1 回で回収する。
pub(crate) const HINT_WINDOW_BUDGET: usize = 32;
/// 受付の窓の長さ。表示の照合の間隔(`RANGE_CHECK_INTERVAL_MS`)と同じ。
pub(crate) const HINT_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

/// 短命種(その場で反映し、一覧では回収できない)。受付の上限を通さない(#1567 INVAR-1)。
fn is_ephemeral_hint(hint: &GossipHint) -> bool {
    matches!(
        hint,
        GossipHint::MetaverseRoomEvent { .. }
            | GossipHint::DomeHostHeartbeat { .. }
            | GossipHint::LivePresence { .. }
    )
}

/// 読み直して、1 件以上反映したら最後の同期時刻を更新する(live/game 一覧の再取得の契機。#1567 AC-1)。
async fn reread_and_mark(
    reader: &AppService,
    topic_id: &str,
    scope: &TimelineScope,
    last_sync: &session_projection::SyncClock,
) {
    if reader.reread_scope(topic_id, scope).await > 0 {
        last_sync.set(Utc::now().timestamp_millis()).await;
    }
}

/// scope の Spatial Context(公開は topic、private channel は channel)。
fn scope_context(topic_id: &str, scope: &TimelineScope) -> kukuri_core::SpatialContextV1 {
    let topic_id = TopicId::new(topic_id);
    match scope {
        TimelineScope::Public => kukuri_core::SpatialContextV1::Topic { topic_id },
        TimelineScope::Channel { channel_id } => kukuri_core::SpatialContextV1::Channel {
            topic_id,
            channel_id: channel_id.clone(),
        },
    }
}

/// 次の日の境界(UTC、bucket の切れ目)までの時間。
pub(crate) fn until_next_bucket(now_secs: i64) -> std::time::Duration {
    let day = BUCKET_SECONDS_V1 as i64;
    let next = now_secs
        .div_euclid(day)
        .saturating_add(1)
        .saturating_mul(day);
    std::time::Duration::from_secs(next.saturating_sub(now_secs).max(1) as u64)
}

impl AppService {
    /// task の中で使う読み手。`ServiceHandles` を共有する(参加状態・通知の Notify は同じ Arc)。
    pub(crate) fn scope_reader(&self) -> AppService {
        let mut services = self.services.clone();
        // 読み直しは lease の開始・再接続(task の作り直し)と日の境界だけ。表示の照合の間隔の台帳を共有すると、
        // 直前に表示が照合した範囲を読み飛ばすので、task ごとの台帳にする。
        services.range_checks = Arc::default();
        AppService::from_handles(services)
    }

    pub(crate) async fn spawn_subscription_task(
        &self,
        topic_id: &str,
        channel_id: Option<ChannelId>,
        hint_topic: TopicId,
    ) -> Result<ScopeTask> {
        let reader = self.scope_reader();
        let metaverse_room_events = Arc::clone(&self.metaverse_room_events);
        let dome_host_heartbeats = Arc::clone(&self.dome_host_heartbeats);
        let last_sync = Arc::clone(&self.last_sync_ts);
        let public_topic_delivery = Arc::clone(&self.public_topic_delivery);
        let topic = topic_id.to_string();
        let storage_channel_id = channel_storage_id(channel_id.as_ref());
        let scope = match channel_id {
            None => TimelineScope::Public,
            Some(channel_id) => TimelineScope::Channel { channel_id },
        };
        let context = scope_context(topic_id, &scope);
        let generation = if scope == TimelineScope::Public {
            let generation = self.next_subscription_generation(topic_id).await;
            self.reset_public_topic_delivery_generation(topic_id, generation)
                .await;
            Some(generation)
        } else {
            None
        };
        let mut hint_stream = self
            .services
            .hint_transport
            .subscribe_hints(&hint_topic)
            .await?;
        let handle = n0_future::task::spawn(async move {
            reread_and_mark(&reader, &topic, &scope, &last_sync).await;
            // 窓あたりの content hint の受付(#1567 AC-1)。`dropped` は、この窓で読まずに捨てた hint の数(transport の
            // 購読 stream が取りこぼした分を含む)。1 件以上なら、何を落としたか分からないので窓の終わりに 1 回読み直す。
            let mut window_end = n0_future::time::Instant::now() + HINT_WINDOW;
            let mut window_used = 0usize;
            let mut dropped = 0u64;
            loop {
                let boundary = n0_future::time::sleep(until_next_bucket(Utc::now().timestamp()));
                let window = n0_future::time::sleep_until(window_end);
                tokio::select! {
                    Some(event) = hint_stream.next() => {
                        dropped = dropped.saturating_add(event.dropped_before);
                        if !hint_targets_topic(&event.hint, topic.as_str()) {
                            continue;
                        }
                        // 短命種以外は窓あたり HINT_WINDOW_BUDGET 件まで。超えた分は peer の学習も docs の読みもせずに捨てる。
                        if !is_ephemeral_hint(&event.hint) {
                            if window_used >= HINT_WINDOW_BUDGET {
                                dropped = dropped.saturating_add(1);
                                continue;
                            }
                            window_used += 1;
                        }
                        let now = Utc::now().timestamp_millis();
                        // hint の送り手を reader・blob の候補台帳へ入れる(hint の exact 読取りの provider になる)。
                        if !event.source_peer.is_empty() {
                            if let Err(error) = reader.services.docs_sync.learn_peer(&event.source_peer).await {
                                warn!(%error, "failed to learn docs peer from hint event");
                            }
                            if let Err(error) = reader.services.blob_service.learn_peer(&event.source_peer).await {
                                warn!(%error, "failed to learn blob peer from hint event");
                            }
                        }
                        match &event.hint {
                            GossipHint::MetaverseRoomEvent { event: envelope, .. } => {
                                match parse_metaverse_room_event_envelope(
                                    envelope.as_ref().clone(),
                                    event.received_at,
                                    event.source_peer.clone(),
                                ) {
                                    Ok(Some(view)) => {
                                        push_metaverse_room_event_buffer(&metaverse_room_events, view).await;
                                        last_sync.set(now).await;
                                    }
                                    Ok(None) => {}
                                    Err(error) => warn!(%error, "failed to parse metaverse room event hint"),
                                }
                            }
                            GossipHint::DomeHostHeartbeat { instance_id, heartbeat, .. } => {
                                if dome_host_heartbeats.lock().await.record(
                                    &context,
                                    instance_id,
                                    heartbeat.as_ref().clone(),
                                    now,
                                ) {
                                    last_sync.set(now).await;
                                }
                            }
                            GossipHint::LivePresence { session_id, author, ttl_ms, .. } => {
                                let projection_store = &reader.services.projection_store;
                                let _ = projection_store
                                    .upsert_live_presence(
                                        topic.as_str(),
                                        storage_channel_id.as_str(),
                                        session_id.as_str(),
                                        author.as_str(),
                                        now + i64::from(*ttl_ms),
                                        now,
                                    )
                                    .await;
                                let _ = projection_store.clear_expired_live_presence(now).await;
                                last_sync.set(now).await;
                            }
                            hint => {
                                let applied = reader.apply_content_hint(&topic, &scope, hint).await;
                                if applied > 0 {
                                    last_sync.set(now).await;
                                    if let Some(generation) = generation {
                                        record_public_topic_docs_activity_if_current(
                                            &public_topic_delivery,
                                            topic.as_str(),
                                            generation,
                                            now,
                                        )
                                        .await;
                                        // topic の届き方(docs の活動)が変わった(#1221 R2-D)。
                                        last_sync.mark(StatusKey::Topic(format!(
                                            "{}{topic}",
                                            kukuri_core::wire::HINT_TOPIC_PREFIX
                                        )));
                                    }
                                }
                            }
                        }
                    }
                    _ = window => {
                        window_end = n0_future::time::Instant::now() + HINT_WINDOW;
                        window_used = 0;
                        let overflowed = std::mem::take(&mut dropped);
                        if overflowed > 0 {
                            debug!(topic = %topic, dropped = overflowed, "hint window overflowed; rereading the scope");
                            reread_and_mark(&reader, &topic, &scope, &last_sync).await;
                        }
                    }
                    _ = boundary => reread_and_mark(&reader, &topic, &scope, &last_sync).await,
                    // hint の購読を抜けるのは lease の持ち主(`stop_scope_task`)。
                    else => break,
                }
            }
        });
        Ok(ScopeTask {
            handle: AbortOnDropTask::new(handle),
            hint_topic: Some(hint_topic),
        })
    }

    /// author の lease: 開始時と日の境界で、制御領域の現在値の key を R5-C の有界な読みで反映し、現在と直前の
    /// author bucket の索引を 1 ページ読み直して手元へ置く(R5-H)。
    pub(crate) async fn spawn_author_subscription(&self, author_pubkey: &str) -> Result<ScopeTask> {
        let services = self.services.clone();
        let last_sync = Arc::clone(&self.last_sync_ts);
        let author = normalize_author_pubkey(author_pubkey)?;
        let local_author = self.current_author_pubkey();
        let handle = n0_future::task::spawn(async move {
            loop {
                match hydrate_author_state(
                    &services,
                    local_author.as_str(),
                    author.as_str(),
                    DocFetchPolicy::LocalThenRemote,
                )
                .await
                {
                    Ok(count) if count > 0 => {
                        last_sync.set(Utc::now().timestamp_millis()).await;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(author_pubkey = %author, %error, "failed to hydrate author state")
                    }
                }
                match super::profile_timeline_support::reread_author_buckets(&services, &author)
                    .await
                {
                    Ok(placed) if placed > 0 => {
                        last_sync.set(Utc::now().timestamp_millis()).await;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(author_pubkey = %author, %error, "failed to reread author buckets")
                    }
                }
                n0_future::time::sleep(until_next_bucket(Utc::now().timestamp())).await;
            }
        });
        Ok(ScopeTask {
            handle: AbortOnDropTask::new(handle),
            hint_topic: None,
        })
    }

    /// 本人の端末間の account 同期を始める（ADR 0061）。scope の lease で hint を購読する。
    /// 購読は account の runtime と一緒に止まる（停止・切替で runtime を作り直す）。
    pub async fn start_account_sync(&self) -> Result<()> {
        let hint_topic = self
            .services
            .keys
            .derive_account_sync()
            .hint_topic()
            .clone();
        self.set_scope_holder(
            ACCOUNT_SYNC_HOLDER,
            [ScopeKey::AccountSync(hint_topic.as_str().to_string())],
        )
        .await
    }

    /// replica の namespace の秘密を登録し、hint の購読を持つ。endpoint の作り直しで docs が新しくなっても、
    /// task の作り直しで登録し直す。差分の取得の契機（ADR 0061 §10）: task の開始（lease の開始・endpoint の世代の
    /// 変化）と日の境界、hint、書込みの後の送り直しと hint の送信の要求、rendezvous の応答に新しく現れた本人の端末。
    pub(crate) async fn spawn_account_sync_subscription(
        &self,
        hint_topic: TopicId,
    ) -> Result<ScopeTask> {
        let keys = self.services.keys.derive_account_sync();
        self.services
            .docs_sync
            .register_private_replica_secret(
                keys.replica_id(),
                keys.expose_namespace_secret_hex().as_str(),
            )
            .await?;
        // 購読で namespace を作る。まだ何も書いていない端末（移行の直後の移行先など）も、本人の別の端末の読取りに
        // 空で答える（namespace が無いと読取りを打ち切り、相手は取得の失敗のままになる。#1220 AC-3b）。
        self.services
            .docs_sync
            .open_replica(keys.replica_id())
            .await?;
        let hints = self
            .services
            .hint_transport
            .subscribe_hints(&hint_topic)
            .await?;
        // 作り直し（起動・復帰）の前の rendezvous の応答を忘れる（task を起こす前に。直後の応答の知らせと入れ替わら
        // ないように）。
        self.forget_account_sync_rendezvous();
        // merge は account の状態を共有する handle で行う（届いた channel の参加・世代の変化を、この account の購読と
        // メモリへ反映する。#1211 AC-4）。
        let handle = n0_future::task::spawn(account_sync_task(self.account_handle(), hints));
        Ok(ScopeTask {
            handle: AbortOnDropTask::new(handle),
            hint_topic: Some(hint_topic),
        })
    }

    /// 現在と直前の bucket(と旧形式の保存済みデータ)を 1 ページだけ読み直す。live・game の session も含める。
    /// 戻り値は反映した件数。
    pub(crate) async fn reread_scope(&self, topic_id: &str, scope: &TimelineScope) -> usize {
        let mut applied = 0;
        match self
            .reconcile_timeline_range_checked(
                topic_id,
                scope,
                None,
                REPLICA_WINDOW_ENTRIES,
                false,
                false,
            )
            .await
        {
            Ok(reconcile) => applied += reconcile.hydrated,
            Err(error) => warn!(topic = %topic_id, %error, "failed to reread the current buckets"),
        }
        match self.reread_sessions(topic_id, scope).await {
            Ok(count) => applied += count,
            Err(error) => warn!(topic = %topic_id, %error, "failed to reread the current sessions"),
        }
        applied
    }

    /// 現在と直前の bucket(と旧形式)の session の索引を、provider から種類ごとに `SESSION_REREAD_LIMIT` 件まで読み、
    /// hint を受けたときと同じ読取りで反映する。途中から参加した端末も、既存の session を hint を待たずに表示できる。
    /// 戻り値は反映した件数。
    async fn reread_sessions(&self, topic_id: &str, scope: &TimelineScope) -> Result<usize> {
        let channel = match scope {
            TimelineScope::Public => None,
            TimelineScope::Channel { channel_id } => Some(channel_id.as_str()),
        };
        let mut sessions = BTreeSet::new();
        for (replica, epoch) in self
            .remote_page_replicas(topic_id, scope, None, false, false)
            .await?
        {
            let readers = self
                .remote_post_readers(
                    topic_id,
                    channel,
                    &replica,
                    epoch
                        .as_ref()
                        .map(|(id, secret)| (id.as_str(), secret.as_str())),
                )
                .await?;
            for (prefix, kind) in [
                ("sessions/live/", "live-session"),
                ("sessions/game/", "game-session"),
            ] {
                for reader in &readers {
                    let page = n0_future::time::timeout(
                        super::remote_read_support::REMOTE_READ_DEADLINE,
                        reader.query_replica_keys(
                            &replica,
                            DocKeyQuery {
                                prefix: prefix.to_string(),
                                order: DocKeyOrder::Descending,
                                limit: SESSION_REREAD_LIMIT,
                            },
                        ),
                    )
                    .await;
                    let Ok(Ok(page)) = page else {
                        continue;
                    };
                    // 日が変わった更新・移した session は、その日の bucket に locator がある(#1221 R5-H)。
                    sessions.extend(page.entries.iter().filter_map(|entry| {
                        let rest = entry.key.strip_prefix(prefix)?;
                        let id = rest
                            .strip_suffix("/state")
                            .or_else(|| rest.strip_suffix("/locator"))?;
                        (!id.is_empty() && !id.contains('/')).then(|| (id.to_string(), kind))
                    }));
                }
            }
        }
        let mut applied = 0;
        for (session_id, kind) in sessions {
            match self.read_session(topic_id, scope, &session_id, kind).await {
                Ok(true) => applied += 1,
                Ok(false) => {}
                Err(error) => warn!(%error, "failed to reread a session"),
            }
        }
        Ok(applied)
    }

    /// session 1 件を、操作と同じ lock の下で読み、projection より新しい版だけを一覧へ反映する。
    pub(crate) async fn read_session(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        session_id: &str,
        object_kind: &str,
    ) -> Result<bool> {
        let projection_store = &self.services.projection_store;
        let channel = match scope {
            TimelineScope::Public => PUBLIC_CHANNEL_ID,
            TimelineScope::Channel { channel_id } => channel_id.as_str(),
        };
        match object_kind {
            "live-session" => {
                let _lock = self
                    .services
                    .live_session_projections
                    .lock(session_id)
                    .await;
                match self
                    .fetch_verified_live_session(topic_id, channel, session_id, true)
                    .await?
                {
                    Some(verified) => projection_store
                        .upsert_live_session_cache(live_projection_row(&verified))
                        .await
                        .map(|()| true),
                    None => Ok(false),
                }
            }
            "game-session" => {
                let _lock = self.services.game_room_projections.lock(session_id).await;
                match self
                    .fetch_verified_game_room(topic_id, channel, session_id, true)
                    .await?
                {
                    Some(verified) => projection_store
                        .upsert_game_room_cache(game_projection_row(&verified))
                        .await
                        .map(|()| true),
                    None => Ok(false),
                }
            }
            // 別の端末の Dome の接続の記録を、知っている Dome の anchor から有界に読む(#1221 R5-H)。
            "dome-topology" => {
                let context = scope_context(topic_id, scope);
                let legacy = self.dome_connection_read_replica(&context).await?;
                let stores = self.dome_connection_stores(&context, legacy, &[]).await?;
                self.hydrate_dome_connection_records(&context, &stores)
                    .await;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// hint が指す対象だけを読む。戻り値は反映した件数。
    pub(crate) async fn apply_content_hint(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        hint: &GossipHint,
    ) -> usize {
        let mut applied = 0;
        let objects: Vec<(EnvelopeId, &str)> = match hint {
            GossipHint::TopicObjectsChanged { objects, .. } => objects
                .iter()
                .map(|object| {
                    (
                        EnvelopeId::from(object.object_id.as_str()),
                        object.object_kind.as_str(),
                    )
                })
                .collect(),
            GossipHint::ThreadUpdated { object_ids, .. } => {
                object_ids.iter().map(|id| (id.clone(), "post")).collect()
            }
            GossipHint::SessionChanged {
                session_id,
                object_kind,
                ..
            } => {
                return self
                    .read_session(topic_id, scope, session_id, object_kind)
                    .await
                    .inspect_err(|error| warn!(%error, "failed to read a hinted session"))
                    .unwrap_or_default() as usize;
            }
            _ => Vec::new(),
        };
        for (object_id, kind) in objects {
            let read = match kind {
                "post_withdrawal" => self.read_hinted_withdrawal(topic_id, &object_id).await,
                "reaction" => {
                    self.read_hinted_reactions(topic_id, scope, &object_id)
                        .await
                }
                _ => self
                    .ensure_object_projection(
                        topic_id,
                        scope,
                        &object_id,
                        DocFetchPolicy::LocalThenRemote,
                    )
                    .await
                    .map(usize::from),
            };
            match read {
                Ok(count) => applied += count,
                Err(error) => {
                    warn!(%error, object_id = %object_id.as_str(), "failed to read a hinted object")
                }
            }
        }
        applied
    }

    /// 取り下げは元投稿の位置に置かれる。手元に元投稿が無い取り下げは読まない。
    async fn read_hinted_withdrawal(
        &self,
        topic_id: &str,
        object_id: &EnvelopeId,
    ) -> Result<usize> {
        let Some(target) = self
            .services
            .projection_store
            .get_object_projection(object_id)
            .await?
        else {
            return Ok(0);
        };
        let channel = target.channel_id.as_str();
        let epoch = match channel {
            PUBLIC_CHANNEL_ID => None,
            _ => {
                let Some(epoch) = self
                    .private_epoch_for_source(topic_id, channel, &target.source_replica_id)
                    .await?
                else {
                    return Ok(0);
                };
                Some(epoch)
            }
        };
        for reader in self
            .remote_post_readers(
                topic_id,
                (channel != PUBLIC_CHANNEL_ID).then_some(channel),
                &target.source_replica_id,
                epoch
                    .as_ref()
                    .map(|(id, secret)| (id.as_str(), secret.as_str())),
            )
            .await?
        {
            let read = n0_future::time::timeout(
                super::remote_read_support::REMOTE_READ_DEADLINE,
                hydrate_post_withdrawal_for_object(
                    reader.as_ref(),
                    self.services.projection_store.as_ref(),
                    &target.source_replica_id,
                    object_id,
                    DocFetchPolicy::LocalThenRemote,
                ),
            )
            .await;
            if let Ok(Ok(Some(outcome))) = read
                && outcome.applied()
            {
                return Ok(1);
            }
        }
        Ok(0)
    }

    /// reaction は作成時の bucket に置かれる。現在と直前の bucket(と旧形式)を対象ごとに上限つきで読む。
    async fn read_hinted_reactions(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        target: &EnvelopeId,
    ) -> Result<usize> {
        if self
            .services
            .projection_store
            .get_object_projection(target)
            .await?
            .is_none()
        {
            return Ok(0);
        }
        let mut applied = 0;
        for (replica, epoch) in self
            .remote_page_replicas(topic_id, scope, None, false, false)
            .await?
        {
            let channel = match scope {
                TimelineScope::Public => None,
                TimelineScope::Channel { channel_id } => Some(channel_id.as_str()),
            };
            for reader in self
                .remote_post_readers(
                    topic_id,
                    channel,
                    &replica,
                    epoch
                        .as_ref()
                        .map(|(id, secret)| (id.as_str(), secret.as_str())),
                )
                .await?
            {
                let read = n0_future::time::timeout(
                    super::remote_read_support::REMOTE_READ_DEADLINE,
                    hydrate_reaction_cache_for_target_bounded(
                        reader.as_ref(),
                        self.services.projection_store.as_ref(),
                        self.services.blob_service.as_ref(),
                        topic_id,
                        &replica,
                        target,
                        DocFetchPolicy::LocalThenRemote,
                        RANGE_CHECK_REACTIONS_PER_OBJECT,
                    ),
                )
                .await;
                if let Ok(Ok(count)) = read {
                    applied += count;
                }
            }
        }
        Ok(applied)
    }
}

/// 操作(終了・参加・更新)が使う state と manifest(検証した版を分解する)。
impl AppService {
    /// 操作(終了・参加・更新)が使う state と manifest。docs から反映するときと同じ検証を通す(#1252)。
    pub(crate) async fn fetch_live_session_state_and_manifest(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<(ReplicaId, LiveSessionStateDocV1, LiveSessionManifestBlobV1)>> {
        Ok(self
            .fetch_verified_live_session(topic_id, PUBLIC_CHANNEL_ID, session_id, false)
            .await?
            .map(VerifiedLiveSession::into_parts))
    }

    /// `fetch_live_session_state_and_manifest` と同じ形。
    pub(crate) async fn fetch_game_room_state_and_manifest(
        &self,
        topic_id: &str,
        room_id: &str,
    ) -> Result<Option<(ReplicaId, GameRoomStateDocV1, GameRoomManifestBlobV1)>> {
        Ok(self
            .fetch_verified_game_room(topic_id, PUBLIC_CHANNEL_ID, room_id, false)
            .await?
            .map(VerifiedGameRoom::into_parts))
    }
}

/// account 同期の lease の task の本体。取得の merge が channel の lease を取り、lease が task を作るので、型を
/// `Send` の box に閉じて、task の作成の型が自分自身に戻らないようにする。
fn account_sync_task(
    reader: AppService,
    mut hints: kukuri_transport::HintStream,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        reader.catch_up_account_sync().await;
        loop {
            let boundary = n0_future::time::sleep(until_next_bucket(Utc::now().timestamp()));
            tokio::select! {
                Some(event) = hints.next() => {
                    if let GossipHint::AccountSyncChanged { device_id, .. } = &event.hint {
                        reader
                            .fetch_account_sync_for_hint(device_id, &event.source_peer)
                            .await;
                    }
                }
                _ = reader.services.account_sync.changed.notified() => {
                    if let Err(error) = reader.resend_account_sync_items().await {
                        warn!(%error, "account sync resend stopped; it resumes at the next trigger");
                    }
                    reader.publish_account_sync_hint().await;
                }
                _ = reader.services.account_sync.peers.notified() => {
                    reader.fetch_account_sync_from_appeared().await;
                }
                _ = boundary => reader.catch_up_account_sync().await,
                else => break,
            }
        }
    })
}
