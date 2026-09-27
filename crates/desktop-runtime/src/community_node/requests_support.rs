use super::*;

impl DesktopRuntime {
    async fn refresh_rendezvous_with_token(
        &self,
        base_url: &str,
        access_token: &str,
    ) -> std::result::Result<(), CommunityNodeRequestError> {
        if let Err(error) = self
            .refresh_account_receive_rendezvous_with_token(base_url, access_token)
            .await
        {
            if matches!(
                error,
                CommunityNodeRequestError::AuthRequired
                    | CommunityNodeRequestError::ConsentRequired
            ) {
                return Err(error);
            }
            if let Some(session) = self.community_node_sessions.lock().await.get_mut(base_url) {
                session.rendezvous_refresh_deadline = Utc::now().timestamp().saturating_add(5);
            }
            debug!(
                ?error,
                "bounded account receive rendezvous refresh deferred"
            );
        }
        self.refresh_topic_rendezvous_with_token(base_url, access_token)
            .await
    }

    pub(crate) async fn request_community_node_authentication_token(
        &self,
        base_url: &str,
    ) -> Result<StoredCommunityNodeToken> {
        let base_url = normalize_http_url(base_url)?;
        let client = community_node_http_client()?;
        let challenge_url = format!("{base_url}{AUTH_CHALLENGE_PATH}");
        let pubkey = self.author_keys.public_key_hex();
        let seed_peer = self.local_community_node_seed_peer("auth").await?;
        let challenge = client
            .post(challenge_url)
            .json(&AuthChallengeRequest { pubkey })
            .send()
            .await
            .context("failed to request auth challenge")?
            .error_for_status()
            .context("auth challenge request failed")?
            .json::<AuthChallengeResponse>()
            .await
            .context("failed to decode auth challenge response")?;

        // resolved_urls 未解決時に base_url で代用するのは互換パスではなく恒常経路
        // (REFACTORING.md「互換パスと sunset 条件」の再分類参照)。ノードを新規追加した
        // 直後は必ず未解決(None)であり、この代用が初回の認証を支えている。削除対象ではない。
        let public_base_url = self
            .community_node_config
            .lock()
            .await
            .nodes
            .iter()
            .find(|node| node.base_url == base_url)
            .and_then(|node| {
                node.resolved_urls
                    .as_ref()
                    .map(|resolved| resolved.public_base_url.clone())
            })
            .unwrap_or_else(|| base_url.clone());
        let auth_envelope_json = build_auth_envelope_json(
            self.author_keys.as_ref(),
            challenge.challenge.as_str(),
            public_base_url.as_str(),
        )?;
        let verify_url = format!("{base_url}{AUTH_VERIFY_PATH}");
        let invite_code =
            load_community_node_invite_code(&self.db_path, self.identity_mode, base_url.as_str())?;
        let verify_response = client
            .post(verify_url)
            .json(&AuthVerifyRequest {
                auth_envelope_json,
                endpoint_id: Some(seed_peer.endpoint_id),
                addr_hint: seed_peer.addr_hint,
                invite_code,
            })
            .send()
            .await
            .context("failed to verify auth envelope")?;
        if verify_response.status() == StatusCode::FORBIDDEN {
            let body = verify_response
                .json::<ApiErrorBody>()
                .await
                .context("failed to decode auth verify rejection response")?;
            if let Some(code) =
                CommunityNodeAdmissionRejectionCode::from_wire_code(body.code.as_str())
            {
                return Err(CommunityNodeAdmissionRejection {
                    code,
                    message: body.message,
                }
                .into());
            }
            return Err(anyhow!(
                "auth verify request was rejected with unknown code `{}`: {}",
                body.code,
                body.message
            ));
        }
        let verify = verify_response
            .error_for_status()
            .context("auth verify request failed")?
            .json::<AuthVerifyResponse>()
            .await
            .context("failed to decode auth verify response")?;
        let token = StoredCommunityNodeToken {
            access_token: verify.access_token,
            expires_at: verify.expires_at,
        };
        persist_community_node_token(&self.db_path, self.identity_mode, base_url.as_str(), &token)?;
        Ok(token)
    }

