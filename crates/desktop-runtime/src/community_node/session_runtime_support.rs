use super::*;

impl DesktopRuntime {
    pub(crate) async fn ensure_community_node_session(
        &self,
        base_url: &str,
    ) -> Result<CommunityNodeSessionOutcome> {
        self.ensure_community_node_session_with_mode(base_url, false)
            .await
    }

    /// scheduler・背景の lane の session。Ready で token が有効な間は、同意と policy を確かめず、期限の来た
    /// 登録だけを送る(#1221 R2-B)。同意と policy は、token の更新・session の確立・利用者の操作
    /// (`ensure_community_node_session`)のときに確かめ、サーバ側の失効はこの要求の 403 で分かる。
    pub(crate) async fn ensure_due_community_node_session(
        &self,
        base_url: &str,
    ) -> Result<CommunityNodeSessionOutcome> {
        let base_url = normalize_http_url(base_url)?;
        let guard = self.community_node_session_guard.lock(&base_url).await;
        if let Some(mut token) = self.ready_community_node_token(base_url.as_str()).await? {
            if !self
                .refresh_community_node_registration_with_token_if_due(
                    base_url.as_str(),
                    &mut token,
                    false,
                )
                .await?
            {
                return self
                    .community_node_consent_required(base_url.as_str())
                    .await;
            }
            // 401 の後の再認証で Authenticating になっていても、確認を済ませたので Ready へ戻す。
            self.set_community_node_session_phase(
                base_url.as_str(),
                CommunityNodeSessionPhase::Ready,
            )
            .await;
            self.note_community_node_token(base_url.as_str(), &token)
                .await;
            return Ok(CommunityNodeSessionOutcome::Ready);
        }
        drop(guard);
        self.ensure_community_node_session(&base_url).await
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
            self.set_community_node_cached_consent(base_url.as_str(), None)
                .await;
            self.set_community_node_local_consent_update_pending(base_url.as_str(), policy_update)
                .await;
            return self
                .community_node_consent_required(base_url.as_str())
                .await;
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
                return self
                    .community_node_consent_required(base_url.as_str())
                    .await;
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
        if !self
            .refresh_community_node_registration_with_token_if_due(
                base_url.as_str(),
                &mut token,
                force_refresh,
            )
            .await?
        {
            return self
                .community_node_consent_required(base_url.as_str())
                .await;
        }
        self.clear_community_node_retry_state(base_url.as_str())
            .await;
        self.set_community_node_session_ready(base_url.as_str(), !was_ready, local_consent)
            .await;
        self.note_community_node_token(base_url.as_str(), &token)
            .await;
        self.apply_ready_community_node_connectivity(base_url.as_str())
            .await?;
        Ok(CommunityNodeSessionOutcome::Ready)
    }

    /// Ready の session の、更新の要らない token。
    async fn ready_community_node_token(
        &self,
        base_url: &str,
    ) -> Result<Option<StoredCommunityNodeToken>> {
        let ready = self
            .community_node_sessions
            .lock()
            .await
            .get(base_url)
            .is_some_and(|session| session.session_phase == CommunityNodeSessionPhase::Ready);
        if !ready {
            return Ok(None);
        }
        let now = Utc::now().timestamp();
        Ok(
            load_community_node_token(&self.db_path, self.identity_mode, base_url)?
                .filter(|token| !Self::community_node_token_requires_refresh(token, now)),
        )
    }

    async fn note_community_node_token(&self, base_url: &str, token: &StoredCommunityNodeToken) {
        if let Some(session) = self.community_node_sessions.lock().await.get_mut(base_url) {
            session.token_refresh_at = token
                .expires_at
                .saturating_sub(COMMUNITY_NODE_AUTH_REFRESH_SKEW_SECONDS);
        }
    }

    /// 同意が要る node を Idle にし、その node の relay と seed を外す。
    pub(crate) async fn community_node_consent_required(
        &self,
        base_url: &str,
    ) -> Result<CommunityNodeSessionOutcome> {
        self.clear_community_node_retry_state(base_url).await;
        self.set_community_node_session_phase(base_url, CommunityNodeSessionPhase::Idle)
            .await;
        self.deactivate_community_node_connectivity(base_url)
            .await?;
        Ok(CommunityNodeSessionOutcome::ConsentRequired)
    }

    pub(crate) async fn apply_ready_community_node_connectivity(
        &self,
        base_url: &str,
    ) -> Result<()> {
        self.sync_community_node_connectivity(base_url).await
    }

