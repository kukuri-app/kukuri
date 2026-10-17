use crate::service::*;
use kukuri_core::{LiveMediaSigner, LiveMediaVerifier, SpatialContextV1};

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
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(self.live_session_view(row).await);
        }
        Ok(items)
    }

    /// 1 件の Live を、作成した topic と id で引く(#1839。紹介 post から別の topic の Live を表示する)。手元の行だけを読む。
    /// まだ無ければ表示要求(`set_session_display`)で読む。限定 channel の Live は、その channel の参加者にだけ返す。
    pub async fn get_live_session(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<LiveSessionView>> {
        let Some(row) = self
            .services
            .projection_store
            .get_live_session(topic_id, session_id)
            .await?
        else {
            return Ok(None);
        };
        if !is_public_storage_channel(&row.channel_id)
            && self
                .ensure_private_channel_access(topic_id, &ChannelId::new(row.channel_id.as_str()))
                .await
                .is_err()
        {
            return Ok(None);
        }
        if self
            .current_hidden_author_pubkeys()
            .await?
            .contains(row.host_pubkey.as_str())
        {
            return Ok(None);
        }
        Ok(Some(self.live_session_view(row).await))
    }

    async fn live_session_view(&self, row: LiveSessionProjectionRow) -> LiveSessionView {
        let joined_by_me = self
            .subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .contains_key(
                live_presence_task_key(
                    row.topic_id.as_str(),
                    row.channel_id.as_str(),
                    row.session_id.as_str(),
                )
                .as_str(),
            );
        LiveSessionView {
            audience_label: self
                .audience_label_for_storage(row.topic_id.as_str(), row.channel_id.as_str())
                .await,
            channel_id: channel_id_for_view(row.channel_id.as_str()),
            session_id: row.session_id,
            host_pubkey: row.host_pubkey,
            title: row.title,
            description: row.description,
            status: row.status,
            started_at: row.started_at,
            ended_at: row.ended_at,
            viewer_count: row.viewer_count,
            joined_by_me,
            start_post_id: row.start_post_id.map(|id| id.0),
        }
    }

    pub async fn create_live_session(
        &self,
        topic_id: &str,
        input: CreateLiveSessionInput,
    ) -> Result<String> {
        self.create_live_session_in_channel(topic_id, ChannelRef::Public, input, None)
            .await
    }

    /// `media_endpoint_id` は映像・音声を送るこの端末の endpoint id(ADR 0067 §8)。無ければ media の無い session。
    /// media のある session には、frame の署名を確かめる鍵(owner の鍵から session ごとに導く)も載せる。
    pub async fn create_live_session_in_channel(
        &self,
        topic_id: &str,
        channel_ref: ChannelRef,
        input: CreateLiveSessionInput,
        media_endpoint_id: Option<String>,
    ) -> Result<String> {
        let now = Utc::now().timestamp_millis();
        let title = input.title.trim();
        if title.is_empty() {
            anyhow::bail!("live session title is required");
        }
        // 公開 channel(#1840)の Live は、topic の公開の Live と同じく topic の bucket に置き、manifest の channel が公開 channel
        // を指す。
        let private_state = match &channel_ref {
            ChannelRef::Public | ChannelRef::PublicChannel { .. } => None,
            ChannelRef::PrivateChannel { channel_id } => Some(
                self.private_channel_write_state(topic_id, channel_id)
                    .await?,
            ),
        };
        let channel_id = channel_ref.channel_id().cloned();
        // 切替後は作成時の bucket に entity の最新 state を置く(id の時刻から読み手が bucket を決める)。
        let source_replica_id =
            self.services
                .scope_write_replica(topic_id, private_state.as_ref(), now / 1_000)?;
        let session_id = format!(
            "live-{}-{}",
            now,
            owner_bound_id_suffix(self.current_author_pubkey().as_str())
        );
        let topic = TopicId::new(topic_id);
        // #1839: 開始 post を先に置き、その id を manifest に入れる(post は Live の id だけを参照するので循環しない)。
        let start_post_id = self
            .create_post_object(
                topic_id,
                channel_ref,
                input.start_post.as_str(),
                None,
                Vec::new(),
                Vec::new(),
                Some(LiveRefV1 {
                    session_id: session_id.clone(),
                    topic_id: topic.clone(),
                    channel_id: channel_id.clone(),
                }),
            )
            .await?;
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
            media_signing_key: media_endpoint_id
                .as_ref()
                .map(|_| self.live_media_signer(&session_id))
                .transpose()?
                .map(|signer| signer.public_key_hex()),
            media_endpoint_id,
            start_post_id: Some(EnvelopeId::from(start_post_id)),
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
                &session_hint_topic_for(topic_id, channel_id.as_ref()),
                GossipHint::SessionChanged {
                    topic_id: topic.clone(),
                    session_id: session_id.clone(),
                    object_kind: "live-session".into(),
                    sent_at: Some(Utc::now().timestamp_millis()),
                },
            )
            .await?;
        self.last_sync_ts.set(now).await;
        Ok(session_id)
    }

    pub async fn end_live_session(&self, topic_id: &str, session_id: &str) -> Result<()> {
        let _projection = self
            .services
            .live_session_projections
            .lock(session_id)
            .await;
        let (source_replica_id, state, mut manifest) = self
            .fetch_live_session_state_and_manifest(topic_id, session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("live session not found"))?;
        let owner = self.current_author_pubkey();
        if state.owner_pubkey.as_str() != owner {
            anyhow::bail!("only the live session owner can end the session");
        }
        let channel_key = channel_storage_id(state.channel_id.as_ref());
        let hint_topic = session_hint_topic_for(topic_id, state.channel_id.as_ref());
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
                    sent_at: Some(Utc::now().timestamp_millis()),
                },
            )
            .await?;
        self.last_sync_ts.set(now).await;
        Ok(())
    }

    pub async fn join_live_session(&self, topic_id: &str, session_id: &str) -> Result<()> {
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
        // 参加は topic(channel なら channel も)を参加の holder で取る。上限なら参加しない(#1221 R2-C)。
        let holder = live_holder(&task_key);
        self.set_scope_holder(
            &holder,
            participation_keys(topic_id, private_channel_of(state.channel_id.as_ref())),
        )
        .await?;
        if let Err(error) = self
            .apply_live_presence(topic_id, state.channel_id.as_ref(), session_id, 30_000)
            .await
        {
            self.release_scope_holder(&holder).await;
            return Err(error);
        }
        let services = self.services.clone();
        let scope_leases = Arc::clone(&self.subscription_registry.scope_leases);
        let hint_transport = Arc::clone(&self.services.hint_transport);
        let projection_store = Arc::clone(&self.services.projection_store);
        let live_presence_tasks = Arc::clone(&self.subscription_registry.live_presence_tasks);
        let hint_topic = session_hint_topic_for(topic_id, state.channel_id.as_ref());
        let topic_key = topic_id.to_string();
        let channel_key_for_task = channel_key.clone();
        let session_key = session_id.to_string();
        let task_key_for_task = task_key.clone();
        let author = Pubkey::from(self.current_author_pubkey());
        let handle = n0_future::task::spawn(async move {
            let mut interval = n0_future::time::interval(std::time::Duration::from_secs(10));
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
                    release_scope_holder(&services, &scope_leases, &holder).await;
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
        self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
        Ok(())
    }

    pub async fn leave_live_session(&self, topic_id: &str, session_id: &str) -> Result<()> {
        let (_, state, _) = self
            .fetch_live_session_state_and_manifest(topic_id, session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("live session not found"))?;
        let channel_key = channel_storage_id(state.channel_id.as_ref());
        self.stop_live_presence_task(topic_id, channel_key.as_str(), session_id)
            .await;
        self.apply_live_presence(topic_id, state.channel_id.as_ref(), session_id, 0)
            .await?;
        self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
        Ok(())
    }

    /// 配信者の端末で、自分の終わっていない session なら、視聴者の資格を判定する scope と owner(自分)と、映像・音声を
    /// 送る端末の id(署名を確かめる鍵の無い旧い session は持たない)を返す(ADR 0067 §3)。
    pub async fn live_media_scope(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<(SpatialContextV1, Pubkey, Option<String>)>> {
        let Some((_, state, manifest)) = self
            .fetch_live_session_state_and_manifest(topic_id, session_id)
            .await?
        else {
            return Ok(None);
        };
        if manifest.status == LiveSessionStatus::Ended
            || state.owner_pubkey.as_str() != self.current_author_pubkey()
        {
            return Ok(None);
        }
        Ok(Some((
            live_spatial_context(&manifest),
            manifest.owner_pubkey,
            manifest
                .media_signing_key
                .as_ref()
                .and(manifest.media_endpoint_id),
        )))
    }

    /// この端末が配信する session の frame の署名鍵(owner の鍵から導く。manifest の鍵と同じ)。
    pub fn live_media_signer(&self, session_id: &str) -> Result<LiveMediaSigner> {
        self.keys().derive_live_media_signer(session_id)
    }

    /// 視聴者の端末で、終わっていない session の配信者の端末と、frame の署名を確かめる鍵と、proof を束縛する scope・
    /// owner を返す。どちらかが無い session は media を持たない。
    pub async fn live_media_source(
        &self,
        topic_id: &str,
        session_id: &str,
    ) -> Result<Option<(String, LiveMediaVerifier, SpatialContextV1, Pubkey)>> {
        let Some((_, _, manifest)) = self
            .fetch_live_session_state_and_manifest(topic_id, session_id)
            .await?
        else {
            return Ok(None);
        };
        if manifest.status == LiveSessionStatus::Ended {
            return Ok(None);
        }
        let context = live_spatial_context(&manifest);
        let key = manifest
            .media_signing_key
            .as_deref()
            .and_then(LiveMediaVerifier::from_hex);
        Ok(manifest
            .media_endpoint_id
            .zip(key)
            .map(|(endpoint, key)| (endpoint, key, context, manifest.owner_pubkey)))
    }
}

/// live session の scope を、Dome と同じ資格の判定(公開 topic・private channel の current epoch)に渡す形にする。公開
/// channel(#1840)で始めた Live は公開の Live なので、topic の資格で視聴できる。
fn live_spatial_context(manifest: &LiveSessionManifestBlobV1) -> SpatialContextV1 {
    match private_channel_of(manifest.channel_id.as_ref()) {
        None => SpatialContextV1::Topic {
            topic_id: manifest.topic_id.clone(),
        },
        Some(channel_id) => SpatialContextV1::Channel {
            topic_id: manifest.topic_id.clone(),
            channel_id: channel_id.clone(),
        },
    }
}
