use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::identity::write_private_file_atomically;

const DESIRED_SUBSCRIPTIONS_VERSION: u32 = 1;
const DESIRED_SUBSCRIPTIONS_EXTENSION: &str = "subscriptions.json";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesiredSubscriptionScope {
    Public,
    Channel { channel_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredSubscription {
    pub topic: String,
    pub scope: DesiredSubscriptionScope,
}

impl DesiredSubscription {
    pub fn validate(&self) -> Result<(), SubscriptionStateError> {
        validate_identifier("topic", &self.topic)?;
        if let DesiredSubscriptionScope::Channel { channel_id } = &self.scope {
            validate_identifier("channel", channel_id)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionStateErrorKind {
    InvalidSubscription,
    ReadFailed,
    DecodeFailed,
    UnsupportedVersion,
    PersistFailed,
    ActivationFailed,
    /// 購読する scope が上限(列・参加と共通の 64)に達している(#1221 R2-C)。
    LimitReached,
}

impl SubscriptionStateErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidSubscription => "invalid_subscription",
            Self::ReadFailed => "subscription_state_read_failed",
            Self::DecodeFailed => "subscription_state_decode_failed",
            Self::UnsupportedVersion => "subscription_state_version_unsupported",
            Self::PersistFailed => "subscription_state_persist_failed",
            Self::ActivationFailed => "subscription_activation_failed",
            Self::LimitReached => "subscription_limit_reached",
        }
    }
}

#[derive(Debug)]
pub struct SubscriptionStateError {
    pub kind: SubscriptionStateErrorKind,
    pub message: String,
}

impl SubscriptionStateError {
    pub(crate) fn new(kind: SubscriptionStateErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 購読の開始の失敗。上限による拒否は `LimitReached` にする。
    pub(crate) fn activation(error: anyhow::Error) -> Self {
        if error
            .downcast_ref::<kukuri_app_api::ScopeLimitReached>()
            .is_some()
        {
            return Self::limit_reached();
        }
        Self::new(
            SubscriptionStateErrorKind::ActivationFailed,
            format!("failed to activate desired subscription: {error:#}"),
        )
    }

    pub(crate) fn limit_reached() -> Self {
        Self::new(
            SubscriptionStateErrorKind::LimitReached,
            kukuri_app_api::ScopeLimitReached.to_string(),
        )
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl std::fmt::Display for SubscriptionStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SubscriptionStateError {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesiredSubscriptionStore {
    version: u32,
    subscriptions: Vec<DesiredSubscription>,
}

pub fn desired_subscriptions_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(DESIRED_SUBSCRIPTIONS_EXTENSION)
}

pub(crate) fn load_desired_subscriptions(
    db_path: &Path,
) -> Result<Vec<DesiredSubscription>, SubscriptionStateError> {
    let path = desired_subscriptions_path(db_path);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(SubscriptionStateError::new(
                SubscriptionStateErrorKind::ReadFailed,
                format!("failed to read `{}`: {error}", path.display()),
            ));
        }
    };
    let mut store: DesiredSubscriptionStore = serde_json::from_slice(&bytes).map_err(|error| {
        SubscriptionStateError::new(
            SubscriptionStateErrorKind::DecodeFailed,
            format!("failed to decode `{}`: {error}", path.display()),
        )
    })?;
    if store.version != DESIRED_SUBSCRIPTIONS_VERSION {
        return Err(SubscriptionStateError::new(
            SubscriptionStateErrorKind::UnsupportedVersion,
            format!(
                "unsupported desired subscription version `{}`",
                store.version
            ),
        ));
    }
    for subscription in &store.subscriptions {
        subscription.validate()?;
    }
    store.subscriptions.sort();
    store.subscriptions.dedup();
    // 保存は 64 件まで。超える file は先頭の 64 件だけを使う(#1221 R2-C)。
    store
        .subscriptions
        .truncate(kukuri_app_api::MAX_ACTIVE_SCOPES);
    Ok(store.subscriptions)
}

pub(crate) fn save_desired_subscriptions(
    db_path: &Path,
    subscriptions: &[DesiredSubscription],
) -> Result<(), SubscriptionStateError> {
    let mut subscriptions = subscriptions.to_vec();
    for subscription in &subscriptions {
        subscription.validate()?;
    }
    subscriptions.sort();
    subscriptions.dedup();
    let bytes = serde_json::to_vec_pretty(&DesiredSubscriptionStore {
        version: DESIRED_SUBSCRIPTIONS_VERSION,
        subscriptions,
    })
    .map_err(|error| {
        SubscriptionStateError::new(
            SubscriptionStateErrorKind::PersistFailed,
            format!("failed to encode desired subscriptions: {error}"),
        )
    })?;
    let path = desired_subscriptions_path(db_path);
    write_private_file_atomically(&path, &bytes).map_err(|error| {
        SubscriptionStateError::new(
            SubscriptionStateErrorKind::PersistFailed,
            format!("failed to persist `{}`: {error:#}", path.display()),
        )
    })
}

fn validate_identifier(label: &str, value: &str) -> Result<(), SubscriptionStateError> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_whitespace) {
        Err(SubscriptionStateError::new(
            SubscriptionStateErrorKind::InvalidSubscription,
            format!("{label} identifier is invalid"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(topic: &str) -> DesiredSubscription {
        DesiredSubscription {
            topic: topic.to_string(),
            scope: DesiredSubscriptionScope::Public,
        }
    }

    #[test]
    fn subscription_state_round_trips_in_canonical_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("kukuri.db");
        save_desired_subscriptions(&db_path, &[public("z"), public("a"), public("z")])
            .expect("save");
        assert_eq!(
            load_desired_subscriptions(&db_path).expect("load"),
            vec![public("a"), public("z")]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(desired_subscriptions_path(&db_path))
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn malformed_and_unknown_version_state_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("kukuri.db");
        let path = desired_subscriptions_path(&db_path);
        std::fs::write(&path, b"not-json").expect("fixture");
        assert_eq!(
            load_desired_subscriptions(&db_path)
                .expect_err("malformed")
                .kind,
            SubscriptionStateErrorKind::DecodeFailed
        );
        std::fs::write(&path, br#"{"version":2,"subscriptions":[]}"#).expect("fixture");
        assert_eq!(
            load_desired_subscriptions(&db_path)
                .expect_err("unknown version")
                .kind,
            SubscriptionStateErrorKind::UnsupportedVersion
        );
    }
}
