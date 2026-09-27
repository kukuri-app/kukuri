//! ブロック / ミュート観測の CN 提供（ADR 0026 §8.3 / §8.5、ADR 0022 追補、#1061）。
//!
//! - 提供は CN ごとの選択で、既定は無効。CN の同意カタログにある任意文書
//!   `trust_observation_sharing` への同意で有効になる。文書を公開していない CN には送らない。
//! - mute / block の canonical は変えない。この端末で行った操作を、提供中の CN ごとの送信待ちへ
//!   （CN × 対象 × 種別で最新 1 件に集約して）積み、session が Ready のときだけ送る。
//! - 無効化・同意取消・CN の削除では送信待ちを破棄し、CN に保存された自分の観測の削除を要求する。
//!   削除要求が完了するまで、その CN には新しい観測を送らない。
//!
//! 状態は account DB の隣のファイル（`<db>.trust-observations.json`）に保存する。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use kukuri_app_api::SocialConnectionKind;
use kukuri_cn_protocol::{
    CommunityNodePolicyDocument, INVALID_TRUST_OBSERVATION_CODE,
    TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED_CODE, TRUST_OBSERVATION_SHARING_NOT_OFFERED_CODE,
    TRUST_OBSERVATION_SHARING_POLICY_SLUG, TRUST_OBSERVATIONS_MAX_ENVELOPES,
    TRUST_OBSERVATIONS_PATH, TRUST_READ_NOT_CONFIGURED_CODE, TrustObservationsRevokeResponse,
    TrustObservationsSubmitResponse, normalize_http_url, normalize_pubkey,
};
use kukuri_core::{
    BlockEdgeStatus, KukuriEnvelope, MuteObservationStatus, Pubkey, TrustObservationKind,
    build_block_edge_envelope, build_mute_observation_envelope, parse_trust_observation,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{
    CommunityNodeConsentDocumentRef, CommunityNodeSessionOutcome, CommunityNodeTargetRequest,
    community_node_http_client, load_community_node_local_consents, load_community_node_token,
    persist_community_node_local_consents, record_community_node_local_consents,
};
use crate::runtime::DesktopRuntime;

pub(crate) const TRUST_OBSERVATION_STATE_FILE_EXTENSION: &str = "trust-observations.json";

/// 観測提供の状態（CN ごと）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeObservationSharingStatus {
    pub base_url: String,
    /// CN が任意文書を公開しているか。取得できなかった場合は false。
    pub offered: bool,
    /// 公開中の任意文書（有効化ダイアログで提示する）。
    pub policy: Option<CommunityNodePolicyDocument>,
    /// 提供中か。
    pub enabled: bool,
    /// CN の文書が更新され、再同意まで提供を止めているか。
    pub needs_reconsent: bool,
    /// 保存済み観測の削除要求が未完了か（完了まで提供を再開しない）。
    pub revocation_pending: bool,
    /// 送信待ちの件数。
    pub pending_count: u32,
}

/// 観測提供の有効化。`policy_*` は利用者に提示した文書の版。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct EnableCommunityNodeObservationSharingRequest {
    pub base_url: String,
    pub policy_version: i32,
    #[serde(default)]
    pub policy_snapshot_revision: Option<String>,
    #[serde(default)]
    pub language: String,
    /// 既存のブロック / ミュートも送るか（既定は送らない）。
    #[serde(default)]
    pub include_existing: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct NodeObservationState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    needs_reconsent: bool,
    #[serde(default)]
    revocation_pending: bool,
    /// key = `<target>|<kind>`。最新の envelope だけを保持する。
    #[serde(default)]
    pending: BTreeMap<String, KukuriEnvelope>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TrustObservationState {
    #[serde(default)]
    nodes: BTreeMap<String, NodeObservationState>,
    /// key = `<target>|<kind>` ごとに最後に署名した created_at（ミリ秒）。同じミリ秒の操作で
    /// 新旧が入れ替わらないよう、次の署名はこれより後にする。
    #[serde(default)]
    last_signed_at: BTreeMap<String, i64>,
}

fn state_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(TRUST_OBSERVATION_STATE_FILE_EXTENSION)
}

