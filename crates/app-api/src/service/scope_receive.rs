//! #1221 R5-H: 旧 sync なしの受信。lease の task は docs の replica を開かず購読もしない。
//!
//! - gossip hint を受けた対象だけを exact に読む(手元 → 有界な provider)。
//! - lease の開始(task の作成)・endpoint の世代の変化(task の作り直し)・日の境界で、現在と直前の bucket を
//!   1 ページ(最大 200 行)だけ読み直す。続きを全件読みに行かない。
//! - content hint(短命種以外)の受付は窓あたり `HINT_WINDOW_BUDGET` 件まで。超えた分は読まずに捨て、窓の終わりに同じ
//!   読み直しを 1 回行う(#1567 AC-1)。topic の活動量が増えても、1 つの lease が読む量は窓あたりの定数に収まる。
//! - session の hint で読めなかった session は、読み先(hint の topic の近隣から順に回して選ぶ)にまだ持つ端末が
//!   無かったとみなし、間を置いて上限つきで読み直す(#1813)。読み先は 1 回の読みごとに次の組へ回るので、読み直しは
//!   前の読みが外した近隣にも聞く。先に読めた近隣も、読んだ版を提供する。
//! - 取り込んだ投稿から、従来どおり通知を作る(`ServiceHandles::notify_remote_posts`)。

use super::replica_window::RANGE_CHECK_REACTIONS_PER_OBJECT;
use super::subscription_catch_up::REPLICA_WINDOW_ENTRIES;
use super::*;
use kukuri_docs_sync::{BUCKET_SECONDS_V1, DocKeyOrder, DocKeyQuery};
use std::sync::atomic::Ordering;
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

/// hint で読めなかった session を読み直すまでの間(前の読みから。n 回目の読み直しは n 番目の間の後)。#1813。
pub(crate) const SESSION_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(4),
    std::time::Duration::from_secs(8),
];
/// 読み直しを待たせる session の上限(lease の task ごと)。超えた分は、次の読み直し・次の hint で回収する。
pub(crate) const SESSION_RETRY_CAPACITY: usize = 16;

/// session の hint の読み直しの予定(#1813)。期限・済んだ読み直しの回数・hint。件数と回数に上限を持つ。
#[derive(Default)]
struct SessionRetries(Vec<(n0_future::time::Instant, usize, GossipHint)>);

/// session の hint が指す session(他の hint は `None`)。
fn hinted_session(hint: &GossipHint) -> Option<(&str, &str)> {
    match hint {
        GossipHint::SessionChanged {
            session_id,
            object_kind,
            ..
        } => Some((session_id, object_kind)),
        _ => None,
    }
}

impl SessionRetries {
    /// session の hint の読みの結果を受ける(`retried` は済んだ読み直しの回数。hint の時点の読みは 0)。読めなければ次の
    /// 読み直しを予定し、読めたら同じ session の予定を消す。予定は session ごとに 1 件まで(続く hint は足さない)で、
    /// 回数・件数の上限を超えたら予定しない。
    fn record(&mut self, hint: &GossipHint, retried: usize, read: bool) {
        let Some(session) = hinted_session(hint) else {
            return;
        };
        let pending = self
            .0
            .iter()
            .position(|(_, _, pending)| hinted_session(pending) == Some(session));
        match (read, pending) {
            (true, Some(index)) => {
                self.0.swap_remove(index);
            }
            (false, None) => {
                if let Some(delay) = SESSION_RETRY_DELAYS.get(retried)
                    && self.0.len() < SESSION_RETRY_CAPACITY
                {
                    let at = n0_future::time::Instant::now() + *delay;
                    self.0.push((at, retried + 1, hint.clone()));
                }
            }
            _ => {}
        }
    }

    fn next_at(&self) -> Option<n0_future::time::Instant> {
        self.0.iter().map(|(at, _, _)| *at).min()
    }

