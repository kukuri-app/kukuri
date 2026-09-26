//! #1221 R5-H: 旧 sync なしの受信。lease の task は docs の replica を開かず購読もしない。
//!
//! - gossip hint を受けた対象だけを exact に読む(手元 → 有界な provider)。
//! - lease の開始(task の作成)・endpoint の世代の変化(task の作り直し)・日の境界で、現在と直前の bucket を
//!   1 ページ(最大 200 行)だけ読み直す。続きを全件読みに行かない。
//! - 取り込んだ投稿から、従来どおり通知を作る(`ServiceHandles::notify_remote_posts`)。

use super::replica_window::RANGE_CHECK_REACTIONS_PER_OBJECT;
use super::subscription_catch_up::REPLICA_WINDOW_ENTRIES;
use super::*;
use kukuri_docs_sync::BUCKET_SECONDS_V1;

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

    /// 現在と直前の bucket(と旧形式の保存済みデータ)を 1 ページだけ読み直す。
    pub(crate) async fn reread_scope(&self, topic_id: &str, scope: &TimelineScope) {
        if let Err(error) = self
            .reconcile_timeline_range_checked(topic_id, scope, None, REPLICA_WINDOW_ENTRIES)
            .await
        {
            warn!(topic = %topic_id, %error, "failed to reread the current buckets");
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
                // 操作と同じ lock の下で読み、projection より新しい版だけを一覧へ反映する。
                let projection_store = &self.services.projection_store;
                let read = match object_kind.as_str() {
                    "live-session" => {
                        let _lock = self
                            .services
                            .live_session_projections
                            .lock(session_id)
                            .await;
                        match self.fetch_verified_live_session(topic_id, session_id).await {
                            Ok(Some(verified)) => projection_store
                                .upsert_live_session_cache(live_projection_row(&verified))
                                .await
                                .map(|()| true),
                            other => other.map(|_| false),
                        }
                    }
                    "game-session" => {
                        let _lock = self.services.game_room_projections.lock(session_id).await;
                        match self.fetch_verified_game_room(topic_id, session_id).await {
                            Ok(Some(verified)) => projection_store
                                .upsert_game_room_cache(game_projection_row(&verified))
                                .await
                                .map(|()| true),
                            other => other.map(|_| false),
                        }
                    }
                    _ => Ok(false),
                };
                return read
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