fn load_state(db_path: &Path) -> Result<TrustObservationState> {
    let path = state_path(db_path);
    if !path.exists() {
        return Ok(TrustObservationState::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read trust observation state `{}`",
            path.display()
        )
    })?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "failed to parse trust observation state `{}`",
            path.display()
        )
    })
}

fn save_state(db_path: &Path, state: &TrustObservationState) -> Result<()> {
    let path = state_path(db_path);
    let json =
        serde_json::to_vec_pretty(state).context("failed to encode trust observation state")?;
    let temporary = path.with_extension(format!("{TRUST_OBSERVATION_STATE_FILE_EXTENSION}.tmp"));
    fs::write(&temporary, json).with_context(|| {
        format!(
            "failed to write trust observation state `{}`",
            temporary.display()
        )
    })?;
    fs::rename(&temporary, &path).with_context(|| {
        format!(
            "failed to replace trust observation state `{}`",
            path.display()
        )
    })
}

fn pending_key(target: &str, kind: TrustObservationKind) -> String {
    format!("{target}|{}", kind.as_str())
}

fn parse_pending_key(key: &str) -> Result<(Pubkey, TrustObservationKind)> {
    let (target, kind) = key
        .split_once('|')
        .ok_or_else(|| anyhow!("malformed trust observation key `{key}`"))?;
    let kind = match kind {
        "block" => TrustObservationKind::Block,
        "mute" => TrustObservationKind::Mute,
        other => bail!("unknown trust observation kind `{other}`"),
    };
    Ok((Pubkey::from(target.to_string()), kind))
}

/// 送信待ちの観測が、この端末の現在の mute / block と一致するか。
///
/// 一致しない送信待ち（提供が止まっている間に解除された等）は、送らずに捨てる。
fn pending_matches_current(
    key: &str,
    envelope: &KukuriEnvelope,
    current: &BTreeSet<String>,
) -> bool {
    match parse_trust_observation(envelope) {
        Ok(Some(observation)) => observation.active == current.contains(key),
        // 読めない envelope は送らない。
        _ => false,
    }
}

fn sharing_policy(policies: &[CommunityNodePolicyDocument]) -> Option<CommunityNodePolicyDocument> {
    policies
        .iter()
        .find(|policy| {
            policy.policy_slug == TRUST_OBSERVATION_SHARING_POLICY_SLUG && !policy.required
        })
        .cloned()
}

/// 送信結果の分類。
enum SubmitOutcome {
    Sent(Vec<(String, String)>),
    ConsentRequired,
    NotOffered,
    Rejected(Vec<(String, String)>),
    Retry,
}

