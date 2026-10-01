use super::*;

#[derive(Clone, Debug)]
pub(crate) enum CommunityNodeConsentPreflight {
    Current {
        base_url: String,
        local_consent: CommunityNodeLocalConsentState,
    },
    Required {
        base_url: String,
        local_consent: CommunityNodeLocalConsentState,
        policy_update: bool,
    },
}

impl CommunityNodeConsentPreflight {
    pub(crate) fn base_url(&self) -> &str {
        match self {
            Self::Current { base_url, .. } | Self::Required { base_url, .. } => base_url,
        }
    }

    pub(crate) fn local_consent(&self) -> &CommunityNodeLocalConsentState {
        match self {
            Self::Current { local_consent, .. } | Self::Required { local_consent, .. } => {
                local_consent
            }
        }
    }
}

impl DesktopRuntime {
    /// Community Node の token・認証・保護 API より前に必ず通す同意境界。
    ///
    /// 設定 membership と active local consent を先に確認し、active な場合だけ公開
    /// policy catalog を取得して、required 文書の版と snapshot revision を照合する。
    /// session や connectivity は変更せず、呼び出し元が結果に応じた状態遷移を行う。
    pub(crate) async fn preflight_community_node_consent(
        &self,
        raw_base_url: &str,
    ) -> Result<CommunityNodeConsentPreflight> {
        let base_url = normalize_http_url(raw_base_url)?;
        self.require_community_node(base_url.as_str()).await?;
        let local_consent = load_community_node_local_consents(
            &self.db_path,
            self.identity_mode,
            base_url.as_str(),
        )
        .await?;
        if !local_consent.has_active_consent() {
            return Ok(CommunityNodeConsentPreflight::Required {
                base_url,
                local_consent,
                policy_update: false,
            });
        }

        let catalog = self
            .request_community_node_policies(base_url.as_str(), None)
            .await?;
        if !community_node_local_consent_satisfies_policies(&local_consent, &catalog.policies) {
            return Ok(CommunityNodeConsentPreflight::Required {
                base_url,
                local_consent,
                policy_update: true,
            });
        }

        Ok(CommunityNodeConsentPreflight::Current {
            base_url,
            local_consent,
        })
    }
}
