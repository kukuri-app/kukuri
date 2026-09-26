use kukuri_desktop_runtime::{
    CommunityNodeIndexQueryError, CommunityNodeIndexingRequestError, CommunityNodeReportError,
    CommunityNodeTesterFeedbackError, CommunityNodeTrustRelationError, DomeHostingRequestError,
    ScopeLimitReached, SubscriptionStateError, SubscriptionStateErrorKind,
};
use serde_json::json;

use crate::protocol::{ProtocolError, error_code};

pub(super) fn command_error(error: anyhow::Error) -> ProtocolError {
    if let Some(error) = error.downcast_ref::<CommunityNodeIndexQueryError>() {
        return node_error(&error.code, error.status, error.retry_after_seconds);
    }
    if let Some(error) = error.downcast_ref::<CommunityNodeIndexingRequestError>() {
        return node_error(&error.code, error.status, error.retry_after_seconds);
    }
    if let Some(error) = error.downcast_ref::<CommunityNodeTesterFeedbackError>() {
        return node_error(&error.code, error.status, error.retry_after_seconds);
    }
    if let Some(error) = error.downcast_ref::<CommunityNodeTrustRelationError>() {
        return node_error(&error.code, error.status, None);
    }
    if let Some(error) = error.downcast_ref::<CommunityNodeReportError>() {
        return node_error(&error.code, error.status, None);
    }
    if let Some(error) = error.downcast_ref::<DomeHostingRequestError>() {
        return node_error(&error.code, Some(error.status), None);
    }
    // #1221 R2-C: 購読・参加の上限(desired と参加の lease で共通の 64)。
    if error.downcast_ref::<ScopeLimitReached>().is_some()
        || error
            .downcast_ref::<SubscriptionStateError>()
            .is_some_and(|error| error.kind == SubscriptionStateErrorKind::LimitReached)
    {
        return ProtocolError::new(
            error_code::VALIDATION_FAILED,
            "購読と参加の合計が上限(64)に達しています",
        );
    }
    // 既存serviceの固定エラーだけを分類する。生のmessage/contextや、
    // 相手の本文・識別子を含み得るHTTP/serdeエラーは診断へ転記しない。
    match error.to_string().as_str() {
        "timed out waiting for invite-only channel replica sync"
        | "timed out waiting for friend-only channel replica sync"
        | "timed out waiting for friend-plus channel replica sync" => ProtocolError::new(
            error_code::NETWORK_UNAVAILABLE,
            "private channelの同期が完了していません",
        ),
        "invite-only access token epoch does not match the current policy"
        | "friend-only grant epoch does not match the current policy"
        | "friend-plus share epoch does not match the current policy" => ProtocolError::new(
            error_code::AUTHORIZATION_FAILED,
            "資格情報の鍵世代が現在の方針と一致しません",
        ),
        "community node consent is required before authentication" => ProtocolError::new(
            error_code::CONSENT_REQUIRED,
            "Community Nodeへの認証には同意が必要です",
        ),
        "only the game room owner can update the room" => ProtocolError::new(
            error_code::AUTHORIZATION_FAILED,
            "Gameの更新には所有者権限が必要です",
        ),
        "direct message requires a mutual relationship" => ProtocolError::new(
            error_code::AUTHORIZATION_FAILED,
            "DMには相互フォローが必要です",
        ),
        "direct message text or attachment is required" => ProtocolError::new(
            error_code::VALIDATION_FAILED,
            "DM本文または添付ファイルが必要です",
        ),
        "direct message reply target was not found" => {
            ProtocolError::new(error_code::NOT_FOUND, "返信先のDMが見つかりません")
        }
        _ => ProtocolError::new(error_code::INTERNAL_ERROR, "操作を完了できませんでした"),
    }
}

fn node_error(
    domain_code: &str,
    status: Option<u16>,
    retry_after_seconds: Option<u64>,
) -> ProtocolError {
    let code = match (domain_code, status) {
        ("CONSENT_REQUIRED", _) => error_code::CONSENT_REQUIRED,
        ("AUTH_REQUIRED", _) | (_, Some(401 | 403)) => error_code::AUTHORIZATION_FAILED,
        (_, Some(400 | 422)) => error_code::VALIDATION_FAILED,
        (_, Some(404)) => error_code::NOT_FOUND,
        (_, Some(409)) => error_code::CONFLICT,
        (_, Some(429)) => error_code::BACKPRESSURE,
        (_, Some(500..=599)) => error_code::NETWORK_UNAVAILABLE,
        _ => error_code::INTERNAL_ERROR,
    };
    ProtocolError::new(code, "Community Nodeへの操作を完了できませんでした")
        .with_details(json!({"status": status, "retry_after_seconds": retry_after_seconds}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_replica_sync_timeout_is_distinct_from_credential_rejection() {
        let timeout = command_error(anyhow::anyhow!(
            "timed out waiting for invite-only channel replica sync"
        ));
        assert_eq!(timeout.code, error_code::NETWORK_UNAVAILABLE);
        let rejected = command_error(anyhow::anyhow!(
            "invite-only access token epoch does not match the current policy"
        ));
        assert_eq!(rejected.code, error_code::AUTHORIZATION_FAILED);
    }

    #[test]
    fn the_scope_limit_is_a_validation_failure() {
        let error = command_error(ScopeLimitReached.into());
        assert_eq!(error.code, error_code::VALIDATION_FAILED);
        assert!(!error.message.contains("SCOPE_LIMIT_REACHED"));
    }

    #[test]
    fn node_error_preserves_status_and_retry_without_remote_diagnostics() {
        for (status, domain_code, expected) in [
            (401, "AUTH_REQUIRED", error_code::AUTHORIZATION_FAILED),
            (403, "CONSENT_REQUIRED", error_code::CONSENT_REQUIRED),
            (429, "RATE_LIMITED", error_code::BACKPRESSURE),
            (503, "remote-code-sentinel", error_code::NETWORK_UNAVAILABLE),
        ] {
            let error = command_error(
                CommunityNodeIndexQueryError {
                    code: domain_code.into(),
                    message: "private-body-sentinel".into(),
                    status: Some(status),
                    retry_after_seconds: Some(7),
                }
                .into(),
            );
            assert_eq!(error.code, expected);
            assert_eq!(
                error.details,
                Some(json!({"status": status, "retry_after_seconds": 7}))
            );
            assert!(!error.message.contains("sentinel"));
            assert!(!error.details.unwrap().to_string().contains("sentinel"));
        }
    }
}
