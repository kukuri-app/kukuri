use super::*;
use kukuri_cn_protocol::CONSENT_REQUIRED_CODE;
use std::fmt;

/// 分散通報ルーティング（#310）の通報送信リクエスト。
///
/// 通報先は中央窓口ではなく、対象を実際に表示・索引・moderation・cache 等した
/// community node の manifest（#310 で追加した `report_endpoint`）から解決される。
/// client 側（provenance + manifest）で通報先を解決し、その `report_endpoint` を
/// この request に載せて渡す。endpoint が無い node は client 側で `abuse_contact`
/// 案内に切り替えるため、この command には到達しない。
///
/// 実行時層は画面側の判定を信頼せず、`node_base_url` が構成済みノードであることと、
/// `report_endpoint` のオリジンが `node_base_url` と一致することを送信前に強制する(#703)。
/// 提供中能力・責任範囲の照合は画面側(`reportRouting.ts`、#702)が正であり、ここでは行わない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(rename_all = "snake_case")]
pub struct SubmitCommunityNodeReportRequest {
    /// 通報先 node の base url。構成済みノードでなければ送信しない(#703)。
    pub node_base_url: String,
    /// node manifest が公開する通報受付 endpoint（絶対 http(s) URL）。
    /// `node_base_url` と同一オリジンでなければ送信しない(#703)。
    pub report_endpoint: String,
    /// 通報対象の種別（post / profile / media / search_result / recommendation 等）。
    pub subject_kind: String,
    /// 通報対象の識別子（post id / pubkey / object id 等）。
    pub subject_id: String,
    /// 通報先となった node capability（community_index / moderation / media_cache 等）。
    pub capability: String,
    /// 通報理由カテゴリ。
    pub reason: String,
    /// 任意の補足説明。
    #[serde(default)]
    pub details: Option<String>,
    /// 任意の通報者連絡先（node が follow-up に使える）。
    #[serde(default)]
    pub reporter_contact: Option<String>,
    /// リスク判定への異議申し立て。通常の通報では省略する。
    #[serde(default)]
    pub appeal: Option<CommunityNodeReportAppeal>,
}

/// コミュニティノードへの通報送信で返る、機械判定可能な失敗。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityNodeReportError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
}

impl CommunityNodeReportError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            status: None,
        }
    }

    fn from_response(status: StatusCode, body: Option<ApiErrorBody>) -> Self {
        let fallback_code = match status {
            StatusCode::NOT_FOUND => "REPORT_NOT_CONFIGURED",
            StatusCode::BAD_REQUEST => "INVALID_REPORT",
            StatusCode::TOO_MANY_REQUESTS => "RATE_LIMITED",
            _ => "REPORT_SUBMISSION_FAILED",
        };
        let fallback_message = format!("通報の送信に失敗しました（{status}）");
        Self {
            code: body
                .as_ref()
                .map_or_else(|| fallback_code.to_string(), |body| body.code.clone()),
            message: body.map_or(fallback_message, |body| body.message),
            status: Some(status.as_u16()),
        }
    }
}

impl fmt::Display for CommunityNodeReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommunityNodeReportError {}

/// 通報送信結果の状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SubmitCommunityNodeReportStatus {
    /// node が通報を受理した（2xx）。
    Submitted,
}

/// 通報送信結果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SubmitCommunityNodeReportResult {
    pub status: SubmitCommunityNodeReportStatus,
    /// node が返した受付参照 ID（任意）。
    #[serde(default)]
    pub reference_id: Option<String>,
    /// 異議申し立てが受理され、`Disputed` へ移ったリスク判定の識別子。
    #[serde(default)]
    pub disputed_risk_signal_id: Option<String>,
}

/// report endpoint が POST 可能な http(s) 絶対 URL か検証する。
///
/// 空文字や非 http(s) は拒否する。client は endpoint が無い node を `abuse_contact`
/// 案内へ切り替えるため、ここに到達する endpoint は manifest 由来の絶対 URL である。
pub(crate) fn validate_report_endpoint(endpoint: &str) -> Result<&str> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("report endpoint is empty"));
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(anyhow!("report endpoint must be an http(s) URL"));
    }
    Ok(trimmed)
}

