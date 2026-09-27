//! #1221 R5-H: 旧 sync なしの受信。lease の task は docs の replica を開かず購読もしない。
//!
//! - gossip hint を受けた対象だけを exact に読む(手元 → 有界な provider)。
//! - lease の開始(task の作成)・endpoint の世代の変化(task の作り直し)・日の境界で、現在と直前の bucket を
//!   1 ページ(最大 200 行)だけ読み直す。続きを全件読みに行かない。
//! - 取り込んだ投稿から、従来どおり通知を作る(`ServiceHandles::notify_remote_posts`)。

use super::replica_window::RANGE_CHECK_REACTIONS_PER_OBJECT;
use super::subscription_catch_up::REPLICA_WINDOW_ENTRIES;
use super::*;
use kukuri_docs_sync::{BUCKET_SECONDS_V1, DocKeyOrder, DocKeyQuery};

/// lease の読み直しで、replica ごとに種類ごとに読む session の索引の上限(判断 5)。
const SESSION_REREAD_LIMIT: usize = 64;

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
    fn scope_reader(&self) -> AppService {
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
        let handle = tokio::spawn(async move {
            reader.reread_scope(&topic, &scope).await;
            loop {
                let boundary = tokio::time::sleep(until_next_bucket(Utc::now().timestamp()));
                tokio::select! {
                    Some(event) = hint_stream.next() => {
                        if !hint_targets_topic(&event.hint, topic.as_str()) {
                            continue;
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
                                        *last_sync.lock().await = Some(now);
                                    }
                                    Ok(None) => {}
                                    Err(error) => warn!(%error, "failed to parse metaverse room event hint"),
                                }
                            }
                            GossipHint::DomeHostHeartbeat { instance_id, heartbeat, .. } => {
                                let mut heartbeats = dome_host_heartbeats.lock().await;
                                let replace = heartbeats.get(instance_id).is_none_or(|current| {
                                    heartbeat.heartbeat.sequence > current.heartbeat.sequence
                                        || (heartbeat.heartbeat.sequence == current.heartbeat.sequence
                                            && heartbeat.heartbeat.sent_at > current.heartbeat.sent_at)
                                });
                                if replace {
                                    heartbeats.insert(instance_id.clone(), heartbeat.as_ref().clone());
                                    *last_sync.lock().await = Some(now);
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
                                *last_sync.lock().await = Some(now);
                            }
                            hint => {
                                let applied = reader.apply_content_hint(&topic, &scope, hint).await;
                                if applied > 0 {
                                    *last_sync.lock().await = Some(now);
                                    if let Some(generation) = generation {
                                        record_public_topic_docs_activity_if_current(
                                            &public_topic_delivery,
                                            topic.as_str(),
                                            generation,
                                            now,
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                    _ = boundary => reader.reread_scope(&topic, &scope).await,
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

    /// author の lease: 開始時と日の境界で、制御領域の現在値の key と author bucket を R5-C の有界な読みで反映する。
    pub(crate) async fn spawn_author_subscription(&self, author_pubkey: &str) -> Result<ScopeTask> {
        let services = self.services.clone();
        let last_sync = Arc::clone(&self.last_sync_ts);
        let author = normalize_author_pubkey(author_pubkey)?;
        let local_author = self.current_author_pubkey();
        let handle = tokio::spawn(async move {
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
                        *last_sync.lock().await = Some(Utc::now().timestamp_millis());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(author_pubkey = %author, %error, "failed to hydrate author state")
                    }
                }
                tokio::time::sleep(until_next_bucket(Utc::now().timestamp())).await;
            }
        });
        Ok(ScopeTask {
            handle: AbortOnDropTask::new(handle),
            hint_topic: None,
        })
    }

    /// 現在と直前の bucket(と旧形式の保存済みデータ)を 1 ページだけ読み直す。live・game の session も含める。
    pub(crate) async fn reread_scope(&self, topic_id: &str, scope: &TimelineScope) {
        if let Err(error) = self
            .reconcile_timeline_range_checked(topic_id, scope, None, REPLICA_WINDOW_ENTRIES, false)
            .await
        {
            warn!(topic = %topic_id, %error, "failed to reread the current buckets");
        }
        if let Err(error) = self.reread_sessions(topic_id, scope).await {
            warn!(topic = %topic_id, %error, "failed to reread the current sessions");
        }
    }

    /// 現在と直前の bucket(と旧形式)の session の索引を、provider から種類ごとに `SESSION_REREAD_LIMIT` 件まで読み、
    /// hint を受けたときと同じ読取りで反映する。途中から参加した端末も、既存の session を hint を待たずに表示できる。
    async fn reread_sessions(&self, topic_id: &str, scope: &TimelineScope) -> Result<()> {
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
                    let page = tokio::time::timeout(
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
        for (session_id, kind) in sessions {
            if let Err(error) = self.read_session(topic_id, scope, &session_id, kind).await {
                warn!(%error, "failed to reread a session");
            }
        }
        Ok(())
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
                    .fetch_verified_live_session(topic_id, channel, session_id)
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
                    .fetch_verified_game_room(topic_id, channel, session_id)
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
                let context = match scope {
                    TimelineScope::Public => kukuri_core::SpatialContextV1::Topic {
                        topic_id: TopicId::new(topic_id),
                    },
                    TimelineScope::Channel { channel_id } => {
                        kukuri_core::SpatialContextV1::Channel {
                            topic_id: TopicId::new(topic_id),
                            channel_id: channel_id.clone(),
                        }
                    }
                };
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
            let read = tokio::time::timeout(
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
                let read = tokio::time::timeout(
                    super::remote_read_support::REMOTE_READ_DEADLINE,
                    hydrate_reaction_cache_for_target_bounded(
                        reader.as_ref(),
                        self.services.projection_store.as_ref(),
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
            .fetch_verified_live_session(topic_id, PUBLIC_CHANNEL_ID, session_id)
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
            .fetch_verified_game_room(topic_id, PUBLIC_CHANNEL_ID, room_id)
            .await?
            .map(VerifiedGameRoom::into_parts))
    }
}
