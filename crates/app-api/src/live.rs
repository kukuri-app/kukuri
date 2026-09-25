use crate::service::*;

impl AppService {
    pub async fn list_live_sessions(&self, topic_id: &str) -> Result<Vec<LiveSessionView>> {
        self.list_live_sessions_scoped(topic_id, TimelineScope::Public)
            .await
    }

    pub async fn list_live_sessions_scoped(
        &self,
        topic_id: &str,
        scope: TimelineScope,
    ) -> Result<Vec<LiveSessionView>> {
        self.ensure_scope_subscriptions(topic_id, &scope).await?;
        let hidden_author_pubkeys = self.current_hidden_author_pubkeys().await?;
        self.services
            .projection_store
            .clear_expired_live_presence(Utc::now().timestamp_millis())
            .await?;
        let channel_id = self.allowed_channel_id_for_scope(topic_id, &scope).await?;
        let projection_store = Arc::clone(&self.services.projection_store);
        let topic_key = topic_id.to_string();
        let channel_key = channel_id.clone();
        let hidden_author_pubkeys = Arc::new(hidden_author_pubkeys);
        let rows = load_projection_rows_with_one_refresh(
            move || {
                let projection_store = Arc::clone(&projection_store);
                let topic_key = topic_key.clone();
                let channel_key = channel_key.clone();
                let hidden_author_pubkeys = Arc::clone(&hidden_author_pubkeys);
                async move {
                    Ok(projection_store
                        .list_channel_live_sessions(
                            topic_key.as_str(),
                            channel_key.as_str(),
                            LIVE_GAME_LIST_LIMIT,
                        )
                        .await?
                        .into_iter()
                        .filter(|row| !hidden_author_pubkeys.contains(row.host_pubkey.as_str()))
                        .collect())
                }
            },
            |rows| rows.is_empty(),
            || async {
                self.maybe_restart_scope_subscription(topic_id, &scope)
                    .await;
                self.maybe_restart_scope_replica_sync(topic_id, &scope)
                    .await;
                // #1239: replica を走査しない。session の固定件数だけを、key の一覧から反映する。
                self.catch_up_scope_sessions(topic_id, &scope).await?;
                self.services
                    .projection_store
                    .clear_expired_live_presence(Utc::now().timestamp_millis())
                    .await
            },
        )
        .await?;
        self.cleanup_ended_live_presence_tasks(&rows).await;
        let joined_sessions = self.subscription_registry.live_presence_tasks.lock().await;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(LiveSessionView {
                session_id: row.session_id.clone(),
                host_pubkey: row.host_pubkey,
                title: row.title,
                description: row.description,
                status: row.status,
                started_at: row.started_at,
                ended_at: row.ended_at,
                viewer_count: row.viewer_count,
                joined_by_me: joined_sessions.contains_key(
                    live_presence_task_key(
                        topic_id,
                        row.channel_id.as_str(),
                        row.session_id.as_str(),
                    )
                    .as_str(),
                ),
                channel_id: channel_id_for_view(row.channel_id.as_str()),
                audience_label: self
                    .audience_label_for_storage(topic_id, row.channel_id.as_str())
                    .await,
            });
        }
        Ok(items)
    }

    pub async fn create_live_session(
        &self,
        topic_id: &str,
        input: CreateLiveSessionInput,
    ) -> Result<String> {
        self.create_live_session_in_channel(topic_id, ChannelRef::Public, input)
            .await
    }

    pub async fn create_live_session_in_channel(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        input: CreateLiveSessionInput,
    ) -> Result<String> {
        self.ensure_topic_subscription(topic_id).await?;
        let now = Utc::now().timestamp_millis();
        let title = input.title.trim();
        if title.is_empty() {
            anyhow::bail!("live session title is required");
        }
        let private_state = match channel_ref {
            ChannelRef::Public => None,
            ChannelRef::PrivateChannel { channel_id } => Some(
                self.private_channel_write_state(topic_id, &channel_id)
                    .await?,
            ),
        };
        let channel_id = private_state.as_ref().map(|state| state.channel_id.clone());
        let source_replica_id = private_state
            .as_ref()
            .map(current_private_channel_replica_id)
            .unwrap_or_else(|| topic_replica_id(topic_id));
        let session_id = format!(
            "live-{}-{}",
            now,
            owner_bound_id_suffix(self.current_author_pubkey().as_str())
        );
        let topic = TopicId::new(topic_id);
        let manifest = LiveSessionManifestBlobV1 {
            session_id: session_id.clone(),
            revision: 1,
            topic_id: topic.clone(),
            channel_id: channel_id.clone(),
            owner_pubkey: Pubkey::from(self.current_author_pubkey()),
            title: title.to_string(),
            description: input.description.trim().to_string(),
            status: LiveSessionStatus::Live,
            started_at: now,
            ended_at: None,
        };
        let state = self
            .persist_live_session_manifest(&source_replica_id, topic_id, manifest.clone(), now)
            .await?;
        self.services
            .projection_store
            .upsert_live_session_cache(live_projection_row(&state))
            .await?;
        self.services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, channel_id.as_ref()),
                GossipHint::SessionChanged {
                    topic_id: topic.clone(),
                    session_id: session_id.clone(),
                    object_kind: "live-session".into(),
                },
            )
            .await?;
        *self.last_sync_ts.lock().await = Some(now);
        Ok(session_id)
    }

    pub async fn end_live_session(&self, topic_id: &str, session_id: &str) -> Result<()> {
        let _projection = self
            .services
            .live_session_projections
            .lock(session_id)
            .await;
        self.ensure_topic_subscription(topic_id).await?;
        let (source_replica_id, state, mut manifest) = self
            .fetch_live_session_state_and_manifest(topic_id, session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("live session not found"))?;
        let owner = self.current_author_pubkey();
        if state.owner_pubkey.as_str() != owner {
            anyhow::bail!("only the live session owner can end the session");
        }
        let channel_key = channel_storage_id(state.channel_id.as_ref());
        let hint_topic = channel_hint_topic_for(topic_id, state.channel_id.as_ref());
        if manifest.status == LiveSessionStatus::Ended {
            self.stop_live_presence_task(topic_id, channel_key.as_str(), session_id)
                .await;
            return Ok(());
        }
        let now = Utc::now().timestamp_millis();
        manifest.revision = manifest
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("live session revision overflow"))?;
        manifest.status = LiveSessionStatus::Ended;
        manifest.ended_at = Some(now);
        let state = self
            .persist_live_session_manifest(
                &source_replica_id,
                topic_id,
                manifest.clone(),
                state.created_at,
            )
            .await?;
        self.services
            .projection_store
            .upsert_live_session_cache(live_projection_row(&state))
            .await?;
        self.stop_live_presence_task(topic_id, channel_key.as_str(), session_id)
            .await;
        self.services
            .hint_transport
            .publish_hint(
                &hint_topic,
                GossipHint::SessionChanged {
                    topic_id: TopicId::new(topic_id),
                    session_id: session_id.to_string(),
                    object_kind: "live-session".into(),
                },
            )
            .await?;
        *self.last_sync_ts.lock().await = Some(now);
        Ok(())
    }

    pub async fn join_live_session(&self, topic_id: &str, session_id: &str) -> Result<()> {
        self.ensure_topic_subscription(topic_id).await?;
        let Some((_, state, manifest)) = self
            .fetch_live_session_state_and_manifest(topic_id, session_id)
            .await?
        else {
            anyhow::bail!("live session not found");
        };
        if manifest.status == LiveSessionStatus::Ended {
            anyhow::bail!("cannot join an ended live session");
        }
        let channel_key = channel_storage_id(state.channel_id.as_ref());
        let task_key = live_presence_task_key(topic_id, channel_key.as_str(), session_id);
        if self
            .subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .contains_key(task_key.as_str())
        {
            return Ok(());
        }
        self.apply_live_presence(topic_id, state.channel_id.as_ref(), session_id, 30_000)
            .await?;
        let hint_transport = Arc::clone(&self.services.hint_transport);
        let projection_store = Arc::clone(&self.services.projection_store);
        let live_presence_tasks = Arc::clone(&self.subscription_registry.live_presence_tasks);
        let hint_topic = channel_hint_topic_for(topic_id, state.channel_id.as_ref());
        let topic_key = topic_id.to_string();
        let channel_key_for_task = channel_key.clone();
        let session_key = session_id.to_string();
        let task_key_for_task = task_key.clone();
        let author = Pubkey::from(self.current_author_pubkey());
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                if matches!(
                    projection_store
                        .get_live_session(topic_key.as_str(), session_key.as_str())
                        .await,
                    Ok(Some(row)) if row.status == LiveSessionStatus::Ended
                ) {
                    live_presence_tasks
                        .lock()
                        .await
                        .remove(task_key_for_task.as_str());
                    return;
                }
                let now = Utc::now().timestamp_millis();
                let _ = projection_store
                    .upsert_live_presence(
                        topic_key.as_str(),
                        channel_key_for_task.as_str(),
                        session_key.as_str(),
                        author.as_str(),
                        now + 30_000,
                        now,
                    )
                    .await;
                let _ = hint_transport
                    .publish_hint(
                        &hint_topic,
                        GossipHint::LivePresence {
                            topic_id: TopicId::new(topic_key.clone()),
                            session_id: session_key.clone(),
                            author: author.clone(),
                            ttl_ms: 30_000,
                        },
                    )
                    .await;
            }
        });
        self.subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .insert(task_key, handle);
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        Ok(())
    }

    pub async fn leave_live_session(&self, topic_id: &str, session_id: &str) -> Result<()> {
        self.ensure_topic_subscription(topic_id).await?;
        let (_, state, _) = self
            .fetch_live_session_state_and_manifest(topic_id, session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("live session not found"))?;
        let channel_key = channel_storage_id(state.channel_id.as_ref());
        self.stop_live_presence_task(topic_id, channel_key.as_str(), session_id)
            .await;
        self.apply_live_presence(topic_id, state.channel_id.as_ref(), session_id, 0)
            .await?;
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        Ok(())
    }
}
