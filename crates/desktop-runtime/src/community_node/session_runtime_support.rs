use super::*;

impl DesktopRuntime {
    pub(crate) async fn ensure_community_node_session(
        &self,
        base_url: &str,
    ) -> Result<CommunityNodeSessionOutcome> {
        self.ensure_community_node_session_with_mode(base_url, false)
            .await
    }

    pub(crate) async fn ensure_community_node_session_with_mode(
        &self,
        base_url: &str,
        force_refresh: bool,
    ) -> Result<CommunityNodeSessionOutcome> {
        let base_url = normalize_http_url(base_url)?;
        let _guard = self.community_node_session_guard.lock(&base_url).await;
        let preflight = self
            .preflight_community_node_consent(base_url.as_str())
            .await?;
        let base_url = preflight.base_url().to_string();
        let local_consent = preflight.local_consent().clone();
        if let CommunityNodeConsentPreflight::Required { policy_update, .. } = preflight {
            self.clear_community_node_retry_state(base_url.as_str())
                .await;
            self.set_community_node_cached_consent(base_url.as_str(), None)
                .await;
            self.set_community_node_local_consent_update_pending(base_url.as_str(), policy_update)
                .await;
            self.set_community_node_session_phase(
                base_url.as_str(),
                CommunityNodeSessionPhase::Idle,
            )
            .await;
            self.deactivate_community_node_connectivity(base_url.as_str())
                .await?;
            return Ok(CommunityNodeSessionOutcome::ConsentRequired);
        }
        self.set_community_node_local_consent_update_pending(base_url.as_str(), false)
            .await;

        let now = Utc::now().timestamp();
        let session_gate = self
            .community_node_sessions
            .lock()
            .await
            .get(base_url.as_str())
            .map(|s| (s.session_retry_deadline, s.session_phase));
        if session_gate
            .is_some_and(|(_, phase)| phase == CommunityNodeSessionPhase::AwaitingAdmission)
        {
            return Ok(CommunityNodeSessionOutcome::Deferred(
                CommunityNodeSessionPhase::AwaitingAdmission,
            ));
        }
        let retry_after = session_gate.map(|(retry_after, _)| retry_after);
        if !force_refresh && retry_after.is_some_and(|retry_after| retry_after > now) {
            self.set_community_node_session_phase(
                base_url.as_str(),
                CommunityNodeSessionPhase::Retrying,
            )
            .await;
            return Ok(CommunityNodeSessionOutcome::Deferred(
                CommunityNodeSessionPhase::Retrying,
            ));
        }

        let was_ready = self
            .community_node_session_was_ready(base_url.as_str())
            .await;
        self.set_community_node_session_phase(
            base_url.as_str(),
            CommunityNodeSessionPhase::Connecting,
        )
        .await;
        let mut token =
            load_community_node_token(&self.db_path, self.identity_mode, base_url.as_str())?;

        if token
            .as_ref()
            .is_none_or(|token| Self::community_node_token_requires_refresh(token, now))
        {
            self.set_community_node_session_phase(
                base_url.as_str(),
                CommunityNodeSessionPhase::Authenticating,
            )
            .await;
            token = Some(
                self.request_community_node_authentication_token(base_url.as_str())
                    .await?,
            );
        }

        let mut token = token.expect("token must exist after authentication");
        let consent_status = self
            .fetch_community_node_consent_status_with_retry(base_url.as_str(), &mut token, true)
            .await?;
        self.set_community_node_cached_consent(base_url.as_str(), Some(consent_status.clone()))
            .await;
        if !consent_status.all_required_accepted {
            if !community_node_local_consent_covers_status(&local_consent, &consent_status) {
                // ローカル同意が現行版をカバーしない = 重要変更の再同意待ち。黙って
                // 再受諾せず、UI が本文を再提示するまでセッションを進めない。
                self.set_community_node_local_consent_update_pending(base_url.as_str(), true)
                    .await;
                self.clear_community_node_retry_state(base_url.as_str())
                    .await;
                self.set_community_node_session_phase(
                    base_url.as_str(),
                    CommunityNodeSessionPhase::Idle,
                )
                .await;
                self.deactivate_community_node_connectivity(base_url.as_str())
                    .await?;
                return Ok(CommunityNodeSessionOutcome::ConsentRequired);
            }
            // ローカル同意済みの内容をサーバ記録へ同期する(#857)。
            self.set_community_node_session_phase(
                base_url.as_str(),
                CommunityNodeSessionPhase::Accepting,
            )
            .await;
            let accepted = self
                .accept_community_node_consents_with_retry(
                    base_url.as_str(),
                    &mut token,
                    &[],
                    consent_status.policy_snapshot_revision.as_deref(),
                )
                .await?;
            self.set_community_node_cached_consent(base_url.as_str(), Some(accepted))
                .await;
        }
        self.set_community_node_local_consent_update_pending(base_url.as_str(), false)
            .await;

        self.set_community_node_session_phase(
            base_url.as_str(),
            CommunityNodeSessionPhase::Refreshing,
        )
        .await;
        self.refresh_community_node_registration_with_token_if_due(
            base_url.as_str(),
            &mut token,
            force_refresh,
        )
        .await?;
        self.clear_community_node_retry_state(base_url.as_str())
            .await;
        self.set_community_node_session_ready(base_url.as_str(), !was_ready, local_consent)
            .await;
        self.apply_ready_community_node_connectivity(base_url.as_str())
            .await?;
        Ok(CommunityNodeSessionOutcome::Ready)
    }