    /// 期限の来た読み直しを 1 件取り出す(済んだ読み直しの回数(これを含む)と hint)。
    fn take_due(&mut self) -> Option<(usize, GossipHint)> {
        let now = n0_future::time::Instant::now();
        let index = self.0.iter().position(|(at, _, _)| *at <= now)?;
        let (_, retried, hint) = self.0.swap_remove(index);
        Some((retried, hint))
    }
}

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

/// scope の Spatial Context(公開は topic、private channel は channel)。公開 channel には Dome を置けないので topic。
fn scope_context(topic_id: &str, scope: &TimelineScope) -> kukuri_core::SpatialContextV1 {
    let topic_id = TopicId::new(topic_id);
    match scope {
        TimelineScope::Public | TimelineScope::PublicChannel { .. } => {
            kukuri_core::SpatialContextV1::Topic { topic_id }
        }
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
        let scope = TimelineScope::for_channel(channel_id.as_ref());
        let context = scope_context(topic_id, &scope);
        let generation = if scope == TimelineScope::Public {
            let counter = &self.subscription_registry.generation;
            let generation = counter.fetch_add(1, Ordering::Relaxed);
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
        // 同じgossip購読のstreamを共用し、本文のremote読取りがPushの受信を待たせないようにする。
        // leaseを落とすと子taskも止まる。投稿/Pushごとのtask・購読は作らない。
        let overlay_task = if channel_id
            .as_ref()
            .is_some_and(ChannelId::is_public_channel)
        {
            let mut stream = self
                .services
                .hint_transport
                .subscribe_hints(&hint_topic)
                .await?;
            let overlay_reader = self.scope_reader();
            let topic = topic.clone();
            let channel = storage_channel_id.clone();
            Some(AbortOnDropTask::new(n0_future::task::spawn(async move {
                while let Some(event) = stream.next().await {
                    let bus = &overlay_reader.services.live_overlay_pushes;
                    if event.dropped_before > 0 {
                        bus.overflow();
                    }
                    if let GossipHint::MetaverseRoomEvent {
                        topic_id,
                        room_id,
                        event: envelope,
                    } = event.hint
                        && envelope.kind == crate::live_overlay::PUSH_KIND
                        && topic_id.as_str() == topic
                        && room_id == channel
                        && !overlay_reader
                            .overlay_author_hidden(envelope.pubkey.as_str())
                            .await
                            .unwrap_or(true)
                    {
                        let _ =
                            bus.receive(&topic, &channel, &envelope, Utc::now().timestamp_millis());
                    }
                }
            })))
        } else {
            None
        };
        let handle = n0_future::task::spawn(async move {
            let _overlay_task = overlay_task;
            reread_and_mark(&reader, &topic, &scope, &last_sync).await;
            // 窓あたりの content hint の受付(#1567 AC-1)。`dropped` は、この窓で読まずに捨てた hint の数(transport の
            // 購読 stream が取りこぼした分を含む)。1 件以上なら、何を落としたか分からないので窓の終わりに 1 回読み直す。
            let mut window_end = n0_future::time::Instant::now() + HINT_WINDOW;
            let mut window_used = 0usize;
            let mut dropped = 0u64;
            let mut session_retries = SessionRetries::default();
            loop {
                let boundary = n0_future::time::sleep(until_next_bucket(Utc::now().timestamp()));
                let window = n0_future::time::sleep_until(window_end);
                let retry = session_retries.next_at();
                // content hint とその読み直しで反映した件数。
                let mut applied = 0;
                tokio::select! {
                    Some(event) = hint_stream.next() => {
                        dropped = dropped.saturating_add(event.dropped_before);
                        if !hint_targets_topic(&event.hint, topic.as_str()) {
                            continue;
                        }
                        if matches!(&event.hint, GossipHint::MetaverseRoomEvent { event, .. } if event.kind == crate::live_overlay::PUSH_KIND) {
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
                                // 公開 channel(#1840)の Live の presence は topic の hint で届くので、Live の行の
                                // channel に結び付ける(private channel の Live は channel の task が受けたものだけ)。
                                // 読むのは topic の task で、session の channel だけ(視聴者は数えない)。
                                let thread = if storage_channel_id == PUBLIC_CHANNEL_ID {
                                    projection_store
                                        .live_session_channel(topic.as_str(), session_id.as_str())
                                        .await
                                        .ok()
                                        .flatten()
                                        .filter(|channel| ChannelId::new(channel.as_str()).is_public_channel())
                                } else {
                                    None
                                };
                                let channel = thread.unwrap_or_else(|| storage_channel_id.clone());
                                let _ = projection_store
                                    .upsert_live_presence(
                                        topic.as_str(),
                                        channel.as_str(),
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
                                applied = reader.apply_content_hint(&topic, &scope, hint).await;
                                session_retries.record(hint, 0, applied > 0);
                            }
                        }
                    }
                    _ = n0_future::time::sleep_until(retry.unwrap_or(window_end)), if retry.is_some() => {
                        if let Some((retried, hint)) = session_retries.take_due() {
                            applied = reader.apply_content_hint(&topic, &scope, &hint).await;
                            session_retries.record(&hint, retried, applied > 0);
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
                if applied > 0 {
                    let now = Utc::now().timestamp_millis();
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
            // 公開 channel の session は topic の bucket にあり、同じ列が持つ topic の lease が読み直す(#1840)。
            TimelineScope::PublicChannel { .. } => return Ok(0),
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
        let channel = scope
            .channel_id()
            .map_or(PUBLIC_CHANNEL_ID, ChannelId::as_str);
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
            // #1840: 公開 channel の作成・変更・削除(#1888)の知らせ(topic の hint)。metadata を読んで、行が無いか、行より
            // 新しい版なら一覧の行に置く。
            kukuri_core::PUBLIC_CHANNEL_KIND
                if *scope == TimelineScope::Public
                    && ChannelId::new(session_id).is_public_channel() =>
            {
                Ok(self
                    .read_public_channel(topic_id, session_id)
                    .await?
                    .is_some())
            }
            // #1980: ゲームのアセットの新しい release の知らせ(公開 topic の hint。SDK を持つ native だけ)。
            #[cfg(not(target_family = "wasm"))]
            crate::GAME_ASSET_HINT_KIND if *scope == TimelineScope::Public => {
                self.receive_game_asset_hint(topic_id, session_id).await
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
        let epoch = match channel_id_from_storage(channel) {
            None => None,
            Some(channel_id) if channel_id.is_public_channel() => None,
            Some(_) => {
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
            let channel = scope.channel_id().map(ChannelId::as_str);
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
                        &self.services.reaction_changes,
                        topic_id,
                        &replica,
                        target,
                        DocFetchPolicy::LocalThenRemote,
                        RANGE_CHECK_REACTIONS_PER_OBJECT,
                    ),
                )
                .await;
                if let Ok(Ok(reactions)) = read {
                    applied += reactions.len();
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
        // 自分が書いた既知の状態は手元が正本。locator の遠隔照会を待って配信操作を止めない。
        if let Some(row) = self
            .services
            .projection_store
            .get_live_session(topic_id, session_id)
            .await?
            && row.host_pubkey == self.current_author_pubkey()
            && (is_public_storage_channel(&row.channel_id)
                || self
                    .ensure_private_channel_access(topic_id, &ChannelId::new(&row.channel_id))
                    .await
                    .is_ok())
            && let Ok(Some(local)) = load_verified_live_session(
                self.services.docs_sync.as_ref(),
                self.services.blob_service.as_ref(),
                &row.source_replica_id,
                topic_id,
                session_id,
                DocFetchPolicy::LocalOnly,
            )
            .await
            && local.revision() >= row.revision
        {
            return Ok(Some(local.into_parts()));
        }
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
