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

    pub(crate) async fn spawn_author_subscription(&self, author_pubkey: &str) -> Result<ScopeTask> {
        let services = self.services.clone();
        let last_sync = Arc::clone(&self.last_sync_ts);
        let notification_inserted = Arc::clone(&self.notification_inserted_notify);
        let author_key = normalize_author_pubkey(author_pubkey)?;
        let local_author_pubkey = self.current_author_pubkey();
        let replica = author_replica_id(author_key.as_str());
        services.docs_sync.open_replica(&replica).await?;
        let mut doc_stream = services
            .docs_sync
            .subscribe_replica_notices(&replica)
            .await?;
        let author_key_for_task = author_key.clone();
        let replica_for_owner = replica.clone();
        let handle = tokio::spawn(async move {
            let store = &services.store;
            let projection_store = &services.projection_store;
            let docs_sync = &services.docs_sync;
            let blob_service = &services.blob_service;
            let notification_baseline = match snapshot_follow_notification_baseline(
                docs_sync.as_ref(),
                &replica,
                local_author_pubkey.as_str(),
                known_docs_author(
                    &services,
                    local_author_pubkey.as_str(),
                    author_key_for_task.as_str(),
                )
                .await
                .ok()
                .flatten()
                .as_deref(),
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
            let bootstrap_services = services.clone();
            let bootstrap_last_sync = Arc::clone(&last_sync);
            let bootstrap_local_author_pubkey = local_author_pubkey.clone();
            let bootstrap_author_pubkey = author_key_for_task.clone();
            // 購読タスクが止まると、この task も止まる(#1239。切り離すと、購読の後にも読み出しが続く)。
            // 手元の状態を先に反映し、手元に無い現在値の key だけを有界な provider から読む(#1221 R5-C)。
            let _bootstrap = AbortOnDrop(tokio::spawn(async move {
                match hydrate_author_state(
                    &bootstrap_services,
                    bootstrap_local_author_pubkey.as_str(),
                    bootstrap_author_pubkey.as_str(),
                    DocFetchPolicy::LocalThenRemote,
                )
                .await
                {
                    Ok(initial_count) if initial_count > 0 => {
                        *bootstrap_last_sync.lock().await = Some(Utc::now().timestamp_millis());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(
                            author_pubkey = %bootstrap_author_pubkey,
                            error = %error,
                            "failed to hydrate author state during bootstrap"
                        );
                    }
                }
            }));
            // #1239: replica は走査しない。docs の event はその key だけを反映する。取りこぼしと同期の区切りでは、
            // 自分を指す follow・block の key だけを読み直す(`catch_up_author_state`)。間隔を空けて 1 回にまとめる。
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
                        }
                    }
                }
            }
        });
        Ok(ScopeTask {
            handle: AbortOnDropTask::new(handle),
            replica: replica_for_owner,
            hint_topic: None,
        })
    }
}

/// drop されたときに task を止める。親の task が abort されると、その future と一緒に drop される。
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
