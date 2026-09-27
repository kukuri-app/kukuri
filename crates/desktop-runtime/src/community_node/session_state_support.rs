use super::*;

impl DesktopRuntime {
    pub(crate) async fn set_community_node_session_phase(
        &self,
        base_url: &str,
        phase: CommunityNodeSessionPhase,
    ) {
        let mut sessions = self.community_node_sessions.lock().await;
        let entry = sessions
            .entry(base_url.to_string())
            .or_insert_with(CommunityNodeSessionState::default);
        entry.session_phase = phase;
        // 一時的な失敗(Retrying)では認証・同意の確認を捨てない。relay と seed を使い続ける(#1221 R2-B)。
        if matches!(
            phase,
            CommunityNodeSessionPhase::Idle | CommunityNodeSessionPhase::AwaitingAdmission
        ) {
            entry.current_policy_verified_for = None;
        }
        if phase != CommunityNodeSessionPhase::Ready
            && matches!(
                phase,
                CommunityNodeSessionPhase::Idle
                    | CommunityNodeSessionPhase::Retrying
                    | CommunityNodeSessionPhase::AwaitingAdmission
            )
        {
            entry.ready_refresh_pending = false;
        }
    }

    pub(crate) async fn set_community_node_session_ready(
        &self,
        base_url: &str,
        schedule_immediate_refresh: bool,
        verified_local_consent: CommunityNodeLocalConsentState,
    ) {
        let mut sessions = self.community_node_sessions.lock().await;
        let entry = sessions
            .entry(base_url.to_string())
            .or_insert_with(CommunityNodeSessionState::default);
        let previous = Some(entry.session_phase);
        entry.session_phase = CommunityNodeSessionPhase::Ready;
        entry.current_policy_verified_for = Some(verified_local_consent);
        if schedule_immediate_refresh {
            entry.ready_refresh_pending = true;
            info!(target: "kukuri_connectivity", base_url, "community-node session ready");
            debug!(
                %base_url,
                previous_phase = ?previous,
                "scheduled immediate community-node metadata refresh after ready transition"
            );
        } else {
            entry.ready_refresh_pending = false;
            debug!(
                %base_url,
                previous_phase = ?previous,
                "keeping community-node metadata refresh pending state cleared for an already-ready session"
            );
        }
    }

    pub(crate) async fn community_node_session_was_ready(&self, base_url: &str) -> bool {
        self.community_node_sessions
            .lock()
            .await
            .get(base_url)
            .map(|s| s.session_phase)
            == Some(CommunityNodeSessionPhase::Ready)
    }

    pub(crate) async fn set_community_node_cached_consent(
        &self,
        base_url: &str,
        consent_state: Option<CommunityNodeConsentStatus>,
    ) {
        let mut sessions = self.community_node_sessions.lock().await;
        let entry = sessions
            .entry(base_url.to_string())
            .or_insert_with(CommunityNodeSessionState::default);
        entry.cached_consent = consent_state;
    }

    /// `ensure_community_node_session` 直後の同意確認(#698 / #705 / #857)。
    ///
    /// 次のいずれかなら真を返す。索引参照・索引申請・信頼関係・テスターフィードバックの
    /// クライアントは、真なら HTTP を送らず `CONSENT_REQUIRED` を返す(サーバの 403 に頼らない)。
    /// - ローカル同意記録が無い/撤回済み(#857: 未同意 node へは通信しない)
    /// - 公開カタログ照合で再同意が必要と判明している(#857)
    /// - キャッシュ済みのサーバ同意が「必須同意未承認」
    pub(crate) async fn community_node_required_consent_is_pending(&self, base_url: &str) -> bool {
        let local_consent_active =
            load_community_node_local_consents(&self.db_path, self.identity_mode, base_url)
                .map(|state| state.has_active_consent())
                .unwrap_or(false);
        if !local_consent_active {
            return true;
        }
        let sessions = self.community_node_sessions.lock().await;
        let Some(session) = sessions.get(base_url) else {
            return false;
        };
        if session.local_consent_update_pending {
            return true;
        }
        session
            .cached_consent
            .as_ref()
            .is_some_and(|consent| !consent.all_required_accepted)
    }

