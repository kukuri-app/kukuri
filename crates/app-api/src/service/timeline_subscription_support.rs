//! タイムラインの購読・復旧(subscription spawn / ingest / restart / hydrate)。
//! WP-H5 PR2 で timeline_runtime_support.rs から分割。View 変換は timeline_view_support.rs。

use super::*;

impl AppService {
    pub(crate) async fn ensure_topic_subscription(&self, topic_id: &str) -> Result<()> {
        if self.is_topic_gossip_disabled(topic_id).await {
            return Ok(());
        }
        let stale_key = {
            let subscriptions = self.subscription_registry.subscriptions.lock().await;
            match subscriptions.get(topic_id) {
                Some(handle) if !handle.is_finished() => return Ok(()),
                Some(_) => Some(topic_id.to_string()),
                None => None,
            }
        };
        if let Some(stale_key) = stale_key {
            self.subscription_registry
                .subscriptions
                .lock()
                .await
                .remove(stale_key.as_str());
        }

        self.spawn_topic_subscription(topic_id).await
    }

    pub(crate) async fn has_topic_subscription(&self, topic_id: &str) -> bool {
        self.subscription_registry
            .subscriptions
            .lock()
            .await
            .get(topic_id)
            .is_some_and(|handle| !handle.is_finished())
    }

    pub(crate) async fn should_restart_after_empty_result(&self, key: &str) -> bool {
        !self
            .empty_recovery_candidates
            .lock()
            .await
            .insert(key.to_string())
    }

    pub(crate) async fn clear_empty_result_restart_marker(&self, key: &str) {
        self.empty_recovery_candidates.lock().await.remove(key);
    }

    pub(crate) async fn restart_topic_subscription(&self, topic_id: &str) -> Result<()> {
        if let Some(handle) = self
            .subscription_registry
            .subscriptions
            .lock()
            .await
            .remove(topic_id)
        {
            handle.abort();
        }
        self.services
            .hint_transport
            .unsubscribe_hints(&TopicId::new(topic_id))
            .await?;
        self.spawn_topic_subscription(topic_id).await
    }

    pub(crate) async fn spawn_topic_subscription(&self, topic_id: &str) -> Result<()> {
        self.spawn_subscription_task(
            topic_id,
            None,
            topic_replica_id(topic_id),
            TopicId::new(topic_id),
            None,
        )
        .await
    }

