use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::normalize::{normalize_http_url, normalize_http_url_list};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeResolvedUrls {
    pub public_base_url: String,
    pub connectivity_urls: Vec<String>,
    // 現行 types.ts と同じく任意(#[serde(default)])。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<CommunityNodeSeedPeer>>"))]
    pub seed_peers: Vec<CommunityNodeSeedPeer>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeSeedPeer {
    pub endpoint_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr_hint: Option<String>,
}

impl CommunityNodeSeedPeer {
    pub fn new(endpoint_id: impl Into<String>, addr_hint: Option<String>) -> Result<Self> {
        let endpoint_id = endpoint_id.into().trim().to_string();
        if endpoint_id.is_empty() {
            bail!("community-node seed peer endpoint id must not be empty");
        }
        let addr_hint = addr_hint
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Ok(Self {
            endpoint_id,
            addr_hint,
        })
    }

    pub fn display(&self) -> String {
        match self.addr_hint.as_deref() {
            Some(addr_hint) => format!("{}@{}", self.endpoint_id, addr_hint),
            None => self.endpoint_id.clone(),
        }
    }
}

pub fn normalize_seed_peers(
    values: Vec<CommunityNodeSeedPeer>,
) -> Result<Vec<CommunityNodeSeedPeer>> {
    let mut deduped = BTreeMap::new();
    for value in values {
        let normalized = CommunityNodeSeedPeer::new(value.endpoint_id, value.addr_hint)?;
        deduped.insert(normalized.display(), normalized);
    }
    Ok(deduped.into_values().collect())
}

impl CommunityNodeResolvedUrls {
    pub fn new(
        public_base_url: impl Into<String>,
        connectivity_urls: Vec<String>,
        seed_peers: Vec<CommunityNodeSeedPeer>,
    ) -> Result<Self> {
        let public_base_url = normalize_http_url(public_base_url.into().as_str())?;
        let connectivity_urls = normalize_http_url_list(connectivity_urls)?;
        let seed_peers = normalize_seed_peers(seed_peers)?;
        Ok(Self {
            public_base_url,
            connectivity_urls,
            seed_peers,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityNodeBootstrapNode {
    pub base_url: String,
    pub resolved_urls: CommunityNodeResolvedUrls,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallengeResponse {
    pub challenge: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthVerifyResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_at: i64,
    pub pubkey: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapHeartbeatResponse {
    pub expires_at: i64,
}

/// `GET /v1/bootstrap/nodes` の response(WP-B17 で server / client の二重定義を共有化)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapNodesResponse {
    pub nodes: Vec<CommunityNodeBootstrapNode>,
}

/// `GET /v1/policies` の公開 policy カタログ 1 件(#857)。同意判断の提示に必要な
/// 文書メタデータと本文のみで、ユーザー固有情報を含まない(認証不要)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityNodePolicyDocument {
    pub policy_slug: String,
    pub policy_version: i32,
    pub title: String,
    pub body_markdown: String,
    pub required: bool,
    /// operator config の `legal.documents[].kind`(#1192)。slug は operator が自由に決める
    /// ため、client が文書の役割(利用規約 / 権利侵害申出ポリシー等)を判別するのに使う。
    /// node が現在公開していない slug(退役 revision 等)では欠落する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_snapshot_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authoritative_language: Option<String>,
    #[serde(default)]
    pub reference_translation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_revision: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_of_version: Option<i32>,
    #[serde(default)]
    pub fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_language: Option<String>,
    #[serde(default)]
    pub material_change: bool,
    #[serde(default)]
    pub requires_reconsent: bool,
    // `true` はこの slug の現在正文。過去 revision は `false` のまま保持される。
    #[serde(default)]
    pub is_current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_policy_version: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_policy_snapshot_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_policy_version: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_policy_snapshot_revision: Option<String>,
}

/// `GET /v1/policies` の response(#857)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityNodePoliciesResponse {
    pub policies: Vec<CommunityNodePolicyDocument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_snapshot_revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeConsentItem {
    pub policy_slug: String,
    pub policy_version: i32,
    pub title: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>"))]
    pub body: String,
    pub required: bool,
    pub accepted_at: Option<i64>,
    /// その policy_slug で過去に同意した最大 version（version 不問）。
    /// `accepted_at` が None でこれが Some なら、版が上がって再同意が必要な「更新」を意味する。
    #[serde(default)]
    pub previously_accepted_version: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_snapshot_revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeConsentStatus {
    pub all_required_accepted: bool,
    pub items: Vec<CommunityNodeConsentItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_snapshot_revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BearerIdentity {
    pub pubkey: String,
    pub endpoint_id: Option<String>,
}