/// `report_endpoint` のオリジン(scheme / host / port)が、正規化済みの `node_base_url` と
/// 一致することを確認する(#703)。実運用の公開ノード情報は受付先を基底アドレスと同一オリジンの
/// `/v1/report` に置くため、別オリジンの受付先は画面側の不具合か改変された呼び出しとみなす。
pub(crate) fn ensure_report_endpoint_origin(
    normalized_base_url: &str,
    endpoint: &str,
) -> Result<()> {
    let base = url::Url::parse(normalized_base_url)
        .with_context(|| format!("invalid community node base url `{normalized_base_url}`"))?;
    let target = url::Url::parse(endpoint)
        .with_context(|| format!("invalid report endpoint `{endpoint}`"))?;
    let same_origin = base.scheme() == target.scheme()
        && base.host_str().map(str::to_ascii_lowercase)
            == target.host_str().map(str::to_ascii_lowercase)
        && base.port_or_known_default() == target.port_or_known_default();
    if !same_origin {
        return Err(anyhow!(
            "report endpoint `{endpoint}` is not on the same origin as community node `{normalized_base_url}`"
        ));
    }
    Ok(())
}

impl DesktopRuntime {
    /// 解決済みの通報先 node の report endpoint へ通報を POST する（#310）。
    ///
    /// この経路は中央集約ではなく、対象に実際に関与した node の authority scope 内へ
    /// 通報を届けるためのものである。endpoint が無い node 宛ては呼び出し側で
    /// `abuse_contact` 案内に切り替えるため、この method には渡らない。
    pub(crate) async fn request_community_node_report_submit(
        &self,
        request: &SubmitCommunityNodeReportRequest,
    ) -> Result<SubmitCommunityNodeReportResult, CommunityNodeReportError> {
        let endpoint = validate_report_endpoint(&request.report_endpoint).map_err(|error| {
            CommunityNodeReportError::new("INVALID_REPORT_ENDPOINT", error.to_string())
        })?;
        // 構成情報にないノード、基底アドレスと異なるオリジンの受付先へは送らない(#703)。
        let base_url = normalize_http_url(request.node_base_url.as_str()).map_err(|error| {
            CommunityNodeReportError::new("REPORT_TARGET_NOT_CONFIGURED", error.to_string())
        })?;
        self.require_community_node(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeReportError::new("REPORT_TARGET_NOT_CONFIGURED", error.to_string())
            })?;
        ensure_report_endpoint_origin(base_url.as_str(), endpoint).map_err(|error| {
            CommunityNodeReportError::new("REPORT_ENDPOINT_MISMATCH", error.to_string())
        })?;
        let preflight = self
            .preflight_community_node_consent(base_url.as_str())
            .await
            .map_err(|error| {
                CommunityNodeReportError::new("REPORT_CONSENT_CHECK_FAILED", error.to_string())
            })?;
        if matches!(preflight, CommunityNodeConsentPreflight::Required { .. }) {
            return Err(CommunityNodeReportError::new(
                CONSENT_REQUIRED_CODE,
                "current community node policies must be accepted before report submission",
            ));
        }
        if self
            .community_node_required_consent_is_pending(base_url.as_str())
            .await
        {
            return Err(CommunityNodeReportError::new(
                CONSENT_REQUIRED_CODE,
                "community node required policies must be accepted before report submission",
            ));
        }
        let payload = CommunityNodeReportRequest {
            subject_kind: request.subject_kind.clone(),
            subject_id: request.subject_id.clone(),
            capability: request.capability.clone(),
            reason: request.reason.clone(),
            details: request.details.clone(),
            reporter_contact: request.reporter_contact.clone(),
            appeal: request.appeal.clone(),
        };
        let payload = serde_json::to_vec(&payload).map_err(|error| {
            CommunityNodeReportError::new("REPORT_SUBMISSION_FAILED", error.to_string())
        })?;
        let (status, body) = post_report(endpoint, payload).await.map_err(|error| {
            CommunityNodeReportError::new("REPORT_SUBMISSION_FAILED", format!("{error:#}"))
        })?;
        let status = StatusCode::from_u16(status).map_err(|error| {
            CommunityNodeReportError::new("REPORT_SUBMISSION_FAILED", error.to_string())
        })?;
        if status.is_redirection() {
            // 転送は追跡しない。受付先の境界を越えて本文を再送しないために拒否する(#703)。
            return Err(CommunityNodeReportError {
                code: "REPORT_REDIRECT_REJECTED".to_string(),
                message: format!("report endpoint responded with a redirect ({status})"),
                status: Some(status.as_u16()),
            });
        }
        if !status.is_success() {
            let body = serde_json::from_slice::<ApiErrorBody>(&body).ok();
            return Err(CommunityNodeReportError::from_response(status, body));
        }
        let ack = serde_json::from_slice::<CommunityNodeReportResponse>(&body).unwrap_or_default();
        Ok(SubmitCommunityNodeReportResult {
            status: SubmitCommunityNodeReportStatus::Submitted,
            reference_id: ack.reference_id,
            disputed_risk_signal_id: ack.disputed_risk_signal_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_endpoint_must_share_the_node_origin() {
        assert!(
            ensure_report_endpoint_origin("https://node.example", "https://node.example/v1/report")
                .is_ok()
        );
        assert!(
            ensure_report_endpoint_origin(
                "https://node.example",
                "https://NODE.example:443/v1/report"
            )
            .is_ok()
        );
        assert!(
            ensure_report_endpoint_origin(
                "http://127.0.0.1:8080",
                "http://127.0.0.1:8080/v1/report"
            )
            .is_ok()
        );
        assert!(
            ensure_report_endpoint_origin(
                "https://node.example",
                "https://other.example/v1/report"
            )
            .is_err()
        );
        assert!(
            ensure_report_endpoint_origin("https://node.example", "http://node.example/v1/report")
                .is_err()
        );
        assert!(
            ensure_report_endpoint_origin(
                "http://127.0.0.1:8080",
                "http://127.0.0.1:9090/v1/report"
            )
            .is_err()
        );
    }

    #[test]
    fn validates_http_endpoints() {
        assert!(validate_report_endpoint("https://node.example/v1/report").is_ok());
        assert!(validate_report_endpoint("http://node.example/v1/report").is_ok());
        assert_eq!(
            validate_report_endpoint("  https://node.example/v1/report  ").unwrap(),
            "https://node.example/v1/report"
        );
    }

    #[test]
    fn rejects_empty_or_non_http_endpoints() {
        assert!(validate_report_endpoint("").is_err());
        assert!(validate_report_endpoint("   ").is_err());
        // mailto / file などは POST 経路に乗せない（client が contact 案内へ切り替える）。
        assert!(validate_report_endpoint("mailto:abuse@node.example").is_err());
        assert!(validate_report_endpoint("ftp://node.example/report").is_err());
    }

    #[test]
    fn report_payload_omits_optional_fields_when_absent() {
        let payload = CommunityNodeReportRequest {
            subject_kind: "post".to_string(),
            subject_id: "abc".to_string(),
            capability: "community_index".to_string(),
            reason: "spam".to_string(),
            details: None,
            reporter_contact: None,
            appeal: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["subject_kind"], "post");
        assert_eq!(json["capability"], "community_index");
        assert!(json.get("details").is_none());
        assert!(json.get("reporter_contact").is_none());
        assert!(json.get("appeal").is_none());
    }

    #[test]
    fn report_payload_includes_appeal_without_reporter_identity() {
        let appeal = CommunityNodeReportAppeal {
            risk_signal_id: "signal-1".to_string(),
        };
        let payload = CommunityNodeReportRequest {
            subject_kind: "profile".to_string(),
            subject_id: "author-pubkey".to_string(),
            capability: "trust_signal".to_string(),
            reason: "other".to_string(),
            details: Some("誤検知として再確認を求めます".to_string()),
            reporter_contact: None,
            appeal: Some(appeal),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["appeal"]["risk_signal_id"], "signal-1");
        assert!(json.get("reporter_contact").is_none());
    }

    #[test]
    fn request_deserializes_from_snake_case() {
        let request: SubmitCommunityNodeReportRequest = serde_json::from_str(
            r#"{
                "node_base_url": "https://node.example",
                "report_endpoint": "https://node.example/v1/report",
                "subject_kind": "post",
                "subject_id": "abc",
                "capability": "community_index",
                "reason": "spam"
            }"#,
        )
        .unwrap();
        assert_eq!(request.node_base_url, "https://node.example");
        assert_eq!(request.capability, "community_index");
        assert!(request.details.is_none());
        assert!(request.appeal.is_none());
    }

    #[test]
    fn result_serializes_status_snake_case() {
        let result = SubmitCommunityNodeReportResult {
            status: SubmitCommunityNodeReportStatus::Submitted,
            reference_id: Some("ref-1".to_string()),
            disputed_risk_signal_id: Some("signal-1".to_string()),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "submitted");
        assert_eq!(json["reference_id"], "ref-1");
        assert_eq!(json["disputed_risk_signal_id"], "signal-1");
    }
}