    /// 401 の後の再認証。失敗したら認証の失効として、その node の relay と seed を外す(#1221 R2-B)。
    /// 5xx・通信失敗などの一時的な失敗だけでは外さない。
    async fn reauthenticate_community_node(
        &self,
        base_url: &str,
    ) -> Result<StoredCommunityNodeToken> {
        self.set_community_node_session_phase(base_url, CommunityNodeSessionPhase::Authenticating)
            .await;
        let result = self
            .request_community_node_authentication_token(base_url)
            .await;
        if result.is_err() {
            if let Some(session) = self.community_node_sessions.lock().await.get_mut(base_url) {
                session.current_policy_verified_for = None;
            }
            self.deactivate_community_node_connectivity(base_url)
                .await?;
        }
        result
    }

    /// 認証不要の公開 policy カタログ取得(#857)。同意判断に必要な文書一覧・本文・版
    /// のみで、Node 同意前に許可される通信(公開 manifest / 法務文書)に含まれる。
    pub(crate) async fn request_community_node_policies(
        &self,
        base_url: &str,
        language: Option<&str>,
    ) -> Result<CommunityNodePoliciesResponse> {
        let base_url = normalize_http_url(base_url)?;
        let client = community_node_http_client()?;
        let mut request = client.get(format!("{base_url}{POLICIES_PATH}"));
        if let Some(language) = language.filter(|value| !value.trim().is_empty()) {
            request = request.query(&[("language", language)]);
        }
        request
            .send()
            .await
            .context("failed to fetch community node policies")?
            .error_for_status()
            .context("community node policies request failed")?
            .json::<CommunityNodePoliciesResponse>()
            .await
            .context("failed to decode community node policies")
    }