    /// #857: 公開カタログ照合の結果(再同意が必要か)をセッション状態へ記録する。
    pub(crate) async fn set_community_node_local_consent_update_pending(
        &self,
        base_url: &str,
        pending: bool,
    ) {
        let mut sessions = self.community_node_sessions.lock().await;
        let entry = sessions
            .entry(base_url.to_string())
            .or_insert_with(CommunityNodeSessionState::default);
        entry.local_consent_update_pending = pending;
    }

    pub(crate) async fn clear_community_node_retry_state(&self, base_url: &str) {
        let mut sessions = self.community_node_sessions.lock().await;
        if let Some(entry) = sessions.get_mut(base_url) {
            entry.session_retry_deadline = 0;
            entry.last_error = None;
            entry.admission_rejection = None;
        }
    }

    pub(crate) async fn set_community_node_admission_rejection(
        &self,
        base_url: &str,
        rejection: CommunityNodeAdmissionRejection,
    ) {
        // 参加拒否後の既存トークンはサーバ側で 401 になる。端末側に残すと「認証済み」判定が
        // 続き、自己修復経路が再認証を繰り返すため、拒否と同時に破棄する(#708)。
        if let Err(error) = crate::identity::delete_optional_secret(
            &self.db_path,
            self.identity_mode,
            COMMUNITY_NODE_TOKEN_PURPOSE,
            base_url,
        ) {
            warn!(
                error = %error,
                base_url,
                "failed to discard community-node token after admission rejection"
            );
        }
        {
            let mut sessions = self.community_node_sessions.lock().await;
            let entry = sessions
                .entry(base_url.to_string())
                .or_insert_with(CommunityNodeSessionState::default);
            entry.session_retry_deadline = 0;
            entry.last_error = None;
            entry.admission_rejection = Some(rejection);
        }
        self.set_community_node_session_phase(
            base_url,
            CommunityNodeSessionPhase::AwaitingAdmission,
        )
        .await;
    }

    pub(crate) fn community_node_admission_rejection(
        error: &anyhow::Error,
    ) -> Option<&CommunityNodeAdmissionRejection> {
        error.downcast_ref::<CommunityNodeAdmissionRejection>()
    }

    pub(crate) async fn set_community_node_retry_state(
        &self,
        base_url: &str,
        error: anyhow::Error,
    ) {
        let now = Utc::now().timestamp();
        {
            let mut sessions = self.community_node_sessions.lock().await;
            let entry = sessions
                .entry(base_url.to_string())
                .or_insert_with(CommunityNodeSessionState::default);
            if entry.last_error.as_deref() != Some(error.to_string().as_str()) {
                warn!(target: "kukuri_connectivity", base_url,
                    phase = ?entry.session_phase, %error,
                    "community-node session failed; retry scheduled");
            }
            entry.last_error = Some(error.to_string());
            entry.admission_rejection = None;
            entry.session_retry_deadline = now.saturating_add(COMMUNITY_NODE_SESSION_RETRY_SECONDS);
        }
        self.set_community_node_session_phase(base_url, CommunityNodeSessionPhase::Retrying)
            .await;
    }

    pub(crate) fn community_node_token_requires_refresh(
        token: &StoredCommunityNodeToken,
        now: i64,
    ) -> bool {
        token.expires_at <= now.saturating_add(COMMUNITY_NODE_AUTH_REFRESH_SKEW_SECONDS)
    }

    pub(crate) fn map_community_node_send_error(
        action: &str,
        error: reqwest::Error,
    ) -> CommunityNodeRequestError {
        CommunityNodeRequestError::Other(anyhow!(error).context(action.to_string()))
    }

    pub(crate) fn map_community_node_status_error(
        action: &str,
        error: reqwest::Error,
    ) -> CommunityNodeRequestError {
        match error.status() {
            Some(StatusCode::UNAUTHORIZED) => CommunityNodeRequestError::AuthRequired,
            Some(StatusCode::FORBIDDEN) => CommunityNodeRequestError::ConsentRequired,
            _ => CommunityNodeRequestError::Other(anyhow!(error).context(action.to_string())),
        }
    }
}
