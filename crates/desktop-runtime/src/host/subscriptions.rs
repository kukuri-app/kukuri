use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DESIRED_SUBSCRIPTIONS_VERSION: u32 = 1;
const DESIRED_SUBSCRIPTIONS_EXTENSION: &str = "subscriptions.json";
/// 公開スレッド（公開 channel）の購読は別の file に置く（#1982）。旧版は `subscriptions.json` の channel を private channel
/// として復元し、参加していないので起動に失敗するため、旧版の読まない file へ分ける（2026-10-31 のユーザー判断）。
const PUBLIC_CHANNEL_SUBSCRIPTIONS_EXTENSION: &str = "public-channel-subscriptions.json";

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
    fn is_public_channel(&self) -> bool {
        matches!(&self.scope, DesiredSubscriptionScope::Channel { channel_id }
            if kukuri_core::ChannelId::new(channel_id).is_public_channel())
    }

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

pub(crate) async fn load_desired_subscriptions(
    db_path: &Path,
) -> Result<Vec<DesiredSubscription>, SubscriptionStateError> {
    let mut subscriptions = read_store(&desired_subscriptions_path(db_path)).await?;
    subscriptions
        .extend(read_store(&db_path.with_extension(PUBLIC_CHANNEL_SUBSCRIPTIONS_EXTENSION)).await?);
    subscriptions.sort();
    subscriptions.dedup();
    // 保存は 64 件まで。超える file は先頭の 64 件だけを使う(#1221 R2-C)。
    subscriptions.truncate(kukuri_app_api::MAX_ACTIVE_SCOPES);
    Ok(subscriptions)
}

async fn read_store(path: &Path) -> Result<Vec<DesiredSubscription>, SubscriptionStateError> {
    let bytes = match crate::storage::read_file(path).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(Vec::new()),
        Err(error) => {
            return Err(SubscriptionStateError::new(
                SubscriptionStateErrorKind::ReadFailed,
                format!("failed to read `{}`: {error:#}", path.display()),
            ));
        }
    };
    let store: DesiredSubscriptionStore = serde_json::from_slice(&bytes).map_err(|error| {
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
    Ok(store.subscriptions)
}

pub(crate) async fn save_desired_subscriptions(
    db_path: &Path,
    subscriptions: &[DesiredSubscription],
) -> Result<(), SubscriptionStateError> {
    let mut subscriptions = subscriptions.to_vec();
    for subscription in &subscriptions {
        subscription.validate()?;
    }
    subscriptions.sort();
    subscriptions.dedup();
    let (public_channels, others): (Vec<_>, Vec<_>) = subscriptions
        .into_iter()
        .partition(DesiredSubscription::is_public_channel);
    write_store(&desired_subscriptions_path(db_path), others).await?;
    write_store(
        &db_path.with_extension(PUBLIC_CHANNEL_SUBSCRIPTIONS_EXTENSION),
        public_channels,
    )
    .await
}

async fn write_store(
    path: &Path,
    subscriptions: Vec<DesiredSubscription>,
) -> Result<(), SubscriptionStateError> {
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
    crate::storage::write_file(path, &bytes)
        .await
        .map_err(|error| {
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

    #[tokio::test]
    async fn subscription_state_round_trips_in_canonical_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("kukuri.db");
        save_desired_subscriptions(&db_path, &[public("z"), public("a"), public("z")])
            .await
            .expect("save");
        assert_eq!(
            load_desired_subscriptions(&db_path).await.expect("load"),
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

    /// 公開スレッドの購読は旧版の読まない file に置き、読むときは合わせる（#1982）。
    #[tokio::test]
    async fn public_channel_subscriptions_live_in_their_own_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("kukuri.db");
        let thread = DesiredSubscription {
            topic: "t".to_string(),
            scope: DesiredSubscriptionScope::Channel {
                channel_id: kukuri_core::ChannelId::public_channel(1, &"a".repeat(64))
                    .as_str()
                    .to_string(),
            },
        };
        save_desired_subscriptions(&db_path, &[public("t"), thread.clone()])
            .await
            .expect("save");
        assert_eq!(
            read_store(&desired_subscriptions_path(&db_path))
                .await
                .expect("old file"),
            vec![public("t")],
            "the file older versions read has no public channel"
        );
        let mut both = load_desired_subscriptions(&db_path).await.expect("load");
        both.sort();
        let mut expected = vec![public("t"), thread];
        expected.sort();
        assert_eq!(both, expected);
    }

    #[tokio::test]
    async fn malformed_and_unknown_version_state_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("kukuri.db");
        let path = desired_subscriptions_path(&db_path);
        std::fs::write(&path, b"not-json").expect("fixture");
        assert_eq!(
            load_desired_subscriptions(&db_path)
                .await
                .expect_err("malformed")
                .kind,
            SubscriptionStateErrorKind::DecodeFailed
        );
        std::fs::write(&path, br#"{"version":2,"subscriptions":[]}"#).expect("fixture");
        assert_eq!(
            load_desired_subscriptions(&db_path)
                .await
                .expect_err("unknown version")
                .kind,
            SubscriptionStateErrorKind::UnsupportedVersion
        );
    }
}