impl DesktopRuntime {
    fn sign_observation(
        &self,
        state: &mut TrustObservationState,
        target: &Pubkey,
        kind: TrustObservationKind,
        active: bool,
    ) -> Result<KukuriEnvelope> {
        let key = pending_key(target.as_str(), kind);
        let last = state.last_signed_at.get(&key).copied().unwrap_or(i64::MIN);
        for _ in 0..50 {
            let envelope = match kind {
                TrustObservationKind::Block => build_block_edge_envelope(
                    &self.author_keys,
                    target,
                    if active {
                        BlockEdgeStatus::Active
                    } else {
                        BlockEdgeStatus::Revoked
                    },
                )?,
                TrustObservationKind::Mute => build_mute_observation_envelope(
                    &self.author_keys,
                    target,
                    if active {
                        MuteObservationStatus::Active
                    } else {
                        MuteObservationStatus::Revoked
                    },
                )?,
            };
            if envelope.created_at > last {
                state.last_signed_at.insert(key, envelope.created_at);
                return Ok(envelope);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Err(anyhow!("failed to sign a newer trust observation"))
    }

    /// この端末で行った mute / block 操作を、提供中の CN の送信待ちへ積む（送信は scheduler）。
    ///
    /// ローカル操作の成否には影響させない（呼出側は失敗を警告として扱う）。
    pub(crate) async fn enqueue_trust_observation(
        &self,
        target_pubkey: &str,
        kind: TrustObservationKind,
        active: bool,
    ) -> Result<()> {
        let _guard = self.trust_observation_guard.lock().await;
        let mut state = load_state(&self.db_path)?;
        if !state
            .nodes
            .values()
            .any(|node| node.enabled && !node.revocation_pending)
        {
            return Ok(());
        }
        let target = Pubkey::from(normalize_pubkey(target_pubkey)?);
        let envelope = self.sign_observation(&mut state, &target, kind, active)?;
        let key = pending_key(target.as_str(), kind);
        for node in state.nodes.values_mut() {
            if node.enabled && !node.revocation_pending {
                node.pending.insert(key.clone(), envelope.clone());
            }
        }
        save_state(&self.db_path, &state)
    }

    /// 1 node の送信待ちの観測を送り、未完了の削除要求を再送する(scheduler の node ごとの lane。
    /// 止まった node が別の node の送信を塞がない。#1221 R2-B)。
    pub(crate) async fn flush_community_node_trust_observations_for(&self, base_url: &str) {
        let node = {
            let _guard = self.trust_observation_guard.lock().await;
            match load_state(&self.db_path) {
                Ok(mut state) => state.nodes.remove(base_url),
                Err(error) => {
                    warn!(error = %error, "failed to load trust observation state");
                    return;
                }
            }
        };
        let Some(node) = node else {
            return;
        };
        if node.revocation_pending {
            if let Err(error) = self.complete_trust_observation_revocation(base_url).await {
                warn!(base_url, error = %error, "trust observation revocation is still pending");
            }
            return;
        }
        if !node.enabled || node.pending.is_empty() {
            return;
        }
        let batch: Vec<(String, KukuriEnvelope)> = node
            .pending
            .into_iter()
            .take(TRUST_OBSERVATIONS_MAX_ENVELOPES)
            .collect();
        let outcome = self.submit_trust_observations(base_url, &batch).await;
        if let Err(error) = self.apply_submit_outcome(base_url, outcome).await {
            warn!(base_url, error = %error, "failed to record trust observation delivery");
        }
    }

    #[cfg(test)]
    pub(crate) async fn flush_community_node_trust_observations_once(&self) {
        let configured = self.community_node_config.lock().await.nodes.clone();
        for node in configured {
            self.flush_community_node_trust_observations_for(&node.base_url)
                .await;
        }
    }

    async fn submit_trust_observations(
        &self,
        base_url: &str,
        batch: &[(String, KukuriEnvelope)],
    ) -> SubmitOutcome {
        let sent: Vec<(String, String)> = batch
            .iter()
            .map(|(key, envelope)| (key.clone(), envelope.id.0.clone()))
            .collect();
        // session が Ready でない（必須同意の未成立・Deferred）場合は HTTP を送らない。
        match self.ensure_community_node_session(base_url).await {
            Ok(CommunityNodeSessionOutcome::Ready) => {}
            Ok(_) => return SubmitOutcome::Retry,
            Err(error) => {
                warn!(base_url = %base_url, error = %error, "community node session is not ready for trust observations");
                return SubmitOutcome::Retry;
            }
        }
        let envelopes: Vec<&KukuriEnvelope> = batch.iter().map(|(_, envelope)| envelope).collect();
        let body = serde_json::json!({ "envelopes": envelopes });
        match self
            .request_community_node_trust_relation::<TrustObservationsSubmitResponse>(
                base_url,
                Method::POST,
                TRUST_OBSERVATIONS_PATH,
                None,
                Some(&body),
            )
            .await
        {
            Ok(_) => SubmitOutcome::Sent(sent),
            Err(error) if error.code == TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED_CODE => {
                SubmitOutcome::ConsentRequired
            }
            Err(error)
                if error.code == TRUST_OBSERVATION_SHARING_NOT_OFFERED_CODE
                    || error.code == TRUST_READ_NOT_CONFIGURED_CODE =>
            {
                SubmitOutcome::NotOffered
            }
            Err(error) if error.code == INVALID_TRUST_OBSERVATION_CODE => {
                warn!(base_url = %base_url, error = %error, "community node rejected trust observations; dropping the batch");
                SubmitOutcome::Rejected(sent)
            }
            Err(error) => {
                warn!(base_url = %base_url, error = %error, "failed to submit trust observations; will retry");
                SubmitOutcome::Retry
            }
        }
    }

    async fn apply_submit_outcome(&self, base_url: &str, outcome: SubmitOutcome) -> Result<()> {
        let _guard = self.trust_observation_guard.lock().await;
        let mut state = load_state(&self.db_path)?;
        let Some(node) = state.nodes.get_mut(base_url) else {
            return Ok(());
        };
        match outcome {
            SubmitOutcome::Sent(sent) | SubmitOutcome::Rejected(sent) => {
                // 送信中に同じ key が新しい操作で置き換わっていたら、それは次回送る。
                for (key, envelope_id) in sent {
                    if node
                        .pending
                        .get(&key)
                        .is_some_and(|envelope| envelope.id.0 == envelope_id)
                    {
                        node.pending.remove(&key);
                    }
                }
            }
            // 提供の同意が失効した / 文書が無くなった CN には、送信済みの観測も残さない。
            // 送信待ちを破棄したうえで削除を要求し、再同意までは新しい観測を送らない。
            SubmitOutcome::ConsentRequired => {
                node.enabled = false;
                node.needs_reconsent = true;
                node.pending.clear();
                node.revocation_pending = true;
            }
            SubmitOutcome::NotOffered => {
                node.enabled = false;
                node.pending.clear();
                node.revocation_pending = true;
            }
            SubmitOutcome::Retry => return Ok(()),
        }
        save_state(&self.db_path, &state)
    }

    /// 保存済み観測の削除と提供同意の取消を CN に要求する。完了したら削除要求の印を外す。
    ///
    /// 取消は必須同意の状態によらず受け付けられる（本人認証だけ）ため、session の Ready は要求しない。
    async fn complete_trust_observation_revocation(&self, base_url: &str) -> Result<()> {
        let token = match load_community_node_token(&self.db_path, self.identity_mode, base_url)? {
            Some(token) => token,
            None => {
                self.request_community_node_authentication_token(base_url)
                    .await?
            }
        };
        let mut status = self
            .send_trust_observation_revocation(base_url, token.access_token.as_str())
            .await?;
        if status == StatusCode::UNAUTHORIZED {
            let refreshed = self
                .request_community_node_authentication_token(base_url)
                .await?;
            status = self
                .send_trust_observation_revocation(base_url, refreshed.access_token.as_str())
                .await?;
        }
        // 未構成の node には削除する観測が無い。
        if !(status.is_success() || status == StatusCode::NOT_FOUND) {
            return Err(anyhow!(
                "community node trust observation revocation failed with {status}"
            ));
        }
        let _guard = self.trust_observation_guard.lock().await;
        let mut state = load_state(&self.db_path)?;
        if let Some(node) = state.nodes.get_mut(base_url) {
            node.revocation_pending = false;
        }
        save_state(&self.db_path, &state)
    }

    async fn send_trust_observation_revocation(
        &self,
        base_url: &str,
        access_token: &str,
    ) -> Result<StatusCode> {
        let response = community_node_http_client()?
            .delete(format!("{base_url}{TRUST_OBSERVATIONS_PATH}"))
            .bearer_auth(access_token)
            .send()
            .await
            .context("failed to send trust observation revocation")?;
        let status = response.status();
        if status.is_success() {
            let _ = response.json::<TrustObservationsRevokeResponse>().await;
        }
        Ok(status)
    }

    /// 提供を止め、保存済み観測の削除を要求する（無効化・同意取消・CN の削除で使う）。
    /// 要求が失敗しても印を残し、scheduler が再送する。
    pub(crate) async fn revoke_community_node_trust_observations(
        &self,
        base_url: &str,
    ) -> Result<()> {
        {
            let _guard = self.trust_observation_guard.lock().await;
            let mut state = load_state(&self.db_path)?;
            let Some(node) = state.nodes.get_mut(base_url) else {
                return Ok(());
            };
            if !node.enabled && !node.revocation_pending && !node.needs_reconsent {
                return Ok(());
            }
            node.enabled = false;
            node.needs_reconsent = false;
            node.pending.clear();
            node.revocation_pending = true;
            save_state(&self.db_path, &state)?;
        }
        if let Err(error) = self.complete_trust_observation_revocation(base_url).await {
            warn!(base_url = %base_url, error = %error, "trust observation revocation will be retried");
        }
        Ok(())
    }

    /// CN の削除時に、削除要求を試みてから状態を破棄する。到達できなかった観測は CN の保持期間で消える。
    pub(crate) async fn forget_community_node_trust_observations(
        &self,
        base_url: &str,
    ) -> Result<()> {
        self.revoke_community_node_trust_observations(base_url)
            .await?;
        let _guard = self.trust_observation_guard.lock().await;
        let mut state = load_state(&self.db_path)?;
        if let Some(node) = state.nodes.remove(base_url)
            && node.revocation_pending
        {
            warn!(base_url = %base_url, "removed community node before trust observation revocation completed");
        }
        save_state(&self.db_path, &state)
    }

    pub async fn get_community_node_observation_sharing(
        &self,
        request: CommunityNodeTargetRequest,
    ) -> Result<CommunityNodeObservationSharingStatus> {
        let base_url = normalize_http_url(request.base_url.as_str())?;
        self.require_community_node(base_url.as_str()).await?;
        let policy = match self
            .request_community_node_policies(base_url.as_str(), None)
            .await
        {
            Ok(response) => sharing_policy(&response.policies),
            Err(error) => {
                warn!(base_url = %base_url, error = %error, "failed to load community node policies for observation sharing");
                None
            }
        };
        self.observation_sharing_status(base_url, policy).await
    }

    async fn observation_sharing_status(
        &self,
        base_url: String,
        policy: Option<CommunityNodePolicyDocument>,
    ) -> Result<CommunityNodeObservationSharingStatus> {
        let node = {
            let _guard = self.trust_observation_guard.lock().await;
            load_state(&self.db_path)?
                .nodes
                .get(&base_url)
                .cloned()
                .unwrap_or_default()
        };
        Ok(CommunityNodeObservationSharingStatus {
            base_url,
            offered: policy.is_some(),
            policy,
            enabled: node.enabled,
            needs_reconsent: node.needs_reconsent,
            revocation_pending: node.revocation_pending,
            pending_count: u32::try_from(node.pending.len()).unwrap_or(u32::MAX),
        })
    }

    /// 任意文書への同意で観測提供を有効にする。
    pub async fn enable_community_node_observation_sharing(
        &self,
        request: EnableCommunityNodeObservationSharingRequest,
        app_version: &str,
    ) -> Result<CommunityNodeObservationSharingStatus> {
        let base_url = normalize_http_url(request.base_url.as_str())?;
        self.require_community_node(base_url.as_str()).await?;
        // 以前の削除要求が終わるまで再開しない。
        let revocation_pending = {
            let _guard = self.trust_observation_guard.lock().await;
            load_state(&self.db_path)?
                .nodes
                .get(&base_url)
                .is_some_and(|node| node.revocation_pending)
        };
        if revocation_pending {
            self.complete_trust_observation_revocation(base_url.as_str())
                .await
                .context("previous trust observation revocation has not completed")?;
        }
        match self
            .ensure_community_node_session(base_url.as_str())
            .await?
        {
            CommunityNodeSessionOutcome::Ready => {}
            _ => {
                return Err(anyhow!(
                    "community node session must be ready before enabling observation sharing"
                ));
            }
        }
        let policies = self
            .request_community_node_policies(base_url.as_str(), None)
            .await?;
        let policy = sharing_policy(&policies.policies)
            .ok_or_else(|| anyhow!("community node does not offer observation sharing"))?;
        if policy.policy_version != request.policy_version
            || policy.policy_snapshot_revision != request.policy_snapshot_revision
        {
            return Err(anyhow!(
                "observation sharing policy changed; reload the document before accepting"
            ));
        }
        let mut token =
            load_community_node_token(&self.db_path, self.identity_mode, base_url.as_str())?
                .ok_or_else(|| anyhow!("community node authentication is required"))?;
        self.accept_community_node_consents_with_retry(
            base_url.as_str(),
            &mut token,
            &[TRUST_OBSERVATION_SHARING_POLICY_SLUG.to_string()],
            policies.policy_snapshot_revision.as_deref(),
        )
        .await?;
        let mut consents = load_community_node_local_consents(
            &self.db_path,
            self.identity_mode,
            base_url.as_str(),
        )?;
        let withdrawn_at = consents.withdrawn_at;
        record_community_node_local_consents(
            &mut consents,
            &[CommunityNodeConsentDocumentRef {
                policy_slug: policy.policy_slug.clone(),
                policy_version: policy.policy_version,
                policy_snapshot_revision: policy.policy_snapshot_revision.clone(),
            }],
            request.language.as_str(),
            app_version,
            Utc::now().timestamp(),
        );
        // 任意文書の記録は必須文書の撤回状態を解除しない（Ready を確認済みなので通常は None）。
        consents.withdrawn_at = withdrawn_at;
        persist_community_node_local_consents(
            &self.db_path,
            self.identity_mode,
            base_url.as_str(),
            &consents,
        )?;

        // 端末の現在の状態。送信待ちの突き合わせに使い、`include_existing` のときは送る対象にもする。
        let mut current: BTreeSet<String> = BTreeSet::new();
        for (kind, connection) in [
            (TrustObservationKind::Mute, SocialConnectionKind::Muted),
            (TrustObservationKind::Block, SocialConnectionKind::Blocking),
        ] {
            for view in self.app_service.list_social_connections(connection).await? {
                // 正規化できない保存値（旧 version の残骸など）は観測にできないので飛ばす。
                match normalize_pubkey(view.author_pubkey.as_str()) {
                    Ok(pubkey) => {
                        current.insert(pending_key(pubkey.as_str(), kind));
                    }
                    Err(error) => {
                        warn!(error = %error, "skipped a social connection with an invalid pubkey");
                    }
                }
            }
        }
        {
            let _guard = self.trust_observation_guard.lock().await;
            let mut state = load_state(&self.db_path)?;
            let mut pending = BTreeMap::new();
            if request.include_existing {
                for key in &current {
                    let (target, kind) = parse_pending_key(key)?;
                    let envelope = self.sign_observation(&mut state, &target, kind, true)?;
                    pending.insert(key.clone(), envelope);
                }
            }
            let node = state.nodes.entry(base_url.clone()).or_default();
            // 提供が止まっている間の解除は積まれないので、端末の現在の状態と合わない送信待ちは捨てる
            // （まだ送っていない観測なので、CN 側に取り消す行も無い）。
            node.pending
                .retain(|key, envelope| pending_matches_current(key, envelope, &current));
            node.enabled = true;
            node.needs_reconsent = false;
            node.revocation_pending = false;
            // 再同意までの間に積んだ送信待ちは捨てない。既存分（同じ対象・種別）は新しい署名で置き換える。
            node.pending.extend(pending);
            save_state(&self.db_path, &state)?;
        }
        self.flush_community_node_trust_observations_for(&base_url)
            .await;
        self.observation_sharing_status(base_url, Some(policy))
            .await
    }

    /// 観測提供を止め、保存済み観測の削除を要求する。
    pub async fn disable_community_node_observation_sharing(
        &self,
        request: CommunityNodeTargetRequest,
    ) -> Result<CommunityNodeObservationSharingStatus> {
        let base_url = normalize_http_url(request.base_url.as_str())?;
        self.require_community_node(base_url.as_str()).await?;
        self.revoke_community_node_trust_observations(base_url.as_str())
            .await?;
        self.get_community_node_observation_sharing(CommunityNodeTargetRequest { base_url })
            .await
    }
}

/// テスト用: CN ごとの送信待ち件数（状態が無ければ 0）。network に触れない。
#[cfg(test)]
pub(crate) async fn load_trust_observation_pending_count(
    runtime: &DesktopRuntime,
    base_url: &str,
) -> usize {
    let _guard = runtime.trust_observation_guard.lock().await;
    load_state(&runtime.db_path)
        .expect("trust observation state")
        .nodes
        .get(base_url)
        .map_or(0, |node| node.pending.len())
}

/// 同意ダイアログで一括受諾する文書から、観測提供の任意文書を外す（提供は専用の操作でだけ同意する）。
pub(crate) fn without_observation_sharing_document(
    documents: &[CommunityNodeConsentDocumentRef],
) -> Vec<CommunityNodeConsentDocumentRef> {
    documents
        .iter()
        .filter(|document| document.policy_slug != TRUST_OBSERVATION_SHARING_POLICY_SLUG)
        .cloned()
        .collect()
}
