//! タイムラインの購読・復旧(subscription spawn / ingest / restart / hydrate)。
//! WP-H5 PR2 で timeline_runtime_support.rs から分割。View 変換は timeline_view_support.rs。

use super::*;

/// 購読と replica の読み書きの対象(#1280)。app-api の内部だけで使う。
///
/// `AllJoined` は、thread を開いたときの購読、root の channel が分からない thread の照合、private channel の一覧での
/// 同期の再開など、参加中の全 channel を対象にする内部の処理のためのもの。API の `TimelineScope` からは指定できない
/// (複数の channel をまたぐタイムラインのページは、許可されない channel の行を件数に比例して読み飛ばすので閉じた)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReplicaScope {
    Public,
    AllJoined,
    Channel { channel_id: ChannelId },
}

impl From<&TimelineScope> for ReplicaScope {
    fn from(scope: &TimelineScope) -> Self {
        match scope {
            TimelineScope::Public => Self::Public,
            TimelineScope::Channel { channel_id } => Self::Channel {
                channel_id: channel_id.clone(),
            },
        }
    }
}

impl From<&ReplicaScope> for ReplicaScope {
    fn from(scope: &ReplicaScope) -> Self {
        scope.clone()
    }
}

impl AppService {
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

    pub(crate) async fn ingest_event(
        &self,
        replica: &ReplicaId,
        envelope: KukuriEnvelope,
    ) -> Result<()> {
        // 自分の投稿も、他人の投稿と同じ検証(署名、書き込む replica と topic / channel の整合)を通す(#1248)。
        // docs は読まない。header と行は envelope から作るので、remote の反映が作る行と同じ値になる。
        let post = VerifiedPost::verify_local(envelope.clone(), replica).map_err(|reason| {
            anyhow::anyhow!("local post cannot be verified: {}", reason.as_str())
        })?;
        self.services.store.put_envelope(envelope.clone()).await?;
        let object = post.header().clone();
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
            projection_row_from_post(&post, content),
        )
        .await?;
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
        let channel = projection.channel_id.as_str();
        let Some(generation) = self
            .services
            .active_content_scope_generation(projection.topic_id.as_str(), channel)
            .await
        else {
            return Ok(None);
        };
        let replica = &projection.source_replica_id;
        let mut readers: Vec<(Arc<dyn DocsSync>, DocFetchPolicy)> = Vec::new();
        if channel == PUBLIC_CHANNEL_ID {
            readers.push((
                Arc::new(LocalSourceReader {
                    docs: self.services.docs_sync.clone(),
                    replica: replica.clone(),
                }),
                DocFetchPolicy::LocalOnly,
            ));
        } else if !replica.as_str().starts_with("bucket::") {
            readers.push((self.services.docs_sync.clone(), DocFetchPolicy::LocalOnly));
        }
        let epoch = if channel == PUBLIC_CHANNEL_ID {
            None
        } else {
            self.private_epoch_for_source(projection.topic_id.as_str(), channel, replica)
                .await?
        };
        if channel != PUBLIC_CHANNEL_ID && epoch.is_none() {
            return Ok(None);
        }
        for reader in self
            .remote_post_readers(
                projection.topic_id.as_str(),
                (channel != PUBLIC_CHANNEL_ID).then_some(channel),
                replica,
                epoch
                    .as_ref()
                    .map(|(id, secret)| (id.as_str(), secret.as_str())),
            )
            .await?
        {
            readers.push((reader, DocFetchPolicy::LocalThenRemote));
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        for (reader, policy) in readers {
            let read = async {
                if load_verified_post(
                    reader.as_ref(),
                    replica,
                    projection.topic_id.as_str(),
                    object_id,
                    policy,
                )
                .await?
                .is_none()
                {
                    return Ok::<_, anyhow::Error>(None);
                }
                let key = stable_key("objects", &format!("{}/envelope", object_id.as_str()));
                let scope = ReplicaPostScope::for_replica(replica, projection.topic_id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("post source is not a post replica"))?;
                let records = reader
                    .query_replica_exact_bounded(
                        replica,
                        &key,
                        MAX_ENVELOPE_RECORDS_PER_OBJECT,
                        DocFetchPolicy::LocalOnly,
                    )
                    .await?;
                Ok(records
                    .into_iter()
                    .filter_map(|record| {
                        let envelope: KukuriEnvelope =
                            serde_json::from_slice(&record.value).ok()?;
                        VerifiedPost::verify(envelope.clone(), object_id, replica, &scope)
                            .ok()
                            .map(|_| envelope)
                    })
                    .next())
            };
            let result = self
                .services
                .until_content_invalid(
                    projection.topic_id.as_str(),
                    channel,
                    generation,
                    tokio::time::timeout_at(deadline, read),
                )
                .await;
            match result {
                Some(Ok(Ok(Some(envelope)))) => {
                    let _save = self.services.content_save_access.lock().await;
                    if !self
                        .services
                        .content_scope_is_current(projection.topic_id.as_str(), channel, generation)
                        .await
                    {
                        return Ok(None);
                    }
                    self.services.store.put_envelope(envelope.clone()).await?;
                    return Ok(Some(envelope));
                }
                Some(Ok(Ok(None))) => {}
                Some(Ok(Err(error))) => warn!(%error, "post envelope source read failed"),
                Some(Err(_)) | None => break,
            }
        }
        Ok(None)
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
        let current_replica = {
            let joined = self.joined_private_channels.lock().await;
            let Some(state) =
                joined.get(&joined_private_channel_key(topic_id, channel_id.as_str()))
            else {
                return false;
            };
            if state.archived_epochs.is_empty() {
                return false;
            }
            current_private_channel_replica_id(state)
        };
        !page
            .items
            .iter()
            .any(|item| item.source_replica_id == current_replica)
    }