    pub(crate) async fn apply_ready_community_node_connectivity(
        &self,
        base_url: &str,
    ) -> Result<()> {
        let local_seed_peer_before = self
            .local_community_node_seed_peer("ready-connectivity-pre-apply")
            .await
            .ok();
        self.apply_runtime_connectivity_assist().await?;
        self.apply_effective_seed_peers().await?;
        let local_seed_peer_after = self
            .local_community_node_seed_peer("ready-connectivity-post-apply")
            .await
            .ok();
        if local_seed_peer_before != local_seed_peer_after
            && let Some(entry) = self.community_node_sessions.lock().await.get_mut(base_url)
        {
            entry.heartbeat_deadline = 0;
        }
        Ok(())
    }

    pub(crate) async fn refresh_community_node_registration_if_due(
        &self,
        base_url: &str,
    ) -> Result<()> {
        let base_url = normalize_http_url(base_url)?;
        match self.ensure_community_node_session(base_url.as_str()).await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(rejection) = Self::community_node_admission_rejection(&error).cloned() {
                    self.set_community_node_admission_rejection(base_url.as_str(), rejection)
                        .await;
                } else {
                    self.set_community_node_retry_state(base_url.as_str(), error)
                        .await;
                }
                Ok(())
            }
        }
    }

    pub(crate) async fn require_community_node(
        &self,
        base_url: &str,
    ) -> Result<CommunityNodeNodeConfig> {
        self.community_node_config
            .lock()
            .await
            .nodes
            .iter()
            .find(|node| node.base_url == base_url)
            .cloned()
            .ok_or_else(|| anyhow!("community node `{base_url}` is not configured"))
    }

    async fn active_community_node_connectivity_config(&self) -> CommunityNodeConfig {
        let config = self.community_node_config.lock().await.clone();
        let mut active = community_node_config_with_active_local_consents(
            &self.db_path,
            self.identity_mode,
            &config,
        );
        let sessions = self.community_node_sessions.lock().await;
        active.nodes.retain(|node| {
            let Ok(local_consent) = load_community_node_local_consents(
                &self.db_path,
                self.identity_mode,
                node.base_url.as_str(),
            ) else {
                return false;
            };
            sessions.get(node.base_url.as_str()).is_some_and(|session| {
                !session.local_consent_update_pending
                    && session.current_policy_verified_for.as_ref() == Some(&local_consent)
            })
        });
        active
    }

    async fn active_community_node_rendezvous_seed_peers(
        &self,
        active_config: &CommunityNodeConfig,
    ) -> Vec<SeedPeer> {
        let rendezvous_by_node = self.community_node_rendezvous_seed_peers.lock().await;
        rendezvous_by_node
            .iter()
            .filter(|(base_url, _)| {
                active_config
                    .nodes
                    .iter()
                    .any(|node| node.base_url.as_str() == base_url.as_str())
            })
            .flat_map(|(_, peers)| peers.iter().cloned())
            .collect()
    }

    pub(crate) async fn deactivate_community_node_connectivity(
        &self,
        base_url: &str,
    ) -> Result<()> {
        self.iroh_stack
            .transport
            .clear_receive_candidates(Some(base_url))
            .await?;
        self.community_node_rendezvous_seed_peers
            .lock()
            .await
            .remove(base_url);
        self.apply_runtime_connectivity_assist().await?;
        self.apply_effective_seed_peers().await
    }

    pub(crate) async fn community_node_status(
        &self,
        node: CommunityNodeNodeConfig,
        consent_state: Option<CommunityNodeConsentStatus>,
        last_error: Option<String>,
    ) -> Result<CommunityNodeNodeStatus> {
        let now = Utc::now().timestamp();
        let local_consent = load_community_node_local_consents(
            &self.db_path,
            self.identity_mode,
            node.base_url.as_str(),
        )?;
        let sessions = self.community_node_sessions.lock().await;
        let session = sessions.get(node.base_url.as_str());
        let consent_state =
            consent_state.or_else(|| session.and_then(|s| s.cached_consent.clone()));
        let last_error = last_error.or_else(|| session.and_then(|s| s.last_error.clone()));
        let admission_rejection = session.and_then(|session| session.admission_rejection.clone());
        let consent_update_pending = session
            .map(|session| session.local_consent_update_pending)
            .unwrap_or(false);
        let retry_after = session
            .map(|s| s.session_retry_deadline)
            .filter(|deadline| *deadline > now);
        let session_phase = session
            .map(|s| s.session_phase)
            .unwrap_or(CommunityNodeSessionPhase::Idle);
        let current_policy_verified = session.is_some_and(|session| {
            !session.local_consent_update_pending
                && session.current_policy_verified_for.as_ref() == Some(&local_consent)
        });
        drop(sessions);
        // status生成自体が、retry／参加承認待ちでpreflightを終えた後のtoken読込を
        // 迂回させない。認証状態はcurrent policy照合済みevidenceがある場合だけ復元する。
        let token = if current_policy_verified {
            load_community_node_token(&self.db_path, self.identity_mode, node.base_url.as_str())?
        } else {
            None
        };
        let auth_state = match token {
            Some(token) if token.expires_at > now => CommunityNodeAuthState {
                authenticated: true,
                expires_at: Some(token.expires_at),
            },
            Some(token) => CommunityNodeAuthState {
                authenticated: false,
                expires_at: Some(token.expires_at),
            },
            None => CommunityNodeAuthState::default(),
        };
        let invite_code_saved = load_community_node_invite_code(
            &self.db_path,
            self.identity_mode,
            node.base_url.as_str(),
        )?
        .is_some();
        let active_config = self.active_community_node_connectivity_config().await;
        let current_connectivity_urls =
            relay_config_from_community_node_config(&active_config).iroh_relay_urls;
        Ok(CommunityNodeNodeStatus {
            base_url: node.base_url,
            auth_state,
            consent_state,
            local_consent,
            consent_update_pending,
            resolved_urls: node.resolved_urls,
            last_error,
            invite_code_saved,
            admission_rejection,
            session_phase,
            retry_after,
            restart_required: current_connectivity_urls
                != *self.active_connectivity_urls.lock().await,
        })
    }

    async fn apply_runtime_connectivity_assist_with_mode(&self, force: bool) -> Result<()> {
        let _apply = self.community_node_connectivity_guard.lock().await;
        let discovery_config = self.discovery_config.lock().await.clone();
        let community_node_config = self.active_community_node_connectivity_config().await;
        let mut next_state =
            runtime_connectivity_assist_state(&discovery_config, &community_node_config);
        let rendezvous_seed_peers = self
            .active_community_node_rendezvous_seed_peers(&community_node_config)
            .await;
        next_state.bootstrap_seed_peers = normalize_seed_peers(
            next_state
                .bootstrap_seed_peers
                .into_iter()
                .chain(rendezvous_seed_peers)
                .collect(),
        );
        if !force {
            let unchanged = self
                .last_runtime_connectivity_assist_state
                .lock()
                .await
                .as_ref()
                == Some(&next_state);
            if unchanged && self.iroh_stack.local_docs_available().await? {
                debug!(
                    relay_url_count = next_state.relay_urls.len(),
                    bootstrap_seed_peer_count = next_state.bootstrap_seed_peers.len(),
                    "skipping runtime connectivity apply because relay and seed inputs are unchanged"
                );
                return Ok(());
            }
        }
        let relay_config = TransportRelayConfig {
            iroh_relay_urls: next_state.relay_urls.clone(),
        };
        let generation = self.iroh_stack.generation();
        self.iroh_stack
            .apply_runtime_connectivity(
                &discovery_config,
                &next_state.bootstrap_seed_peers,
                relay_config.clone(),
            )
            .await?;
        if self.iroh_stack.generation() != generation {
            // 新しい stack へ seed を設定し直す(入力が同じでも)。
            *self.last_effective_seed_peer_apply_state.lock().await = None;
            // 旧 stack の stream は終わっている。lease のある key の task だけを作り直す(#1221 R2-C)。
            self.app_service.rebuild_scope_subscriptions().await?;
        }
        debug!(
            relay_url_count = relay_config.iroh_relay_urls.len(),
            bootstrap_seed_peer_count = next_state.bootstrap_seed_peers.len(),
            "applied runtime connectivity assist from community-node metadata"
        );
        *self.active_connectivity_urls.lock().await = relay_config.iroh_relay_urls;
        *self.last_runtime_connectivity_assist_state.lock().await = Some(next_state);
        self.runtime_connectivity_apply_version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub(crate) async fn apply_runtime_connectivity_assist(&self) -> Result<()> {
        self.apply_runtime_connectivity_assist_with_mode(false)
            .await
    }

    pub(crate) async fn force_apply_runtime_connectivity_assist(&self) -> Result<()> {
        self.apply_runtime_connectivity_assist_with_mode(true).await
    }

    pub(crate) async fn force_rebuild_runtime_connectivity_assist(&self) -> Result<()> {
        let _apply = self.community_node_connectivity_guard.lock().await;
        let discovery_config = self.discovery_config.lock().await.clone();
        let community_node_config = self.active_community_node_connectivity_config().await;
        let mut next_state =
            runtime_connectivity_assist_state(&discovery_config, &community_node_config);
        let rendezvous_seed_peers = self
            .active_community_node_rendezvous_seed_peers(&community_node_config)
            .await;
        next_state.bootstrap_seed_peers = normalize_seed_peers(
            next_state
                .bootstrap_seed_peers
                .into_iter()
                .chain(rendezvous_seed_peers)
                .collect(),
        );
        let relay_config = TransportRelayConfig {
            iroh_relay_urls: next_state.relay_urls.clone(),
        };
        self.iroh_stack
            .force_rebuild_runtime_connectivity(
                &discovery_config,
                &next_state.bootstrap_seed_peers,
                relay_config.clone(),
            )
            .await?;
        self.app_service.rebuild_scope_subscriptions().await?;
        debug!(
            relay_url_count = relay_config.iroh_relay_urls.len(),
            bootstrap_seed_peer_count = next_state.bootstrap_seed_peers.len(),
            "force rebuilt runtime connectivity assist from community-node metadata"
        );
        *self.active_connectivity_urls.lock().await = relay_config.iroh_relay_urls;
        *self.last_runtime_connectivity_assist_state.lock().await = Some(next_state);
        self.runtime_connectivity_apply_version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn apply_effective_seed_peers_with_mode(&self, force: bool) -> Result<()> {
        let _apply = self.community_node_connectivity_guard.lock().await;
        let discovery_config = self.discovery_config.lock().await.clone();
        let community_node_config = self.active_community_node_connectivity_config().await;
        let mut next_state =
            effective_seed_peer_apply_state(&discovery_config, &community_node_config);
        let rendezvous_seed_peers = self
            .active_community_node_rendezvous_seed_peers(&community_node_config)
            .await;
        next_state.bootstrap_seed_peers = normalize_seed_peers(
            next_state
                .bootstrap_seed_peers
                .into_iter()
                .chain(rendezvous_seed_peers)
                .collect(),
        );
        if !force {
            let current_state = self.last_effective_seed_peer_apply_state.lock().await;
            if current_state.as_ref() == Some(&next_state) {
                debug!(
                    bootstrap_seed_peer_count = next_state.bootstrap_seed_peers.len(),
                    configured_seed_peer_count = next_state.configured_seed_peers.len(),
                    "skipping discovery seed apply because the effective seed inputs are unchanged"
                );
                return Ok(());
            }
        }
        self.app_service
            .set_discovery_seeds(
                next_state.discovery_mode.clone(),
                next_state.discovery_env_locked,
                next_state.configured_seed_peers.clone(),
                next_state.bootstrap_seed_peers.clone(),
            )
            .await?;
        debug!(
            bootstrap_seed_peer_count = next_state.bootstrap_seed_peers.len(),
            configured_seed_peer_count = next_state.configured_seed_peers.len(),
            "applied effective discovery seeds from community-node metadata"
        );
        *self.last_effective_seed_peer_apply_state.lock().await = Some(next_state);
        self.effective_seed_peer_apply_version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub(crate) async fn apply_effective_seed_peers(&self) -> Result<()> {
        self.apply_effective_seed_peers_with_mode(false).await
    }

    pub(crate) async fn force_apply_effective_seed_peers(&self) -> Result<()> {
        self.apply_effective_seed_peers_with_mode(true).await
    }
}