    pub(crate) async fn refresh_community_node_registration_if_due(
        &self,
        base_url: &str,
    ) -> Result<()> {
        let base_url = normalize_http_url(base_url)?;
        match self
            .ensure_due_community_node_session(base_url.as_str())
            .await
        {
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

    /// 認証・同意を確かめ済みの node の relay と seed。確かめていない node は `None`。
    async fn community_node_connectivity(&self, base_url: &str) -> Option<NodeConnectivity> {
        let local_consent =
            load_community_node_local_consents(&self.db_path, self.identity_mode, base_url)
                .ok()
                .filter(|consent| consent.has_active_consent())?;
        let verified = self
            .community_node_sessions
            .lock()
            .await
            .get(base_url)
            .is_some_and(|session| {
                !session.local_consent_update_pending
                    && session.current_policy_verified_for.as_ref() == Some(&local_consent)
            });
        if !verified {
            return None;
        }
        let config = self.community_node_config.lock().await;
        let resolved = config
            .nodes
            .iter()
            .find(|node| node.base_url == base_url)?
            .resolved_urls
            .as_ref()?;
        Some(NodeConnectivity {
            relay_urls: resolved.connectivity_urls.clone(),
            seed_peers: resolved
                .seed_peers
                .iter()
                .filter_map(seed_peer_from_community_node)
                .collect(),
        })
    }

    /// その node の relay と seed を、認証・同意の状態に合わせて差分だけ適用する(#1221 R2-B)。
    /// 自分の宛先が変わったら、その node への登録をすぐ送り直す。
    pub(crate) async fn sync_community_node_connectivity(&self, base_url: &str) -> Result<()> {
        let next = self.community_node_connectivity(base_url).await;
        let before = self
            .local_community_node_seed_peer("connectivity-pre-apply")
            .await
            .ok();
        if !self
            .apply_community_node_connectivity(Some((base_url, next)))
            .await?
        {
            return Ok(());
        }
        let after = self
            .local_community_node_seed_peer("connectivity-post-apply")
            .await
            .ok();
        if before != after
            && let Some(entry) = self.community_node_sessions.lock().await.get_mut(base_url)
        {
            entry.heartbeat_deadline = 0;
        }
        Ok(())
    }

    /// 同意の解除・認証の失効・設定からの削除で、その node の relay・seed・rendezvous の候補を外す。
    pub(crate) async fn deactivate_community_node_connectivity(
        &self,
        base_url: &str,
    ) -> Result<()> {
        self.iroh_stack
            .transport
            .clear_receive_candidates(Some(base_url))
            .await?;
        self.apply_community_node_connectivity(Some((base_url, None)))
            .await?;
        Ok(())
    }

    /// node ごとの relay と seed の和を transport・docs・blob へ適用する(#1221 R2-B)。`change` はその node の
    /// 新しい寄与で、和が変わらなければ何もしない。`None` は利用者の seed の設定の変更で、和が同じでも適用し直す。
    /// 適用したら真を返す。endpoint を作り直すのは、既存の endpoint を使えないときだけ(`SharedIrohStack`)。
    pub(crate) async fn apply_community_node_connectivity(
        &self,
        change: Option<(&str, Option<NodeConnectivity>)>,
    ) -> Result<bool> {
        let mut applied = self.community_node_connectivity.lock().await;
        let mut nodes = applied.nodes.clone();
        let forced = change.is_none();
        if let Some((base_url, next)) = change {
            if nodes.get(base_url) == next.as_ref() {
                return Ok(false);
            }
            match next {
                Some(next) => nodes.insert(base_url.to_string(), next),
                None => nodes.remove(base_url),
            };
        }
        let relay_urls = nodes
            .values()
            .flat_map(|node| node.relay_urls.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let seed_peers = normalize_seed_peers(
            nodes
                .values()
                .flat_map(|node| node.seed_peers.iter().cloned())
                .collect(),
        );
        let changed = relay_urls != applied.relay_urls || seed_peers != applied.seed_peers;
        if forced || changed {
            let discovery_config = self.discovery_config.lock().await.clone();
            let generation = self.iroh_stack.generation();
            self.iroh_stack
                .apply_runtime_connectivity(
                    &discovery_config,
                    &seed_peers,
                    TransportRelayConfig {
                        iroh_relay_urls: relay_urls.clone(),
                    },
                )
                .await?;
            if self.iroh_stack.generation() != generation {
                // 旧 stack の stream は終わっている。lease のある key の task だけを作り直す(#1221 R2-C)。
                self.app_service.rebuild_scope_subscriptions().await?;
            }
            debug!(
                relay_url_count = relay_urls.len(),
                bootstrap_seed_peer_count = seed_peers.len(),
                "applied community-node relay and seed connectivity"
            );
        }
        *applied = AppliedConnectivity {
            nodes,
            relay_urls,
            seed_peers,
        };
        Ok(forced || changed)
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
        // 確かめ済みの relay のうち、まだ endpoint へ入っていないものがある。
        let restart_required = match self.community_node_connectivity(&node.base_url).await {
            Some(desired) => {
                let applied = self.community_node_connectivity.lock().await;
                desired
                    .relay_urls
                    .iter()
                    .any(|url| !applied.relay_urls.contains(url))
            }
            None => false,
        };
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
            restart_required,
        })
    }
}