    pub(crate) async fn ingest_event(
        &self,
        replica: &ReplicaId,
        envelope: KukuriEnvelope,
        _stored_blob: Option<StoredBlob>,
        attachments: Vec<(AssetRole, StoredBlob)>,
    ) -> Result<()> {
        self.services.store.put_envelope(envelope.clone()).await?;
        let mut object = envelope
            .to_post_object()?
            .ok_or_else(|| anyhow::anyhow!("expected timeline envelope"))?;
        if object.object_kind != "repost" {
            object.attachments = attachments
                .iter()
                .map(|(role, stored)| kukuri_core::AssetRef {
                    hash: stored.hash.clone(),
                    mime: stored.mime.clone(),
                    bytes: stored.bytes,
                    role: role.clone(),
                })
                .collect();
        }
        let content = match &object.payload_ref {
            PayloadRef::InlineText { text } => Some(text.clone()),
            PayloadRef::BlobText { hash, .. } => self
                .services
                .blob_service
                .fetch_blob(hash)
                .await?
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string()),
        };
        persist_post_object(
            self.services.docs_sync.as_ref(),
            replica,
            object.clone(),
            envelope.clone(),
        )
        .await?;
        if let Err(error) = self.services.docs_sync.restart_replica_sync(replica).await {
            warn!(
                replica_id = %replica.as_str(),
                error = %error,
                "failed to restart replica sync after local timeline write"
            );
        }
        ObjectProjectionStore::put_object_projection(
            self.services.projection_store.as_ref(),
            projection_row_from_header(&object, content, replica),
        )
        .await?;
        if let PayloadRef::BlobText { hash, .. } = &object.payload_ref {
            BlobCacheStore::mark_blob_status(
                self.services.projection_store.as_ref(),
                hash,
                BlobCacheStatus::Available,
            )
            .await?;
        }
        for (_, attachment) in attachments {
            BlobCacheStore::mark_blob_status(
                self.services.projection_store.as_ref(),
                &attachment.hash,
                BlobCacheStatus::Available,
            )
            .await?;
        }
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        Ok(())
    }

    pub(crate) async fn resolve_parent_object(
        &self,
        object_id: &EnvelopeId,
    ) -> Result<Option<KukuriEnvelope>> {
        if let Some(envelope) = self.services.store.get_envelope(object_id).await? {
            return Ok(Some(envelope));
        }

        let Some(projection) = ObjectProjectionStore::get_object_projection(
            self.services.projection_store.as_ref(),
            object_id,
        )
        .await?
        else {
            return Ok(None);
        };

        let object_kind = projection.object_kind.as_str();
        let mut tags = vec![
            vec!["topic".into(), projection.topic_id.clone()],
            vec!["object".into(), object_kind.to_string()],
        ];
        if projection.channel_id != PUBLIC_CHANNEL_ID {
            tags.push(vec!["channel".into(), projection.channel_id.clone()]);
        }

        Ok(Some(KukuriEnvelope {
            id: projection.object_id,
            pubkey: projection.author_pubkey.into(),
            created_at: projection.created_at,
            kind: object_kind.into(),
            tags,
            content: serde_json::to_string(&kukuri_core::KukuriPostEnvelopeContentV1 {
                object_kind: object_kind.into(),
                topic_id: TopicId::new(projection.topic_id.clone()),
                channel_id: channel_id_from_storage(projection.channel_id.as_str()),
                payload_ref: projection.payload_ref.clone(),
                attachments: Vec::new(),
                media_manifest_refs: Vec::new(),
                visibility: if projection.channel_id == PUBLIC_CHANNEL_ID {
                    ObjectVisibility::Public
                } else {
                    ObjectVisibility::Private
                },
                reply_to: projection.reply_to_object_id.clone(),
                root_id: projection.root_object_id.clone(),
                repost_of: projection.repost_of.clone(),
                content_labels: projection.content_labels.clone(),
            })?,
            sig: String::new(),
        }))
    }

    pub(crate) async fn resolve_signed_post_envelope(
        &self,
        object_id: &EnvelopeId,
    ) -> Result<Option<KukuriEnvelope>> {
        if let Some(envelope) = self.services.store.get_envelope(object_id).await? {
            envelope.verify()?;
            return Ok(Some(envelope));
        }
        let Some(projection) = ObjectProjectionStore::get_object_projection(
            self.services.projection_store.as_ref(),
            object_id,
        )
        .await?
        else {
            return Ok(None);
        };
        let key = stable_key("objects", &format!("{}/envelope", object_id.as_str()));
        let Some(record) = self
            .services
            .docs_sync
            .query_replica(&projection.source_replica_id, DocQuery::Exact(key))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
        envelope.verify()?;
        if envelope.id != *object_id {
            anyhow::bail!("signed post envelope object id does not match");
        }
        self.services.store.put_envelope(envelope.clone()).await?;
        Ok(Some(envelope))
    }

    pub async fn ensure_scope_subscriptions(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
    ) -> Result<()> {
        self.ensure_topic_subscription(topic_id).await?;
        match scope {
            TimelineScope::Public => Ok(()),
            TimelineScope::AllJoined => {
                self.ensure_joined_private_channel_subscriptions(topic_id)
                    .await
            }
            TimelineScope::Channel { channel_id } => {
                self.ensure_private_channel_access(topic_id, channel_id)
                    .await?;
                self.ensure_private_channel_subscription(topic_id, channel_id.as_str())
                    .await
            }
        }
    }

    pub(crate) async fn scope_needs_current_private_epoch_hydration(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        page: &Page<ObjectProjectionRow>,
    ) -> bool {
        let TimelineScope::Channel { channel_id } = scope else {
            return false;
        };
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id.as_str())
            .await
        else {
            return false;
        };
        if state.archived_epochs.is_empty() {
            return false;
        }
        let current_replica = current_private_channel_replica_id(&state);
        !page
            .items
            .iter()
            .any(|item| item.source_replica_id == current_replica)
    }

    pub(crate) async fn allowed_channel_ids_for_scope(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
    ) -> Result<BTreeSet<String>> {
        let mut allowed = BTreeSet::new();
        match scope {
            TimelineScope::Public => {
                allowed.insert(PUBLIC_CHANNEL_ID.to_string());
            }
            TimelineScope::AllJoined => {
                allowed.insert(PUBLIC_CHANNEL_ID.to_string());
                for state in self.joined_private_channel_states_for_topic(topic_id).await {
                    allowed.insert(state.channel_id.as_str().to_string());
                }
            }
            TimelineScope::Channel { channel_id } => {
                self.ensure_private_channel_access(topic_id, channel_id)
                    .await?;
                allowed.insert(channel_id.as_str().to_string());
            }
        }
        Ok(allowed)
    }

    /// #1225: ページに出ている行のうち、本文 blob が欠けているものを行単位で取り直す。
    ///
    /// local にある本文はその場で読んで行へ反映する(network I/O なし)。local に無い本文は背景で取りに行き、
    /// 呼び出し側は待たない。試行は `MissingBodyLedger` の間隔と回数に従い、同時実行にも上限がある。
    /// 対象は呼び出し側が権限で絞り込んだ後のページの行だけで、replica の走査は行わない。
    pub(crate) async fn recover_missing_bodies(&self, rows: &mut [ObjectProjectionRow]) {
        let now = Utc::now().timestamp_millis();
        for row in rows {
            if row.content.is_some() {
                continue;
            }
            let PayloadRef::BlobText { hash, .. } = &row.payload_ref else {
                continue;
            };
            let local = matches!(
                best_effort_blob_cache_status(self.services.blob_service.as_ref(), hash).await,
                BlobCacheStatus::Available | BlobCacheStatus::Pinned
            );
            if local {
                if let Some(text) =
                    fetch_projection_blob_text(self.services.blob_service.as_ref(), hash).await
                {
                    row.content = Some(text);
                    let stored = self
                        .services
                        .projection_store
                        .put_object_projection(row.clone())
                        .await;
                    if let Err(error) = stored {
                        warn!(
                            object_id = %row.object_id.as_str(),
                            error = %error,
                            "failed to store a locally available post body"
                        );
                    }
                    let _ = self
                        .services
                        .projection_store
                        .mark_blob_status(hash, BlobCacheStatus::Available)
                        .await;
                    self.services.missing_body_ledger.forget(hash);
                }
                continue;
            }
            // `attempt` は task と一緒に破棄されても失敗として記録される(panic・runtime の終了を含む)。
            let Some(attempt) = self.services.missing_body_ledger.try_begin(hash, now) else {
                continue;
            };
            let services = self.services.clone();
            let object_id = row.object_id.clone();
            let hash = hash.clone();
            tokio::spawn(async move {
                let permits = services.missing_body_ledger.fetch_permits();
                let Ok(_permit) = permits.acquire().await else {
                    attempt.fail();
                    return;
                };
                let Some(text) =
                    fetch_projection_blob_text(services.blob_service.as_ref(), &hash).await
                else {
                    attempt.fail();
                    return;
                };
                let stored = async {
                    let projection_store = services.projection_store.as_ref();
                    // 取得を待つ間に取り下げや更新が入った行は書き戻さない。
                    if projection_store
                        .get_post_withdrawal(&object_id)
                        .await?
                        .is_some()
                    {
                        return Ok::<_, anyhow::Error>(());
                    }
                    let Some(mut current) =
                        projection_store.get_object_projection(&object_id).await?
                    else {
                        return Ok(());
                    };
                    let same_body = matches!(
                        &current.payload_ref,
                        PayloadRef::BlobText { hash: current_hash, .. } if *current_hash == hash
                    );
                    if current.content.is_none() && same_body {
                        current.content = Some(text);
                        projection_store.put_object_projection(current).await?;
                    }
                    projection_store
                        .mark_blob_status(&hash, BlobCacheStatus::Available)
                        .await?;
                    Ok(())
                }
                .await;
                match stored {
                    Ok(()) => attempt.succeed(),
                    Err(error) => {
                        warn!(
                            object_id = %object_id.as_str(),
                            error = %error,
                            "failed to store a recovered post body"
                        );
                        attempt.fail();
                    }
                }
            });
        }
    }

    /// 利用者の操作(repost・reaction・reply・bookmark など)の対象が projection に無いときに備えた走査。
    ///
    /// 対象の行が projection に無い場合は、指紋による省略を無効にして必ず反映し直す(#1225)。
    pub(crate) async fn hydrate_scope_projection_for_target(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        target_object_id: &EnvelopeId,
    ) -> Result<usize> {
        let target_known = self
            .services
            .projection_store
            .get_object_projection(target_object_id)
            .await?
            .is_some();
        if !target_known {
            self.forget_scope_scan_cache(topic_id, scope).await;
        }
        Box::pin(self.hydrate_scope_projection(topic_id, scope)).await
    }

    pub(crate) async fn forget_scope_scan_cache(&self, topic_id: &str, scope: &TimelineScope) {
        let mut replicas = vec![topic_replica_id(topic_id)];
        let states = match scope {
            TimelineScope::Public => Vec::new(),
            TimelineScope::AllJoined => {
                self.joined_private_channel_states_for_topic(topic_id).await
            }
            TimelineScope::Channel { channel_id } => self
                .joined_private_channel_state(topic_id, channel_id.as_str())
                .await
                .into_iter()
                .collect(),
        };
        for state in states {
            for epoch in private_channel_epoch_capabilities(&state) {
                replicas.push(private_channel_replica_for_epoch(
                    state.channel_id.as_str(),
                    epoch.epoch_id.as_str(),
                ));
            }
        }
        for replica in replicas {
            self.services
                .replica_scan_cache
                .forget_replica(replica.as_str());
        }
    }

    pub(crate) async fn hydrate_scope_projection(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
    ) -> Result<usize> {
        let mut hydrated =
            hydrate_topic_state(&self.services, topic_id, DocFetchPolicy::LocalOnly).await?;
        match scope {
            TimelineScope::Public => {}
            TimelineScope::AllJoined => {
                for state in self.joined_private_channel_states_for_topic(topic_id).await {
                    for replica in
                        private_channel_epoch_capabilities(&state)
                            .into_iter()
                            .map(|epoch| {
                                private_channel_replica_for_epoch(
                                    state.channel_id.as_str(),
                                    epoch.epoch_id.as_str(),
                                )
                            })
                    {
                        hydrated += hydrate_subscription_state(
                            &self.services,
                            topic_id,
                            &replica,
                            DocFetchPolicy::LocalOnly,
                        )
                        .await?;
                    }
                }
            }
            TimelineScope::Channel { channel_id } => {
                self.ensure_private_channel_access(topic_id, channel_id)
                    .await?;
                if let Some(state) = self
                    .joined_private_channel_state(topic_id, channel_id.as_str())
                    .await
                {
                    for replica in
                        private_channel_epoch_capabilities(&state)
                            .into_iter()
                            .map(|epoch| {
                                private_channel_replica_for_epoch(
                                    state.channel_id.as_str(),
                                    epoch.epoch_id.as_str(),
                                )
                            })
                    {
                        hydrated += hydrate_subscription_state(
                            &self.services,
                            topic_id,
                            &replica,
                            DocFetchPolicy::LocalOnly,
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(hydrated)
    }

    pub(crate) async fn maybe_restart_scope_replica_sync(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
    ) {
        self.maybe_restart_replica_sync(topic_id, &topic_replica_id(topic_id))
            .await;
        match scope {
            TimelineScope::Public => {}
            TimelineScope::AllJoined => {
                for state in self.joined_private_channel_states_for_topic(topic_id).await {
                    self.maybe_restart_private_channel_subscription(
                        topic_id,
                        state.channel_id.as_str(),
                    )
                    .await;
                    for replica in
                        private_channel_epoch_capabilities(&state)
                            .into_iter()
                            .map(|epoch| {
                                private_channel_replica_for_epoch(
                                    state.channel_id.as_str(),
                                    epoch.epoch_id.as_str(),
                                )
                            })
                    {
                        self.maybe_restart_replica_sync(topic_id, &replica).await;
                    }
                }
            }
            TimelineScope::Channel { channel_id } => {
                if let Some(state) = self
                    .joined_private_channel_state(topic_id, channel_id.as_str())
                    .await
                {
                    self.maybe_restart_private_channel_subscription(topic_id, channel_id.as_str())
                        .await;
                    for replica in
                        private_channel_epoch_capabilities(&state)
                            .into_iter()
                            .map(|epoch| {
                                private_channel_replica_for_epoch(
                                    state.channel_id.as_str(),
                                    epoch.epoch_id.as_str(),
                                )
                            })
                    {
                        self.maybe_restart_replica_sync(topic_id, &replica).await;
                    }
                }
            }
        }
    }

    pub(crate) async fn maybe_restart_replica_sync(&self, topic_id: &str, replica: &ReplicaId) {
        maybe_restart_replica_sync_with_cooldown(
            self.services.docs_sync.as_ref(),
            &self.subscription_registry.replica_sync_restart_deadlines,
            topic_id,
            replica,
        )
        .await;
    }

    pub(crate) async fn maybe_restart_private_channel_subscription(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) {
        let key = format!("private-channel:{topic_id}:{channel_id}");
        let now = Utc::now().timestamp();
        {
            let mut deadlines = self
                .subscription_registry
                .replica_sync_restart_deadlines
                .lock()
                .await;
            let next_due_at = deadlines.get(key.as_str()).copied().unwrap_or_default();
            if next_due_at > now {
                return;
            }
            deadlines.insert(key, now.saturating_add(REPLICA_SYNC_RESTART_RETRY_SECONDS));
        }
        if let Err(error) = self
            .restart_private_channel_subscription(topic_id, channel_id)
            .await
        {
            warn!(
                topic = %topic_id,
                channel_id = %channel_id,
                error = %error,
                "failed to restart private channel subscription"
            );
        }
    }

    pub(crate) async fn maybe_restart_topic_subscription(&self, topic_id: &str) {
        let key = format!("topic-subscription:{topic_id}");
        let now = Utc::now().timestamp();
        {
            let mut deadlines = self
                .subscription_registry
                .replica_sync_restart_deadlines
                .lock()
                .await;
            let next_due_at = deadlines.get(key.as_str()).copied().unwrap_or_default();
            if next_due_at > now {
                return;
            }
            deadlines.insert(key, now.saturating_add(REPLICA_SYNC_RESTART_RETRY_SECONDS));
        }
        if let Err(error) = self.restart_topic_subscription(topic_id).await {
            warn!(
                topic = %topic_id,
                error = %error,
                "failed to restart topic subscription"
            );
        }
    }

    pub(crate) async fn maybe_restart_scope_subscription(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
    ) {
        self.maybe_restart_topic_subscription(topic_id).await;
        match scope {
            TimelineScope::Public => {}
            TimelineScope::AllJoined => {
                for state in self.joined_private_channel_states_for_topic(topic_id).await {
                    self.maybe_restart_private_channel_subscription(
                        topic_id,
                        state.channel_id.as_str(),
                    )
                    .await;
                }
            }
            TimelineScope::Channel { channel_id } => {
                self.maybe_restart_private_channel_subscription(topic_id, channel_id.as_str())
                    .await;
            }
        }
    }
}
