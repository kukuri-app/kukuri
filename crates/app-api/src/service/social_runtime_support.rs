use super::*;

impl AppService {
    pub(crate) async fn build_author_social_view(
        &self,
        author_pubkey: &str,
    ) -> Result<AuthorSocialView> {
        let profile = self.services.store.get_profile(author_pubkey).await?;
        let relationship = self
            .services
            .projection_store
            .get_author_relationship(self.current_author_pubkey().as_str(), author_pubkey)
            .await?;
        let muted = self
            .services
            .projection_store
            .get_muted_author(author_pubkey)
            .await?
            .is_some();
        let local_author = self.current_author_pubkey();
        let blocking = self
            .services
            .store
            .list_block_edges_by_subject(local_author.as_str())
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == author_pubkey
                    && edge.status == BlockEdgeStatus::Active
            });
        let blocked_by = self
            .services
            .store
            .list_block_edges_by_target(local_author.as_str())
            .await?
            .into_iter()
            .any(|edge| {
                edge.subject_pubkey.as_str() == author_pubkey
                    && edge.status == BlockEdgeStatus::Active
            });
        let mut view = author_social_view_from_parts(
            author_pubkey,
            profile.as_ref(),
            relationship.as_ref(),
            muted,
            blocking,
            blocked_by,
        );
        view.provenance = self
            .content_provenance_view("profile", author_pubkey, "author_docs")
            .await?;
        Ok(view)
    }

    pub(crate) async fn rebuild_author_relationships(&self) -> Result<()> {
        rebuild_author_relationships(
            self.services.store.as_ref(),
            self.services.projection_store.as_ref(),
            self.current_author_pubkey().as_str(),
        )
        .await?;
        self.reconcile_direct_message_subscriptions().await
    }

    pub(crate) async fn restart_direct_message_subscriptions(&self) -> Result<()> {
        let existing_peers = self
            .subscription_registry
            .direct_message_subscriptions
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for peer_pubkey in existing_peers {
            stop_direct_message_subscription(
                self.subscription_registry
                    .direct_message_subscriptions
                    .as_ref(),
                &self.services,
                peer_pubkey.as_str(),
            )
            .await?;
        }
        self.reconcile_direct_message_subscriptions().await
    }

    pub(crate) async fn current_muted_author_pubkeys(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .services
            .projection_store
            .list_muted_authors()
            .await?
            .into_iter()
            .map(|row| row.author_pubkey)
            .collect())
    }

    /// #961: content surface から隠す author の集合。ミュート(端末内)に加え、どちらの向きでも
    /// Active な署名済み block edge を持つ相手を含める。取得・保存は禁止せず表示だけを隠す。
    pub(crate) async fn current_hidden_author_pubkeys(&self) -> Result<BTreeSet<String>> {
        let mut hidden = self.current_muted_author_pubkeys().await?;
        let local_author = self.current_author_pubkey();
        hidden.extend(
            self.services
                .store
                .list_block_edges_by_subject(local_author.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == BlockEdgeStatus::Active)
                .map(|edge| edge.target_pubkey.as_str().to_string()),
        );
        hidden.extend(
            self.services
                .store
                .list_block_edges_by_target(local_author.as_str())
                .await?
                .into_iter()
                .filter(|edge| edge.status == BlockEdgeStatus::Active)
                .map(|edge| edge.subject_pubkey.as_str().to_string()),
        );
        Ok(hidden)
    }

    pub(crate) async fn authors_blocked_either_direction(
        &self,
        left_pubkey: &str,
        right_pubkey: &str,
    ) -> Result<bool> {
        let left_blocks_right = self
            .services
            .store
            .list_block_edges_by_subject(left_pubkey)
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == right_pubkey
                    && edge.status == BlockEdgeStatus::Active
            });
        if left_blocks_right {
            return Ok(true);
        }
        Ok(self
            .services
            .store
            .list_block_edges_by_subject(right_pubkey)
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == left_pubkey && edge.status == BlockEdgeStatus::Active
            }))
    }

    pub(crate) async fn owner_blocks_visitor(
        &self,
        owner_pubkey: &str,
        visitor_pubkey: &str,
    ) -> Result<bool> {
        Ok(self
            .services
            .store
            .list_block_edges_by_subject(owner_pubkey)
            .await?
            .into_iter()
            .any(|edge| {
                edge.target_pubkey.as_str() == visitor_pubkey
                    && edge.status == BlockEdgeStatus::Active
            }))
    }

    pub(crate) async fn ensure_author_subscriptions_for_rows(
        &self,
        rows: &[ObjectProjectionRow],
    ) -> Result<()> {
        let mut author_pubkeys = BTreeSet::new();
        for row in rows {
            author_pubkeys.insert(row.author_pubkey.clone());
            if let Some(repost_of) = row.repost_of.as_ref() {
                author_pubkeys.insert(repost_of.source_author_pubkey.as_str().to_string());
            }
        }
        for author_pubkey in author_pubkeys {
            self.ensure_author_subscription(author_pubkey.as_str())
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn ensure_author_subscription(&self, author_pubkey: &str) -> Result<()> {
        let author_pubkey = normalize_author_pubkey(author_pubkey)?;
        let stale_key = {
            let subscriptions = self.subscription_registry.author_subscriptions.lock().await;
            match subscriptions.get(author_pubkey.as_str()) {
                Some(handle) if !handle.is_finished() => return Ok(()),
                Some(_) => Some(author_pubkey.to_string()),
                None => None,
            }
        };
        if let Some(stale_key) = stale_key {
            self.subscription_registry
                .author_subscriptions
                .lock()
                .await
                .remove(stale_key.as_str());
        }

        self.spawn_author_subscription(author_pubkey.as_str()).await
    }

    pub(crate) async fn restart_author_subscription(&self, author_pubkey: &str) -> Result<()> {
        let author_pubkey = normalize_author_pubkey(author_pubkey)?;
        if let Some(handle) = self
            .subscription_registry
            .author_subscriptions
            .lock()
            .await
            .remove(author_pubkey.as_str())
        {
            handle.abort();
        }
        self.spawn_author_subscription(author_pubkey.as_str()).await
    }

    pub(crate) async fn maybe_restart_author_subscription(&self, author_pubkey: &str) {
        let Ok(author_pubkey) = normalize_author_pubkey(author_pubkey) else {
            return;
        };
        let key = format!("author-subscription:{author_pubkey}");
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
            .restart_author_subscription(author_pubkey.as_str())
            .await
        {
            warn!(
                author_pubkey = %author_pubkey,
                error = %error,
                "failed to restart author subscription"
            );
        }
    }

    pub(crate) async fn spawn_author_subscription(&self, author_pubkey: &str) -> Result<()> {
        let services = self.services.clone();
        let last_sync = Arc::clone(&self.last_sync_ts);
        let notification_inserted = Arc::clone(&self.notification_inserted_notify);
        let direct_message_subscriptions =
            Arc::clone(&self.subscription_registry.direct_message_subscriptions);
        let author_key = normalize_author_pubkey(author_pubkey)?;
        let local_author_pubkey = self.current_author_pubkey();
        let replica = author_replica_id(author_key.as_str());
        services.docs_sync.open_replica(&replica).await?;
        let mut doc_stream = services
            .docs_sync
            .subscribe_replica_notices(&replica)
            .await?;
        let author_key_for_task = author_key.clone();
        let handle = tokio::spawn(async move {
            let store = &services.store;
            let projection_store = &services.projection_store;
            let docs_sync = &services.docs_sync;
            let blob_service = &services.blob_service;
            let notification_baseline = match snapshot_follow_notification_baseline(
                docs_sync.as_ref(),
                &replica,
                local_author_pubkey.as_str(),
            )
            .await
            {
                Ok(baseline) => baseline,
                Err(error) => {
                    warn!(
                        author_pubkey = %author_key_for_task,
                        error = %error,
                        "failed to snapshot local follow baseline for author bootstrap"
                    );
                    NotificationDocEventBaseline::default()
                }
            };
            match hydrate_author_state(
                &services,
                local_author_pubkey.as_str(),
                author_key_for_task.as_str(),
                DocFetchPolicy::LocalOnly,
            )
            .await
            {
                Ok(initial_count) if initial_count > 0 => {
                    *last_sync.lock().await = Some(Utc::now().timestamp_millis());
                    schedule_direct_message_reconcile(
                        services.clone(),
                        Arc::clone(&last_sync),
                        Arc::clone(&direct_message_subscriptions),
                        Arc::clone(&notification_inserted),
                        local_author_pubkey.clone(),
                        author_key_for_task.clone(),
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        author_pubkey = %author_key_for_task,
                        error = %error,
                        "failed to hydrate local author cache during bootstrap"
                    );
                }
            }
            let recovery_services = services.clone();
            let recovery_last_sync = Arc::clone(&last_sync);
            let recovery_notification_inserted = Arc::clone(&notification_inserted);
            let recovery_direct_message_subscriptions = Arc::clone(&direct_message_subscriptions);
            let recovery_local_author_pubkey = local_author_pubkey.clone();
            let recovery_author_pubkey = author_key_for_task.clone();
            tokio::spawn(async move {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    hydrate_author_state(
                        &recovery_services,
                        recovery_local_author_pubkey.as_str(),
                        recovery_author_pubkey.as_str(),
                        DocFetchPolicy::LocalThenRemote,
                    ),
                )
                .await
                {
                    Ok(Ok(initial_count)) if initial_count > 0 => {
                        *recovery_last_sync.lock().await = Some(Utc::now().timestamp_millis());
                        schedule_direct_message_reconcile(
                            recovery_services,
                            Arc::clone(&recovery_last_sync),
                            Arc::clone(&recovery_direct_message_subscriptions),
                            Arc::clone(&recovery_notification_inserted),
                            recovery_local_author_pubkey,
                            recovery_author_pubkey,
                        );
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        warn!(
                            author_pubkey = %recovery_author_pubkey,
                            error = %error,
                            "failed to hydrate remote author cache during bootstrap recovery"
                        );
                    }
                    Err(_) => {
                        warn!(
                            author_pubkey = %recovery_author_pubkey,
                            "timed out hydrating remote author cache during bootstrap recovery"
                        );
                    }
                }
            });
            // #1239: 自分の replica の follow・block は、起動時の上限つきの一覧に収まらないことがある。背景で小分けに
            // すべて読む(読み終えた位置を残し、読み終えたら繰り返さない)。購読タスクが止まると、この task も止まる。
            let is_own_replica = author_key_for_task == local_author_pubkey;
            let mut own_edge_sweep = is_own_replica
                .then(|| spawn_own_edge_sweep(&services, author_key_for_task.as_str()));
            // #1239: replica は走査しない。docs の event はその key だけを反映する。取りこぼしと同期の区切りでは、
            // 上限つきの追いつき(`catch_up_author_state`)を間隔を空けて 1 回にまとめる。
            let mut catch_up = CatchUpSchedule::default();
            let mut catch_up_tick = tokio::time::interval(std::time::Duration::from_secs(1));
            catch_up_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    notice = doc_stream.next() => {
                        let Some(notice) = notice else {
                            break;
                        };
                        let event = match notice {
                            Ok(kukuri_docs_sync::ReplicaNotice::Entry(event)) => event,
                            Ok(kukuri_docs_sync::ReplicaNotice::Lagged { .. }) => {
                                catch_up.request_now();
                                // 自分の replica の event を取りこぼした。どの key かは分からないので、自分の edge の
                                // 読み出しを最初からやり直す(読み終えた印があっても。新しい端末の最初の同期で、
                                // 読み終えた後に大量の edge が届いた場合など)。
                                if is_own_replica {
                                    // 走っている読み出しを止めてから、位置を戻す(止めないと、古い位置で上書きされる)。
                                    drop(own_edge_sweep.take());
                                    if let Err(error) = restart_own_author_edge_sweep(
                                        services.projection_store.as_ref(),
                                        author_key_for_task.as_str(),
                                    )
                                    .await
                                    {
                                        warn!(
                                            author_pubkey = %author_key_for_task,
                                            error = %error,
                                            "failed to restart the own follow and block edge reading"
                                        );
                                    }
                                    own_edge_sweep.replace(spawn_own_edge_sweep(
                                        &services,
                                        author_key_for_task.as_str(),
                                    ));
                                }
                                continue;
                            }
                            Ok(kukuri_docs_sync::ReplicaNotice::ContentReady) => {
                                catch_up.request_now();
                                continue;
                            }
                            Ok(kukuri_docs_sync::ReplicaNotice::SyncFinished) => {
                                catch_up.request();
                                continue;
                            }
                            Err(_) => continue,
                        };
                        let event = &event;
                        if event.source_peer.is_some() {
                            *last_sync.lock().await = Some(Utc::now().timestamp_millis());
                        }
                        if let Some(source_peer) = event.source_peer.as_deref() {
                            if let Err(error) = docs_sync.learn_peer(source_peer).await {
                                warn!(
                                    author_pubkey = %author_key_for_task,
                                    source_peer = %source_peer,
                                    error = %error,
                                    "failed to learn docs peer from author sync event"
                                );
                            }
                            if let Err(error) = blob_service.learn_peer(source_peer).await {
                                warn!(
                                    author_pubkey = %author_key_for_task,
                                    source_peer = %source_peer,
                                    error = %error,
                                    "failed to learn blob peer from author sync event"
                                );
                            }
                        }
                        match AppService::maybe_create_notification_for_remote_follow_event(
                            store.as_ref(),
                            projection_store.as_ref(),
                            docs_sync.as_ref(),
                            local_author_pubkey.as_str(),
                            author_key_for_task.as_str(),
                            &notification_baseline,
                            event,
                        ).await {
                            Ok(true) => {
                                *last_sync.lock().await = Some(Utc::now().timestamp_millis());
                                notification_inserted.notify_waiters();
                            }
                            Ok(false) => {}
                            Err(error) => {
                                warn!(
                                    author_pubkey = %author_key_for_task,
                                    key = %event.key,
                                    error = %error,
                                    "failed to create notification from remote follow event"
                                );
                            }
                        }
                        match hydrate_author_key(
                            &services,
                            local_author_pubkey.as_str(),
                            author_key_for_task.as_str(),
                            event.key.as_str(),
                            DocFetchPolicy::LocalThenRemote,
                        ).await {
                            Ok(outcome) if outcome.reflected > 0 => {
                                if outcome.changed > 0 {
                                    catch_up.record_progress();
                                }
                                *last_sync.lock().await = Some(Utc::now().timestamp_millis());
                                schedule_direct_message_reconcile(
                                    services.clone(),
                                    Arc::clone(&last_sync),
                                    Arc::clone(&direct_message_subscriptions),
                                    Arc::clone(&notification_inserted),
                                    local_author_pubkey.clone(),
                                    author_key_for_task.clone(),
                                );
                            }
                            Ok(_) => {
                                // 相手から届いた profile・follow・block の key が反映できなかった(本体がまだ届いていない
                                // など)。走査はせず、追いつきを依頼する。
                                if event.source_peer.is_some()
                                    && (event.key == "profile/latest"
                                        || event.key.starts_with("graph/follows/")
                                        || event.key.starts_with("graph/blocks/"))
                                {
                                    catch_up.request_now();
                                }
                            }
                            Err(error) => {
                                warn!(
                                    author_pubkey = %author_key_for_task,
                                    key = %event.key,
                                    error = %error,
                                    "failed to hydrate author state from docs event"
                                );
                                catch_up.request_now();
                            }
                        }
                    }
                    _ = catch_up_tick.tick() => {
                        let Some(run) = catch_up.take_due(Utc::now().timestamp_millis()) else {
                            continue;
                        };
                        let changed = match catch_up_author_state(
                            &services,
                            local_author_pubkey.as_str(),
                            author_key_for_task.as_str(),
                            DocFetchPolicy::LocalThenRemote,
                        )
                        .await
                        {
                            Ok(outcome) => outcome.changed,
                            Err(error) => {
                                warn!(
                                    author_pubkey = %author_key_for_task,
                                    error = %error,
                                    "failed to catch up author state"
                                );
                                catch_up.restore(run);
                                0
                            }
                        };
                        let now = Utc::now().timestamp_millis();
                        catch_up.record_finished(now, changed);
                        if changed > 0 {
                            *last_sync.lock().await = Some(now);
                            schedule_direct_message_reconcile(
                                services.clone(),
                                Arc::clone(&last_sync),
                                Arc::clone(&direct_message_subscriptions),
                                Arc::clone(&notification_inserted),
                                local_author_pubkey.clone(),
                                author_key_for_task.clone(),
                            );
                        }
                    }
                }
            }
        });
        self.subscription_registry
            .author_subscriptions
            .lock()
            .await
            .insert(author_key, handle);
        Ok(())
    }
}

/// 自分の replica の follow・block の edge を、背景で小分けに読む task を起動する(#1239)。
fn spawn_own_edge_sweep(services: &ServiceHandles, author_pubkey: &str) -> AbortOnDrop {
    let services = services.clone();
    let author_pubkey = author_pubkey.to_string();
    AbortOnDrop(tokio::spawn(async move {
        if let Err(error) = sweep_own_author_edges(&services, author_pubkey.as_str()).await {
            warn!(
                author_pubkey = %author_pubkey,
                error = %error,
                "failed to read the own follow and block edges"
            );
        }
    }))
}

/// drop されたときに task を止める。親の task が abort されると、その future と一緒に drop される。
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
