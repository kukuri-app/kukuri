//! Separate bounded account-route rendezvous. The legacy topic refresh still
//! has a whole-subscription snapshot; this request does not add to that scan.

use super::*;
use std::collections::BTreeMap;

const MAX_ACCOUNT_QUERY_RECIPIENTS: usize = 4;
const MAX_ACCOUNT_RENDEZVOUS_RESPONSE_BYTES: usize = 65_536;
const MAX_ACCOUNT_RENDEZVOUS_CANDIDATES: usize = 8;
const MAX_CANDIDATE_RELAY_URLS: usize = 4;

impl DesktopRuntime {
    pub(super) async fn refresh_account_receive_rendezvous_with_token(
        &self,
        base_url: &str,
        access_token: &str,
    ) -> std::result::Result<(), CommunityNodeRequestError> {
        let consent_at_request =
            load_community_node_local_consents(&self.db_path, self.identity_mode, base_url)
                .await
                .map_err(CommunityNodeRequestError::Other)?;
        if !consent_at_request.has_active_consent() {
            return Err(CommunityNodeRequestError::Other(anyhow!(
                "account rendezvous requires active local consent"
            )));
        }
        let candidate_fence = self
            .iroh_stack
            .transport
            .receive_candidate_fence()
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        let own_route = receive_route_for_account(&self.author_keys.public_key())
            .map_err(CommunityNodeRequestError::Other)?;
        let own_key = public_topic_rendezvous_key(&own_route);
        let (after, cycle_end) = self
            .community_node_sessions
            .lock()
            .await
            .get(base_url)
            .map(|session| {
                (
                    session.account_candidate_after.clone(),
                    session.account_candidate_cycle_end.clone(),
                )
            })
            .unwrap_or_default();
        let page = self
            .app_service
            .pending_receive_destination_recipients(after.as_ref(), cycle_end.as_ref())
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        if page.recipients.len() > MAX_ACCOUNT_QUERY_RECIPIENTS {
            return Err(CommunityNodeRequestError::Other(anyhow!(
                "account receive demand exceeds bounded rendezvous window"
            )));
        }
        let mut by_key = BTreeMap::<String, Pubkey>::new();
        for recipient in &page.recipients {
            if *recipient == self.author_keys.public_key() {
                continue;
            }
            let route =
                receive_route_for_account(recipient).map_err(CommunityNodeRequestError::Other)?;
            by_key.insert(public_topic_rendezvous_key(&route), recipient.clone());
        }
        let mut refreshes = Vec::with_capacity(by_key.len() + 1);
        refreshes.push(own_key);
        refreshes.extend(by_key.keys().cloned());
        let endpoint = self
            .local_community_node_seed_peer("account-rendezvous")
            .await
            .map_err(CommunityNodeRequestError::Other)?;
        let client = community_node_http_client().map_err(CommunityNodeRequestError::Other)?;
        let mut response = client
            .post(format!("{base_url}{TOPIC_RENDEZVOUS_HEARTBEAT_PATH}"))
            .bearer_auth(access_token)
            .json(&TopicRendezvousHeartbeat {
                endpoint_id: endpoint.endpoint_id.clone(),
                addr_hint: endpoint.addr_hint,
                joins: Vec::new(),
                refreshes,
                leaves: Vec::new(),
            })
            .send()
            .await
            .map_err(|error| {
                Self::map_community_node_send_error(
                    "failed to refresh account receive rendezvous",
                    error,
                )
            })?
            .error_for_status()
            .map_err(|error| {
                Self::map_community_node_status_error(
                    "account receive rendezvous request failed",
                    error,
                )
            })?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            Self::map_community_node_send_error(
                "failed to read account receive rendezvous response",
                error,
            )
        })? {
            if bytes.len().saturating_add(chunk.len()) > MAX_ACCOUNT_RENDEZVOUS_RESPONSE_BYTES {
                return Err(CommunityNodeRequestError::Other(anyhow!(
                    "account receive rendezvous response is too large"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: TopicRendezvousHeartbeatResponse = serde_json::from_slice(&bytes)
            .map_err(|error| CommunityNodeRequestError::Other(error.into()))?;
        // Initial registration runs while the session is Refreshing, before
        // Ready records current_policy_verified_for. Require its verified
        // consent status and compare the local consent across the HTTP await.
        let consent_now =
            load_community_node_local_consents(&self.db_path, self.identity_mode, base_url)
                .await
                .map_err(CommunityNodeRequestError::Other)?;
        let configured = self
            .community_node_config
            .lock()
            .await
            .nodes
            .iter()
            .any(|node| node.base_url == base_url);
        let session_valid = self
            .community_node_sessions
            .lock()
            .await
            .get(base_url)
            .is_some_and(|session| {
                !session.local_consent_update_pending
                    && session
                        .cached_consent
                        .as_ref()
                        .is_some_and(|consent| consent.all_required_accepted)
                    && matches!(
                        session.session_phase,
                        CommunityNodeSessionPhase::Refreshing | CommunityNodeSessionPhase::Ready
                    )
            });
        if !configured || consent_now != consent_at_request || !session_valid {
            return Err(CommunityNodeRequestError::Other(anyhow!(
                "account rendezvous session is no longer active"
            )));
        }
        if response.topics.len() > by_key.len() + 1 {
            return Err(CommunityNodeRequestError::Other(anyhow!(
                "account receive rendezvous returned too many topics"
            )));
        }
        for topic in response.topics {
            let Some(recipient) = by_key.get(&topic.topic_key) else {
                continue;
            };
            if topic.peers.len() > MAX_ACCOUNT_RENDEZVOUS_CANDIDATES {
                return Err(CommunityNodeRequestError::Other(anyhow!(
                    "account receive rendezvous returned too many peers"
                )));
            }
            let mut candidates = Vec::with_capacity(topic.peers.len());
            for peer in topic.peers {
                if peer.endpoint_id == endpoint.endpoint_id
                    || peer.relay_urls.len() > MAX_CANDIDATE_RELAY_URLS
                {
                    continue;
                }
                if let Ok(address) = (SeedPeer {
                    endpoint_id: peer.endpoint_id,
                    addr_hint: peer.addr_hint,
                })
                .to_endpoint_addr_with_relay_url_strings(&peer.relay_urls)
                {
                    candidates.push(address);
                }
            }
            self.iroh_stack
                .transport
                .offer_receive_candidates(base_url, recipient, candidates, candidate_fence)
                .await
                .map_err(CommunityNodeRequestError::Other)?;
        }
        if self
            .iroh_stack
            .transport
            .receive_candidate_fence()
            .await
            .map_err(CommunityNodeRequestError::Other)?
            != candidate_fence
        {
            return Err(CommunityNodeRequestError::Other(anyhow!(
                "account rendezvous candidate owner changed"
            )));
        }
        if let Some(session) = self.community_node_sessions.lock().await.get_mut(base_url) {
            session.account_candidate_after = page.next_cursor;
            session.account_candidate_cycle_end = page.cycle_end;
            session.rendezvous_refresh_deadline = Utc::now().timestamp().saturating_add(
                (response.expires_in_seconds.min(i64::MAX as u64) as i64)
                    .saturating_sub(COMMUNITY_NODE_TOPIC_RENDEZVOUS_REFRESH_MARGIN_SECONDS),
            );
        }
        Ok(())
    }
}