    /// scope が指す 1 つの channel(#1280)。private channel は、参加状態の確認を通ったものだけを返す。
    pub(crate) async fn allowed_channel_id_for_scope(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
    ) -> Result<String> {
        match scope {
            TimelineScope::Public => Ok(PUBLIC_CHANNEL_ID.to_string()),
            TimelineScope::Channel { channel_id } => {
                self.ensure_private_channel_access(topic_id, channel_id)
                    .await?;
                Ok(channel_id.as_str().to_string())
            }
        }
    }

    /// #1225: ページに出ている行のうち、本文 blob が欠けているものを行単位で取り直す。
    ///
    /// local にある本文はその場で読んで行へ反映する(network I/O なし)。local に無い本文は背景で取りに行き、
    /// 呼び出し側は待たない。試行は `MissingBodyLedger` の間隔と回数に従い、同時実行にも上限がある。
    /// 対象は呼び出し側が権限で絞り込んだ後のページの行だけで、replica の走査は行わない。
    pub(crate) async fn recover_missing_bodies(&self, rows: &mut [ObjectProjectionRow]) {
        let now = Utc::now().timestamp_millis();
        // 今回の呼び出しで取りに行き始めた本文(行の位置と task)。
        let mut started = Vec::new();
        for (index, row) in rows.iter_mut().enumerate() {
            if row.content.is_some() {
                continue;
            }
            let PayloadRef::BlobText { hash, .. } = &row.payload_ref else {
                continue;
            };
            let Some(scope_generation) = self
                .services
                .active_content_scope_generation(&row.topic_id, &row.channel_id)
                .await
            else {
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
                    let _save_access = self.services.content_save_access.lock().await;
                    if !self
                        .services
                        .content_scope_is_current(&row.topic_id, &row.channel_id, scope_generation)
                        .await
                    {
                        continue;
                    }
                    row.content = Some(text);
                    let stored = self.services.put_post_projection(row.clone()).await;
                    if let Err(error) = stored {
                        warn!(
                            object_id = %row.object_id.as_str(),
                            error = %error,
                            "failed to store a locally available post body"
                        );
                    }
                    self.services.missing_body_ledger.forget(hash);
                }
                continue;
            }
            if !self
                .services
                .missing_body_ledger
                .ready_key(hash.as_str(), now)
            {
                continue;
            }
            let Ok(permit) = self
                .services
                .missing_body_ledger
                .fetch_permits()
                .try_acquire_owned()
            else {
                continue;
            };
            let services = self.services.clone();
            let object_id = row.object_id.clone();
            let hash = hash.clone();
            let topic_id = row.topic_id.clone();
            let channel_id = row.channel_id.clone();
            let task = tokio::spawn(async move {
                let _permit = permit;
                let deadline = tokio::time::Instant::now() + projection_blob_fetch_timeout();
                let Some(Ok(Ok(fetch))) = services
                    .until_content_invalid(
                        &topic_id,
                        &channel_id,
                        scope_generation,
                        tokio::time::timeout_at(
                            deadline,
                            services.blob_service.prepare_retry_fetch(&hash),
                        ),
                    )
                    .await
                else {
                    return;
                };
                let Some(attempt) = services
                    .missing_body_ledger
                    .try_begin(&hash, Utc::now().timestamp_millis())
                else {
                    return;
                };
                let Some(fetched) = services
                    .until_content_invalid(
                        &topic_id,
                        &channel_id,
                        scope_generation,
                        tokio::time::timeout_at(deadline, fetch),
                    )
                    .await
                else {
                    attempt.defer();
                    return;
                };
                let Some(bytes) = fetched.ok().and_then(Result::ok).flatten() else {
                    attempt.fail();
                    return;
                };
                let text = String::from_utf8_lossy(&bytes).to_string();
                let _save_access = services.content_save_access.lock().await;
                if !services
                    .content_scope_is_current(&topic_id, &channel_id, scope_generation)
                    .await
                {
                    attempt.defer();
                    return;
                }
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
                        let stored = services
                            .blob_service
                            .put_remote_blob(bytes, "text/plain")
                            .await?;
                        anyhow::ensure!(stored.hash == hash, "recovered body hash changed");
                        current.content = Some(text);
                        services.put_post_projection(current).await?;
                    }
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
            started.push((index, task));
        }
        if started.is_empty() {
            return;
        }
        // 取りに行き始めた本文を、決まった短い時間だけ待つ。接続済みの peer からの本文は数十 ms で届くので、
        // 多くの場合は最初の表示から本文が入る。待つ時間は件数に依存せず、過ぎた取得は背景で続く
        // (台帳があるので、次の表示が同じ本文を重ねて取りに行くことはない)。
        let (indexes, tasks): (Vec<_>, Vec<_>) = started.into_iter().unzip();
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(hydration_limits::MISSING_BODY_DISPLAY_GRACE_MS),
            futures_util::future::join_all(tasks),
        )
        .await;
        for index in indexes {
            let row = &mut rows[index];
            if let Ok(Some(current)) = self
                .services
                .projection_store
                .get_object_projection(&row.object_id)
                .await
                && current.content.is_some()
            {
                row.content = current.content;
            }
        }
    }

    /// 利用者の操作(repost・reaction・reply・bookmark・取り下げ・community index の解決)の対象を
    /// projection に用意する(#1239)。
    ///
    /// projection にあれば何も読まない。無ければ、scope の replica から対象の key だけを読んで反映する。
    /// replica は走査しない。読む docs の record 数は、replica の大きさに依存しない。
    /// 戻り値は、対象が projection に用意できたか。
    ///
    /// 利用者の操作は `LocalOnly` で呼ぶ(entry の本体が手元に無い対象は、操作を待たせずに失敗させる)。
    pub(crate) async fn ensure_object_projection(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        object_id: &EnvelopeId,
        policy: DocFetchPolicy,
    ) -> Result<bool> {
        // private channel の scope は、projection に行が残っていても、参加状態の確認を先に通す。
        // 退出した channel の投稿を、手元に残った行から操作できないようにする(照会だけで docs は読まない)。
        if let TimelineScope::Channel { channel_id } = scope {
            self.ensure_private_channel_access(topic_id, channel_id)
                .await?;
        }
        if self
            .services
            .projection_store
            .get_object_projection(object_id)
            .await?
            .is_some()
        {
            return Ok(true);
        }
        for replica in self
            .local_page_replicas(topic_id, scope, None, false)
            .await?
        {
            if hydrate_object_in_topic(
                &self.services,
                topic_id,
                &replica,
                object_id,
                DocFetchPolicy::LocalOnly,
            )
            .await?
            {
                return Ok(true);
            }
        }
        if policy == DocFetchPolicy::LocalOnly {
            return Ok(false);
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
            return Ok(false);
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        for (replica, epoch) in self
            .remote_page_replicas(topic_id, scope, None, false, false)
            .await?
        {
            let readers = self
                .remote_post_readers(
                    topic_id,
                    (channel != PUBLIC_CHANNEL_ID).then_some(channel),
                    &replica,
                    epoch
                        .as_ref()
                        .map(|(id, secret)| (id.as_str(), secret.as_str())),
                )
                .await?;
            for reader in readers {
                let mut services = self.services.clone();
                services.docs_sync = reader;
                let read = tokio::time::timeout_at(
                    deadline,
                    hydrate_object_in_topic(
                        &services,
                        topic_id,
                        &replica,
                        object_id,
                        DocFetchPolicy::LocalThenRemote,
                    ),
                );
                match self
                    .services
                    .until_content_invalid(topic_id, channel, generation, read)
                    .await
                {
                    Some(Ok(Ok(true))) => return Ok(true),
                    Some(Ok(Ok(false))) => {}
                    Some(Ok(Err(error))) => warn!(%error, "post target source read failed"),
                    Some(Err(_)) | None => return Ok(false),
                }
            }
        }
        Ok(false)
    }

    /// 表示した投稿の取り下げを、背景で確認する(#1239)。呼び出し側は待たない。
    ///
    /// 対象は、購読していない topic の投稿でありうる repost 元と profile の投稿。projection に取り下げが
    /// 既にあれば何もしない。確認は確認先(replica と object id の組)ごとに間隔を空け、
    /// `withdrawals/<object id>/state` を key 指定で読むだけで、replica は走査しない。
    pub(crate) fn schedule_withdrawal_check(&self, topic_id: &str, object_id: &EnvelopeId) {
        let Ok(permit) = self
            .services
            .withdrawal_checks
            .permits()
            .try_acquire_owned()
        else {
            return;
        };
        let replica = topic_replica_id(topic_id);
        // 台帳の key は replica と object id の組にする。topic を偽った repost の snapshot が、
        // 正しい topic での確認を見送らせないようにする。
        let ledger_key = format!("{}\n{}", replica.as_str(), object_id.as_str());
        if !self
            .services
            .withdrawal_checks
            .try_begin(ledger_key.as_str(), Utc::now().timestamp_millis())
        {
            return;
        }
        let services = self.services.clone();
        let object_id = object_id.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let checked = async {
                let projection_store = services.projection_store.as_ref();
                if projection_store
                    .get_post_withdrawal(&object_id)
                    .await?
                    .is_some()
                {
                    return Ok::<_, anyhow::Error>(());
                }
                let docs_sync = services.docs_sync.as_ref();
                hydrate_post_withdrawal_for_object(
                    docs_sync,
                    projection_store,
                    &replica,
                    &object_id,
                    DocFetchPolicy::LocalThenRemote,
                )
                .await?;
                Ok(())
            }
            .await;
            if let Err(error) = checked {
                warn!(
                    object_id = %object_id.as_str(),
                    error = %error,
                    "failed to check a post withdrawal in the background"
                );
            }
        });
    }

    /// scope が読む replica。件数は「参加中の private channel × epoch」で決まり、投稿数に依存しない。
    /// private channel は、参加状態の確認(`ensure_private_channel_access`)を通ったものだけを返す。
    pub(crate) async fn scope_replicas(
        &self,
        topic_id: &str,
        scope: impl Into<ReplicaScope>,
    ) -> Result<Vec<ReplicaId>> {
        let scope = scope.into();
        let mut replicas = Vec::new();
        let states = match &scope {
            ReplicaScope::Public => {
                replicas.push(topic_replica_id(topic_id));
                Vec::new()
            }
            ReplicaScope::AllJoined => {
                replicas.push(topic_replica_id(topic_id));
                self.joined_private_channel_states_for_topic(topic_id).await
            }
            ReplicaScope::Channel { channel_id } => {
                self.ensure_private_channel_access(topic_id, channel_id)
                    .await?;
                self.joined_private_channel_state(topic_id, channel_id.as_str())
                    .await
                    .into_iter()
                    .collect()
            }
        };
        for state in states {
            for epoch in private_channel_epoch_capabilities(&state) {
                replicas.push(private_channel_replica_for_epoch(
                    state.channel_id.as_str(),
                    epoch.epoch_id.as_str(),
                ));
            }
        }
        Ok(replicas)
    }

    pub(crate) async fn maybe_restart_scope_replica_sync(
        &self,
        topic_id: &str,
        scope: impl Into<ReplicaScope>,
    ) {
        let scope = scope.into();
        self.maybe_restart_replica_sync(topic_id, &topic_replica_id(topic_id))
            .await;
        match &scope {
            ReplicaScope::Public => {}
            ReplicaScope::AllJoined => {
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
            ReplicaScope::Channel { channel_id } => {
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
        self.restart_scope_subscription(&ScopeKey::Channel(
            topic_id.to_string(),
            channel_id.to_string(),
        ))
        .await;
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
        self.restart_scope_subscription(&ScopeKey::Topic(topic_id.to_string()))
            .await;
    }

    pub(crate) async fn maybe_restart_scope_subscription(
        &self,
        topic_id: &str,
        scope: impl Into<ReplicaScope>,
    ) {
        let scope = scope.into();
        self.maybe_restart_topic_subscription(topic_id).await;
        match &scope {
            ReplicaScope::Public => {}
            ReplicaScope::AllJoined => {
                for state in self.joined_private_channel_states_for_topic(topic_id).await {
                    self.maybe_restart_private_channel_subscription(
                        topic_id,
                        state.channel_id.as_str(),
                    )
                    .await;
                }
            }
            ReplicaScope::Channel { channel_id } => {
                self.maybe_restart_private_channel_subscription(topic_id, channel_id.as_str())
                    .await;
            }
        }
    }
}