    async fn request_community_node_consent_status(
        &self,
        base_url: &str,
        access_token: &str,
    ) -> std::result::Result<CommunityNodeConsentStatus, CommunityNodeRequestError> {
        let client = community_node_http_client().map_err(CommunityNodeRequestError::Other)?;
        let response = client
            .get(format!("{base_url}{CONSENTS_STATUS_PATH}"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to fetch community node consent status",
                    error,
                )
            })?;
        let response = response.error_for_status().map_err(|error| {
            Self::map_community_node_status_error(
                "community node consent status request failed",
                error,
            )
        })?;
        response
            .json::<CommunityNodeConsentStatus>()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to decode community node consent status",
                    error,
                )
            })
    }

    async fn request_accept_community_node_consents(
        &self,
        base_url: &str,
        access_token: &str,
        policy_slugs: &[String],
        policy_snapshot_revision: Option<&str>,
    ) -> std::result::Result<CommunityNodeConsentStatus, CommunityNodeRequestError> {
        let client = community_node_http_client().map_err(CommunityNodeRequestError::Other)?;
        let response = client
            .post(format!("{base_url}{CONSENTS_PATH}"))
            .bearer_auth(access_token)
            .json(&AcceptConsentsRequest {
                policy_slugs: policy_slugs.to_vec(),
                policy_snapshot_revision: policy_snapshot_revision.map(str::to_owned),
            })
            .send()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to accept community node consents",
                    error,
                )
            })?;
        let response = response.error_for_status().map_err(|error| {
            Self::map_community_node_status_error(
                "community node consent accept request failed",
                error,
            )
        })?;
        response
            .json::<CommunityNodeConsentStatus>()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to decode accepted community node consents",
                    error,
                )
            })
    }

    async fn sync_community_node_bootstrap_metadata(
        &self,
        base_url: &str,
        access_token: &str,
    ) -> std::result::Result<CommunityNodeNodeConfig, CommunityNodeRequestError> {
        let base_url = normalize_http_url(base_url).map_err(CommunityNodeRequestError::Other)?;
        self.require_community_node(&base_url)
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        let client = community_node_http_client().map_err(CommunityNodeRequestError::Other)?;
        let response = client
            .get(format!("{base_url}{BOOTSTRAP_NODES_PATH}"))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to refresh community node metadata",
                    error,
                )
            })?;
        let bootstrap = response
            .error_for_status()
            .map_err(|error| {
                Self::map_community_node_status_error(
                    "community node bootstrap request failed",
                    error,
                )
            })?
            .json::<BootstrapNodesResponse>()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to decode community node bootstrap response",
                    error,
                )
            })?;
        let resolved_urls = bootstrap
            .nodes
            .iter()
            .find(|node| node.base_url == base_url)
            .map(|node| node.resolved_urls.clone())
            .ok_or_else(|| {
                CommunityNodeRequestError::Other(anyhow!(
                    "community node bootstrap response is missing self metadata"
                ))
            })?;
        debug!(
            %base_url,
            relay_url_count = resolved_urls.connectivity_urls.len(),
            seed_peer_count = resolved_urls.seed_peers.len(),
            "community-node metadata sync resolved bootstrap metadata"
        );
        // Other nodes can now refresh concurrently. Merge only this node into
        // the latest config, and never resurrect a node removed during the I/O.
        let mut current_config = self.community_node_config.lock().await;
        let mut next_config = current_config.clone();
        let index = next_config
            .nodes
            .iter()
            .position(|node| node.base_url == base_url)
            .ok_or_else(|| {
                CommunityNodeRequestError::Other(anyhow!(
                    "community node was removed during metadata refresh"
                ))
            })?;
        next_config.nodes[index].resolved_urls = Some(
            refresh_community_node_resolved_urls(
                next_config.nodes[index].resolved_urls.clone(),
                resolved_urls,
            )
            .map_err(CommunityNodeRequestError::Other)?,
        );
        let normalized = normalize_community_node_config(next_config)
            .map_err(CommunityNodeRequestError::Other)?;
        save_community_node_config(&self.db_path, &normalized)
            .map_err(CommunityNodeRequestError::Other)?;
        *current_config = normalized.clone();
        drop(current_config);
        self.sync_community_node_connectivity(base_url.as_str())
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        normalized
            .nodes
            .iter()
            .find(|node| node.base_url == base_url)
            .cloned()
            .ok_or_else(|| {
                CommunityNodeRequestError::Other(anyhow!(
                    "community node `{base_url}` disappeared after normalization"
                ))
            })
    }

    pub(crate) async fn community_node_bootstrap_metadata_retry_due(
        &self,
        base_url: &str,
        now: i64,
    ) -> bool {
        let seed_peers_empty = self
            .community_node_config
            .lock()
            .await
            .nodes
            .iter()
            .find(|node| node.base_url == base_url)
            .and_then(|node| node.resolved_urls.as_ref())
            .is_none_or(|resolved_urls| resolved_urls.seed_peers.is_empty());
        let mut sessions = self.community_node_sessions.lock().await;
        if !seed_peers_empty {
            if let Some(entry) = sessions.get_mut(base_url) {
                entry.metadata_refresh_deadline = 0;
            }
            return false;
        }
        let next_due_at = sessions
            .get(base_url)
            .map(|s| s.metadata_refresh_deadline)
            .unwrap_or_default();
        next_due_at <= now
    }

    pub(crate) async fn record_community_node_bootstrap_metadata_refresh(
        &self,
        base_url: &str,
        seed_peers_empty: bool,
        now: i64,
    ) {
        let mut sessions = self.community_node_sessions.lock().await;
        let entry = sessions
            .entry(base_url.to_string())
            .or_insert_with(CommunityNodeSessionState::default);
        if seed_peers_empty {
            entry.metadata_refresh_deadline =
                now.saturating_add(COMMUNITY_NODE_BOOTSTRAP_METADATA_RETRY_SECONDS);
        } else {
            entry.metadata_refresh_deadline = 0;
        }
    }

    pub(crate) async fn local_community_node_seed_peer(
        &self,
        operation: &str,
    ) -> Result<CommunityNodeSeedPeer> {
        let publish_addr_hint = self.should_publish_community_node_addr_hint().await;
        match self.local_peer_ticket().await {
            Ok(Some(ticket)) => {
                let mut seed_peer = parse_seed_peer(ticket.as_str()).with_context(|| {
                    format!(
                        "failed to derive local seed peer from ticket for community node {operation}"
                    )
                })?;
                if !publish_addr_hint {
                    seed_peer.addr_hint = None;
                }
                return CommunityNodeSeedPeer::new(seed_peer.endpoint_id, seed_peer.addr_hint);
            }
            Ok(None) => {}
            Err(error) => {
                debug!(
                    operation,
                    error = %error,
                    "local peer ticket unavailable; registering community-node endpoint without addr_hint"
                );
            }
        }

        let endpoint_id = self
            .iroh_stack
            .transport
            .discovery()
            .await
            .with_context(|| {
                format!("failed to read local endpoint id for community node {operation}")
            })?
            .local_endpoint_id;
        CommunityNodeSeedPeer::new(endpoint_id, None)
    }

    async fn should_publish_community_node_addr_hint(&self) -> bool {
        true
    }

    async fn refresh_topic_rendezvous_with_token(
        &self,
        base_url: &str,
        access_token: &str,
    ) -> std::result::Result<(), CommunityNodeRequestError> {
        // 購読している topic だけを読む(通信状態の全体は作らない。#1221 R2-D)。
        let subscribed_topics = self
            .iroh_stack
            .transport
            .subscribed_topics()
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        let private_topic_keys = self.app_service.private_channel_rendezvous_keys().await;
        let account_receive_route = receive_route_for_account(&self.author_keys.public_key())
            .map_err(CommunityNodeRequestError::Other)?;
        // rendezvous の鍵 → 購読している topic。応答の peer は、その topic だけへ join する(#1221 R2-B)。
        let mut topic_keys = std::collections::BTreeMap::new();
        let mut skipped_private_topics = 0usize;
        for topic in &subscribed_topics {
            if topic == account_receive_route.as_str() {
                continue;
            }
            let is_private_channel_hint = topic
                .strip_prefix(HINT_TOPIC_PREFIX)
                .is_some_and(|topic| topic.starts_with(PRIVATE_CHANNEL_TOPIC_PREFIX));
            if is_private_channel_hint {
                if let Some(key) = private_topic_keys.get(topic) {
                    topic_keys.insert(key.clone(), topic.clone());
                } else {
                    skipped_private_topics += 1;
                }
                continue;
            }
            topic_keys.insert(
                public_topic_rendezvous_key(&TopicId::new(topic.clone())),
                topic.clone(),
            );
        }
        if skipped_private_topics > 0 {
            warn!(
                skipped_private_topics,
                "現在世代の秘密がない非公開チャンネルのランデブー話題を除外しました"
            );
        }
        if topic_keys.is_empty() {
            return Ok(());
        }
        let seed_peer = self
            .local_community_node_seed_peer("topic-rendezvous")
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        let client = community_node_http_client().map_err(CommunityNodeRequestError::Other)?;
        let response = client
            .post(format!("{base_url}{TOPIC_RENDEZVOUS_HEARTBEAT_PATH}"))
            .bearer_auth(access_token)
            .json(&TopicRendezvousHeartbeat {
                endpoint_id: seed_peer.endpoint_id,
                addr_hint: seed_peer.addr_hint,
                joins: Vec::new(),
                refreshes: topic_keys.keys().cloned().collect(),
                leaves: Vec::new(),
            })
            .send()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to refresh community node topic rendezvous",
                    error,
                )
            })?;
        let response = response.error_for_status().map_err(|error| {
            Self::map_community_node_status_error(
                "community node topic rendezvous request failed",
                error,
            )
        })?;
        let response = response
            .json::<TopicRendezvousHeartbeatResponse>()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to decode community node topic rendezvous response",
                    error,
                )
            })?;
        // 便乗・独立どちらの呼び出しでも、サーバ返却 TTL − マージンで次回期限を更新する。
        // heartbeat の「expires_at − マージン」(下の heartbeat_deadline)と同型(#572)。
        self.community_node_sessions
            .lock()
            .await
            .entry(base_url.to_string())
            .or_insert_with(CommunityNodeSessionState::default)
            .rendezvous_refresh_deadline = Utc::now().timestamp().saturating_add(
            (response.expires_in_seconds.min(i64::MAX as u64) as i64)
                .saturating_sub(COMMUNITY_NODE_TOPIC_RENDEZVOUS_REFRESH_MARGIN_SECONDS),
        );
        for response_topic in response.topics {
            let Some(topic) = topic_keys.get(&response_topic.topic_key) else {
                continue;
            };
            let peers = response_topic
                .peers
                .into_iter()
                .map(|peer| SeedPeer {
                    endpoint_id: peer.endpoint_id,
                    addr_hint: peer.addr_hint,
                })
                .collect();
            self.iroh_stack
                .transport
                .join_topic_peers(base_url, &TopicId::new(topic.clone()), peers)
                .await
                .map_err(CommunityNodeRequestError::Other)?;
        }
        Ok(())
    }

    pub(crate) async fn fetch_community_node_consent_status_with_retry(
        &self,
        base_url: &str,
        token: &mut StoredCommunityNodeToken,
        allow_reauthenticate: bool,
    ) -> Result<CommunityNodeConsentStatus> {
        match self
            .request_community_node_consent_status(base_url, token.access_token.as_str())
            .await
        {
            Ok(status) => Ok(status),
            Err(CommunityNodeRequestError::AuthRequired) if allow_reauthenticate => {
                *token = self.reauthenticate_community_node(base_url).await?;
                self.request_community_node_consent_status(base_url, token.access_token.as_str())
                    .await
                    .map_err(CommunityNodeRequestError::into_anyhow)
            }
            Err(error) => Err(error.into_anyhow()),
        }
    }

    pub(crate) async fn accept_community_node_consents_with_retry(
        &self,
        base_url: &str,
        token: &mut StoredCommunityNodeToken,
        policy_slugs: &[String],
        policy_snapshot_revision: Option<&str>,
    ) -> Result<CommunityNodeConsentStatus> {
        match self
            .request_accept_community_node_consents(
                base_url,
                token.access_token.as_str(),
                policy_slugs,
                policy_snapshot_revision,
            )
            .await
        {
            Ok(status) => Ok(status),
            Err(CommunityNodeRequestError::AuthRequired) => {
                *token = self.reauthenticate_community_node(base_url).await?;
                self.request_accept_community_node_consents(
                    base_url,
                    token.access_token.as_str(),
                    policy_slugs,
                    policy_snapshot_revision,
                )
                .await
                .map_err(CommunityNodeRequestError::into_anyhow)
            }
            Err(error) => Err(error.into_anyhow()),
        }
    }

    async fn refresh_community_node_registration_with_token_if_due_once(
        &self,
        base_url: &str,
        access_token: &str,
        force_heartbeat: bool,
    ) -> std::result::Result<(), CommunityNodeRequestError> {
        let base_url = normalize_http_url(base_url).map_err(CommunityNodeRequestError::Other)?;
        let now = Utc::now().timestamp();
        let (next_due_at, ready_refresh_pending, rendezvous_due_at) = {
            let mut sessions = self.community_node_sessions.lock().await;
            let entry = sessions
                .entry(base_url.to_string())
                .or_insert_with(CommunityNodeSessionState::default);
            let due = entry.heartbeat_deadline;
            let pending = std::mem::replace(&mut entry.ready_refresh_pending, false);
            (due, pending, entry.rendezvous_refresh_deadline)
        };
        if !force_heartbeat && next_due_at > now {
            if !self
                .community_node_bootstrap_metadata_retry_due(base_url.as_str(), now)
                .await
                && !ready_refresh_pending
            {
                // rendezvous presence(サーバ TTL 45 秒)は heartbeat(実効約 60 秒毎)より
                // 短命のため、heartbeat が not-due でも独立に refresh する(#572)。
                if rendezvous_due_at <= now {
                    self.refresh_rendezvous_with_token(base_url.as_str(), access_token)
                        .await?;
                    return Ok(());
                }
                debug!(
                    %base_url,
                    next_due_at,
                    now,
                    "skipping community-node heartbeat because the next refresh is not due"
                );
                return Ok(());
            }
            info!(
                %base_url,
                next_due_at,
                now,
                ready_refresh_pending,
                "running community-node metadata refresh without waiting for the next heartbeat"
            );
            return match self
                .sync_community_node_bootstrap_metadata(base_url.as_str(), access_token)
                .await
            {
                Ok(node) => {
                    self.refresh_rendezvous_with_token(base_url.as_str(), access_token)
                        .await?;
                    self.record_community_node_bootstrap_metadata_refresh(
                        base_url.as_str(),
                        node.resolved_urls
                            .as_ref()
                            .is_none_or(|resolved_urls| resolved_urls.seed_peers.is_empty()),
                        now,
                    )
                    .await;
                    Ok(())
                }
                Err(error) => {
                    self.record_community_node_bootstrap_metadata_refresh(
                        base_url.as_str(),
                        true,
                        now,
                    )
                    .await;
                    Err(error)
                }
            };
        }
        if force_heartbeat && next_due_at > now {
            info!(
                %base_url,
                next_due_at,
                now,
                "forcing community-node heartbeat before bootstrap metadata refresh"
            );
        }
        let seed_peer = self
            .local_community_node_seed_peer("heartbeat")
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        info!(
            %base_url,
            next_due_at,
            now,
            "refreshing community-node bootstrap heartbeat"
        );
        let client = community_node_http_client().map_err(CommunityNodeRequestError::Other)?;
        let response = client
            .post(format!("{base_url}{BOOTSTRAP_HEARTBEAT_PATH}"))
            .bearer_auth(access_token)
            .json(&BootstrapHeartbeatRequest {
                endpoint_id: seed_peer.endpoint_id,
                addr_hint: seed_peer.addr_hint,
            })
            .send()
            .await;
        match response {
            Ok(response) => {
                let heartbeat = response
                    .error_for_status()
                    .map_err(|error| {
                        Self::map_community_node_status_error(
                            "community node bootstrap heartbeat request failed",
                            error,
                        )
                    })?
                    .json::<BootstrapHeartbeatResponse>()
                    .await
                    .map_err(|error| {
                        Self::map_community_node_send_error(
                            "failed to decode community node bootstrap heartbeat response",
                            error,
                        )
                    })?;
                self.community_node_sessions
                    .lock()
                    .await
                    .entry(base_url.clone())
                    .or_insert_with(CommunityNodeSessionState::default)
                    .heartbeat_deadline = heartbeat
                    .expires_at
                    .saturating_sub(COMMUNITY_NODE_BOOTSTRAP_HEARTBEAT_INTERVAL_SECONDS);
                debug!(
                    %base_url,
                    expires_at = heartbeat.expires_at,
                    "community-node bootstrap heartbeat refreshed"
                );
                match self
                    .sync_community_node_bootstrap_metadata(base_url.as_str(), access_token)
                    .await
                {
                    Ok(node) => {
                        self.refresh_rendezvous_with_token(base_url.as_str(), access_token)
                            .await?;
                        self.record_community_node_bootstrap_metadata_refresh(
                            base_url.as_str(),
                            node.resolved_urls
                                .as_ref()
                                .is_none_or(|resolved_urls| resolved_urls.seed_peers.is_empty()),
                            now,
                        )
                        .await;
                        Ok(())
                    }
                    Err(error) => {
                        self.record_community_node_bootstrap_metadata_refresh(
                            base_url.as_str(),
                            true,
                            now,
                        )
                        .await;
                        Err(error)
                    }
                }
            }
            Err(error) => {
                self.community_node_sessions
                    .lock()
                    .await
                    .entry(base_url)
                    .or_insert_with(CommunityNodeSessionState::default)
                    .heartbeat_deadline =
                    now.saturating_add(COMMUNITY_NODE_BOOTSTRAP_HEARTBEAT_RETRY_SECONDS);
                Err(Self::map_community_node_send_error(
                    "failed to refresh community node bootstrap registration",
                    error,
                ))
            }
        }
    }

    /// サーバ側の未承認同意をローカル同意記録と照合し、カバー済みなら POST /v1/consents で
    /// 同期する(#857)。カバーしない(= 版が上がった再同意待ち)なら黙って再受諾せず
    /// Idle に落として偽を返す。
    async fn sync_covered_community_node_consents(
        &self,
        base_url: &str,
        token: &mut StoredCommunityNodeToken,
        consent_status: CommunityNodeConsentStatus,
    ) -> Result<bool> {
        self.set_community_node_cached_consent(base_url, Some(consent_status.clone()))
            .await;
        if consent_status.all_required_accepted {
            return Ok(true);
        }
        let local_consent =
            load_community_node_local_consents(&self.db_path, self.identity_mode, base_url)?;
        if !community_node_local_consent_covers_status(&local_consent, &consent_status) {
            self.set_community_node_local_consent_update_pending(base_url, true)
                .await;
            self.set_community_node_session_phase(base_url, CommunityNodeSessionPhase::Idle)
                .await;
            return Ok(false);
        }
        self.set_community_node_session_phase(base_url, CommunityNodeSessionPhase::Accepting)
            .await;
        let accepted = self
            .accept_community_node_consents_with_retry(
                base_url,
                token,
                &[],
                consent_status.policy_snapshot_revision.as_deref(),
            )
            .await?;
        self.set_community_node_cached_consent(base_url, Some(accepted))
            .await;
        Ok(true)
    }

    /// 期限の来た登録を送る。偽は、サーバの同意が手元の同意で覆えず、再同意が要ること。
    pub(crate) async fn refresh_community_node_registration_with_token_if_due(
        &self,
        base_url: &str,
        token: &mut StoredCommunityNodeToken,
        force_heartbeat: bool,
    ) -> Result<bool> {
        match self
            .refresh_community_node_registration_with_token_if_due_once(
                base_url,
                token.access_token.as_str(),
                force_heartbeat,
            )
            .await
        {
            Ok(()) => Ok(true),
            Err(CommunityNodeRequestError::AuthRequired) => {
                *token = self.reauthenticate_community_node(base_url).await?;
                let consent_status = self
                    .fetch_community_node_consent_status_with_retry(base_url, token, false)
                    .await?;
                if !self
                    .sync_covered_community_node_consents(base_url, token, consent_status)
                    .await?
                {
                    return Ok(false);
                }
                self.refresh_community_node_registration_with_token_if_due_once(
                    base_url,
                    token.access_token.as_str(),
                    force_heartbeat,
                )
                .await
                .map_err(CommunityNodeRequestError::into_anyhow)?;
                Ok(true)
            }
            Err(CommunityNodeRequestError::ConsentRequired) => {
                // 版が上がっての再同意（更新）かどうかを判定するため、現在の consent 状態を取得する。
                // ローカル同意がカバーする場合だけ同期し、更新はユーザーへ本文を再提示する。
                let consent_status = self
                    .fetch_community_node_consent_status_with_retry(base_url, token, false)
                    .await?;
                if !self
                    .sync_covered_community_node_consents(base_url, token, consent_status)
                    .await?
                {
                    return Ok(false);
                }
                self.refresh_community_node_registration_with_token_if_due_once(
                    base_url,
                    token.access_token.as_str(),
                    force_heartbeat,
                )
                .await
                .map_err(CommunityNodeRequestError::into_anyhow)?;
                Ok(true)
            }
            Err(error) => Err(error.into_anyhow()),
        }
    }
}
