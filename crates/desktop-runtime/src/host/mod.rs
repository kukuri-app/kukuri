mod accounts;
#[cfg(test)]
mod accounts_tests;
mod consent;
mod consent_acceptance;
mod profile;
mod restore_lifecycle;
mod subscriptions;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::{sync::broadcast, task::JoinHandle};

use crate::{
    AccountRecord, CommunityNodeConfig, DesktopRuntime, RuntimeEvent, StoreStartupError,
    account_db_path, ensure_accounts_initialized_from_env, list_accounts, set_active_account,
};

pub use consent::{
    AGE_ATTESTATION_VERSION, APP_LEGAL_AUTHORITATIVE_LANGUAGE, APP_LEGAL_DOCUMENTS,
    APP_LEGAL_EFFECTIVE_DATE, AgeAttestationRecord, AgeAttestationStatus, AppConsentDocumentRecord,
    AppConsentDocumentStatus, AppConsentStore, ClientStartupErrorKind, ClientStartupErrorView,
    ClientStartupStatus, LEGAL_BUNDLE_VERSION, age_attestation_satisfied, age_attestation_status,
    app_consent_documents_satisfied, app_consent_documents_status, app_consent_path,
    app_consent_satisfied, consent_required_status, current_unix_seconds, load_app_consent_store,
    reset_app_consent_at_path, save_app_consent_store,
};
pub use consent_acceptance::{
    AcceptedAppConsentDocument, AppConsentStatus, app_consent_status, record_app_consents,
    require_consent_acceptance_state, validate_app_consent_documents,
};
pub use profile::{
    ClientProfile, ClientProfileKind, ProfileError, ProfileErrorKind, ProfileLease, gui_profile,
    resolve_cli_profile,
};
pub use restore_lifecycle::{
    ClientOperationState, RestoreActivationFailure, RestoreActivationOrchestrationFailure,
    RestoreStartupAction, advance_committed_restore_to_consent, orchestrate_restore_activation,
    persist_restore_activation_phase, recover_device_restore_before_startup,
    require_runtime_operation_ready, restore_startup_action, runtime_access_allowed,
};
pub(crate) use subscriptions::load_desired_subscriptions;
#[cfg(test)]
pub(crate) use subscriptions::save_desired_subscriptions;
pub use subscriptions::{
    DesiredSubscription, DesiredSubscriptionScope, SubscriptionStateError,
    SubscriptionStateErrorKind, desired_subscriptions_path,
};

#[derive(Debug)]
pub struct ClientStartupError {
    pub kind: ClientStartupErrorKind,
    pub message: String,
}

impl ClientStartupError {
    pub fn unknown(message: String) -> Self {
        Self {
            kind: ClientStartupErrorKind::Unknown,
            message,
        }
    }

    pub fn from_error(error: anyhow::Error) -> Self {
        let kind = match error.downcast_ref::<StoreStartupError>() {
            Some(StoreStartupError::Migration(_)) => ClientStartupErrorKind::DatabaseMigration,
            Some(StoreStartupError::Open { .. }) => ClientStartupErrorKind::DatabaseOpen,
            None => ClientStartupErrorKind::Unknown,
        };
        Self {
            kind,
            message: format!("{error:#}"),
        }
    }

    pub fn from_profile_error(error: ProfileError) -> Self {
        let kind = match error.kind {
            ProfileErrorKind::ProfileInUse => ClientStartupErrorKind::ProfileInUse,
            _ => ClientStartupErrorKind::ProfileInvalid,
        };
        Self {
            kind,
            message: format!("{}: {}", error.code(), error),
        }
    }

    pub fn from_subscription_error(error: SubscriptionStateError) -> Self {
        Self {
            kind: ClientStartupErrorKind::SubscriptionState,
            message: format!("{}: {}", error.code(), error),
        }
    }
}

impl std::fmt::Display for ClientStartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub fn failed_startup_status(
    error: ClientStartupError,
    db_path: Option<PathBuf>,
) -> ClientStartupStatus {
    ClientStartupStatus::Failed {
        error: ClientStartupErrorView {
            kind: error.kind,
            message: "kukuri could not open the local app database.".to_string(),
            detail: error.message,
            db_path: db_path.map(|path| path.display().to_string()),
        },
    }
}

pub struct ClientStartupState {
    status: tokio::sync::watch::Sender<ClientStartupStatus>,
}

impl ClientStartupState {
    pub fn initializing() -> Self {
        Self::new(ClientStartupStatus::Initializing)
    }

    pub fn new(status: ClientStartupStatus) -> Self {
        let (status, _) = tokio::sync::watch::channel(status);
        Self { status }
    }

    pub fn status(&self) -> ClientStartupStatus {
        self.status.borrow().clone()
    }

    pub fn set_status(&self, next: ClientStartupStatus) {
        self.status.send_replace(next);
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<ClientStartupStatus> {
        self.status.subscribe()
    }
}

/// UI adapterに依存せず、一つのactive account runtimeとevent転送を所有する。
pub struct ClientHost {
    runtime: RwLock<Arc<DesktopRuntime>>,
    app_data_dir: PathBuf,
    events: broadcast::Sender<RuntimeEvent>,
    event_state: Arc<std::sync::Mutex<ClientEventState>>,
    event_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    operation_guard: tokio::sync::Mutex<()>,
    shutdown_guard: tokio::sync::Mutex<()>,
    shutdown_started: AtomicBool,
}

pub enum ClientHostStart {
    Ready(Arc<ClientHost>),
    ConsentRequired(ClientStartupStatus),
}

#[derive(Default)]
struct ClientEventState {
    latest_sync_status: Option<RuntimeEvent>,
}

/// 新規購読者には直近のsync状態を一度再送し、その後のeventをbroadcastで配信する。
pub struct ClientEventReceiver {
    initial: Option<RuntimeEvent>,
    receiver: broadcast::Receiver<RuntimeEvent>,
}

impl ClientEventReceiver {
    pub async fn recv(&mut self) -> Result<RuntimeEvent, broadcast::error::RecvError> {
        if let Some(initial) = self.initial.take() {
            return Ok(initial);
        }
        self.receiver.recv().await
    }
}

impl ClientHost {
    pub async fn start_if_consented(
        app_data_dir: PathBuf,
    ) -> Result<ClientHostStart, ClientStartupError> {
        let consent = load_app_consent_store(&app_data_dir.join(crate::paths::DB_FILE_NAME));
        if !app_consent_satisfied(&consent) {
            return Ok(ClientHostStart::ConsentRequired(consent_required_status(
                &consent,
            )));
        }
        Self::start(app_data_dir).await.map(ClientHostStart::Ready)
    }

    async fn start(app_data_dir: PathBuf) -> Result<Arc<Self>, ClientStartupError> {
        let db_path = ensure_accounts_initialized_from_env(&app_data_dir)
            .map_err(ClientStartupError::from_error)?;
        let runtime = Self::build_detached_runtime(db_path).await?;
        Self::from_runtime(app_data_dir, runtime).await
    }

    pub async fn from_runtime(
        app_data_dir: PathBuf,
        runtime: Arc<DesktopRuntime>,
    ) -> Result<Arc<Self>, ClientStartupError> {
        let desired = match subscriptions::load_desired_subscriptions(runtime.db_path()) {
            Ok(desired) => desired,
            Err(error) => {
                runtime.shutdown().await;
                return Err(ClientStartupError::from_subscription_error(error));
            }
        };
        let (events, _) = broadcast::channel(64);
        let host = Arc::new(Self {
            runtime: RwLock::new(runtime.clone()),
            app_data_dir,
            events,
            event_state: Arc::new(std::sync::Mutex::new(ClientEventState::default())),
            event_task: tokio::sync::Mutex::new(None),
            operation_guard: tokio::sync::Mutex::new(()),
            shutdown_guard: tokio::sync::Mutex::new(()),
            shutdown_started: AtomicBool::new(false),
        });
        runtime.start_community_node_session_scheduler().await;
        host.replace_event_task(runtime).await;
        if let Err(error) = host.restore_desired_subscriptions(&desired).await {
            host.shutdown().await;
            return Err(ClientStartupError::from_subscription_error(error));
        }
        host.runtime().start_sync_status_observer().await;
        host.runtime().start_protected_migration().await;
        Ok(host)
    }

    /// runtimeを構築するが、schedulerとobserverはまだ開始しない。
    /// `from_runtime`または`replace_runtime`へ渡してhostのevent購読後に有効化する。
    pub async fn build_detached_runtime(
        db_path: impl AsRef<Path>,
    ) -> Result<Arc<DesktopRuntime>, ClientStartupError> {
        let initial_community_node_config = distribution_community_node_config()
            .map_err(|error| ClientStartupError::unknown(error.to_string()))?;
        let runtime = DesktopRuntime::from_env(db_path, initial_community_node_config)
            .await
            .map_err(ClientStartupError::from_error)?;
        Ok(Arc::new(runtime))
    }

    pub fn app_data_dir(&self) -> &Path {
        &self.app_data_dir
    }

    pub fn runtime(&self) -> Arc<DesktopRuntime> {
        self.runtime.read().expect("runtime lock poisoned").clone()
    }

    pub fn subscribe_events(&self) -> ClientEventReceiver {
        subscribe_host_events(&self.events, &self.event_state)
    }

    pub async fn replace_runtime(
        &self,
        next: Arc<DesktopRuntime>,
    ) -> Result<Arc<DesktopRuntime>, ClientStartupError> {
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(ClientStartupError::unknown(
                "client host is shutting down".to_string(),
            ));
        }
        self.replace_runtime_locked(next).await
    }

    async fn replace_runtime_locked(
        &self,
        next: Arc<DesktopRuntime>,
    ) -> Result<Arc<DesktopRuntime>, ClientStartupError> {
        let desired = match subscriptions::load_desired_subscriptions(next.db_path()) {
            Ok(desired) => desired,
            Err(error) => {
                next.shutdown().await;
                return Err(ClientStartupError::from_subscription_error(error));
            }
        };
        next.start_community_node_session_scheduler().await;
        let previous = std::mem::replace(
            &mut *self.runtime.write().expect("runtime lock poisoned"),
            next.clone(),
        );
        self.replace_event_task(next.clone()).await;
        if let Err(error) = self.restore_desired_subscriptions(&desired).await {
            let failed = std::mem::replace(
                &mut *self.runtime.write().expect("runtime lock poisoned"),
                previous.clone(),
            );
            self.replace_event_task(previous).await;
            failed.shutdown().await;
            return Err(ClientStartupError::from_subscription_error(error));
        }
        next.start_sync_status_observer().await;
        next.start_protected_migration().await;
        Ok(previous)
    }

    pub async fn restart_runtime(
        &self,
        db_path: impl AsRef<Path>,
    ) -> Result<(), ClientStartupError> {
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(ClientStartupError::unknown(
                "client host is shutting down".to_string(),
            ));
        }
        let next = Self::build_detached_runtime(db_path).await?;
        let previous = self.replace_runtime_locked(next).await?;
        previous.shutdown().await;
        Ok(())
    }

    pub async fn switch_account(&self, account_id: &str) -> anyhow::Result<AccountRecord> {
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            anyhow::bail!("client host is shutting down");
        }
        let snapshot = list_accounts(&self.app_data_dir)?;
        let record = snapshot
            .accounts
            .iter()
            .find(|record| record.id == account_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown account `{account_id}`"))?;
        if snapshot.active_account_id == account_id {
            return Ok(record);
        }

        let db_path = account_db_path(&self.app_data_dir, account_id);
        crate::accounts::verify_persisted_identity(
            &db_path,
            crate::identity::IdentityStorageMode::from_env(),
            &record.pubkey,
        )?;
        let next = Self::build_detached_runtime(db_path)
            .await
            .map_err(|error| anyhow::anyhow!("failed to start the account runtime: {error}"))?;
        let previous = self
            .replace_runtime_locked(next)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let commit = set_active_account(&self.app_data_dir, account_id).map(|_| ());
        self.finish_account_change(previous, &snapshot.active_account_id, record, false, commit)
            .await
    }

    pub fn desired_subscriptions(
        &self,
    ) -> Result<Vec<DesiredSubscription>, SubscriptionStateError> {
        subscriptions::load_desired_subscriptions(self.runtime().db_path())
    }

    pub async fn add_desired_subscription(
        &self,
        subscription: DesiredSubscription,
    ) -> Result<(), SubscriptionStateError> {
        subscription.validate()?;
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(SubscriptionStateError::new(
                SubscriptionStateErrorKind::ActivationFailed,
                "client host is shutting down",
            ));
        }
        let runtime = self.runtime();
        let mut desired = subscriptions::load_desired_subscriptions(runtime.db_path())?;
        if desired.contains(&subscription) {
            return runtime
                .set_desired_subscription(&subscription, true)
                .await
                .map_err(SubscriptionStateError::activation);
        }
        if desired.len() >= kukuri_app_api::MAX_ACTIVE_SCOPES {
            return Err(SubscriptionStateError::limit_reached());
        }
        runtime
            .set_desired_subscription(&subscription, true)
            .await
            .map_err(SubscriptionStateError::activation)?;
        desired.push(subscription.clone());
        if let Err(error) = subscriptions::save_desired_subscriptions(runtime.db_path(), &desired) {
            let _ = runtime.set_desired_subscription(&subscription, false).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn remove_desired_subscription(
        &self,
        subscription: &DesiredSubscription,
    ) -> Result<(), SubscriptionStateError> {
        subscription.validate()?;
        let _guard = self.operation_guard.lock().await;
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(SubscriptionStateError::new(
                SubscriptionStateErrorKind::ActivationFailed,
                "client host is shutting down",
            ));
        }
        let runtime = self.runtime();
        let mut desired = subscriptions::load_desired_subscriptions(runtime.db_path())?;
        if !desired.iter().any(|current| current == subscription) {
            return Ok(());
        }
        desired.retain(|current| current != subscription);
        subscriptions::save_desired_subscriptions(runtime.db_path(), &desired)?;
        runtime
            .set_desired_subscription(subscription, false)
            .await
            .map_err(|error| {
                SubscriptionStateError::new(
                    SubscriptionStateErrorKind::ActivationFailed,
                    format!("failed to remove active subscription: {error:#}"),
                )
            })
    }

    pub async fn shutdown(&self) {
        let _shutdown_guard = self.shutdown_guard.lock().await;
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let _guard = self.operation_guard.lock().await;
        if let Some(task) = self.event_task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        self.runtime().shutdown().await;
    }

    async fn replace_event_task(&self, runtime: Arc<DesktopRuntime>) {
        let mut task = self.event_task.lock().await;
        if let Some(previous) = task.take() {
            previous.abort();
            let _ = previous.await;
        }
        let mut events = runtime.subscribe_events();
        let sender = self.events.clone();
        let event_state = self.event_state.clone();
        *task = Some(tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => forward_host_event(&sender, &event_state, event),
                    Err(broadcast::error::RecvError::Lagged(_)) => forward_host_event(
                        &sender,
                        &event_state,
                        RuntimeEvent::AdultMediaLabelEvicted { hash: None },
                    ),
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }));
    }

    async fn restore_desired_subscriptions(
        &self,
        desired: &[DesiredSubscription],
    ) -> Result<(), SubscriptionStateError> {
        let runtime = self.runtime();
        for subscription in desired {
            match runtime.set_desired_subscription(subscription, true).await {
                Ok(()) => {}
                // 参加中の channel と合わせて上限を超えた分は購読せず、起動は続ける(#1221 R2-C)。
                Err(error)
                    if error
                        .downcast_ref::<kukuri_app_api::ScopeLimitReached>()
                        .is_some() =>
                {
                    tracing::warn!(
                        topic = %subscription.topic,
                        "desired subscription is not restored because the active scopes are full"
                    );
                }
                Err(error) => {
                    return Err(SubscriptionStateError::new(
                        SubscriptionStateErrorKind::ActivationFailed,
                        format!("failed to restore desired subscription: {error:#}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn subscribe_host_events(
    sender: &broadcast::Sender<RuntimeEvent>,
    event_state: &std::sync::Mutex<ClientEventState>,
) -> ClientEventReceiver {
    let state = event_state
        .lock()
        .expect("client event state lock poisoned");
    let receiver = sender.subscribe();
    let initial = state.latest_sync_status.clone();
    ClientEventReceiver { initial, receiver }
}

fn forward_host_event(
    sender: &broadcast::Sender<RuntimeEvent>,
    event_state: &std::sync::Mutex<ClientEventState>,
    event: RuntimeEvent,
) {
    let mut state = event_state
        .lock()
        .expect("client event state lock poisoned");
    if matches!(event, RuntimeEvent::SyncStatusChanged { .. }) {
        state.latest_sync_status = Some(event.clone());
    }
    let _ = sender.send(event);
}

pub fn distribution_community_node_config() -> Result<CommunityNodeConfig, serde_json::Error> {
    serde_json::from_str(include_str!(
        "../../../../apps/desktop/src-tauri/distribution/community-nodes.json"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn late_subscriber_receives_latest_sync_status_once() {
        let (sender, _) = broadcast::channel(4);
        let event_state = std::sync::Mutex::new(ClientEventState::default());
        forward_host_event(
            &sender,
            &event_state,
            RuntimeEvent::SyncStatusChanged {
                sync_status: None,
                community_node_statuses: None,
            },
        );

        let mut receiver = subscribe_host_events(&sender, &event_state);
        assert!(matches!(
            receiver.recv().await,
            Ok(RuntimeEvent::SyncStatusChanged { .. })
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), receiver.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn consent_gate_does_not_initialize_account_or_runtime() {
        let root = tempfile::tempdir().expect("tempdir");
        let app_data_dir = root.path().join("profile");
        let result = ClientHost::start_if_consented(app_data_dir.clone())
            .await
            .expect("consent gate");
        assert!(matches!(result, ClientHostStart::ConsentRequired(_)));
        assert!(!app_data_dir.join("accounts.json").exists());
    }

    #[tokio::test]
    async fn desired_subscription_and_identity_survive_host_restart() {
        let root = tempfile::tempdir().expect("tempdir");
        let db_path = root.path().join("kukuri.db");
        let desired = DesiredSubscription {
            topic: "kukuri:topic:host-restart".to_string(),
            scope: DesiredSubscriptionScope::Public,
        };
        let runtime = Arc::new(DesktopRuntime::new(&db_path).await.expect("first runtime"));
        let first = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
            .await
            .expect("first host");
        first
            .add_desired_subscription(desired.clone())
            .await
            .expect("add desired subscription");
        assert_eq!(
            first.desired_subscriptions().expect("list desired"),
            vec![desired.clone()]
        );
        let first_status = first
            .runtime()
            .get_sync_status()
            .await
            .expect("first status");
        assert!(first_status.subscribed_topics.contains(&desired.topic));
        first.shutdown().await;
        first.shutdown().await;

        let runtime = Arc::new(DesktopRuntime::new(&db_path).await.expect("second runtime"));
        let second = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
            .await
            .expect("second host");
        let second_status = second
            .runtime()
            .get_sync_status()
            .await
            .expect("second status");
        assert_eq!(
            second_status.local_author_pubkey,
            first_status.local_author_pubkey
        );
        assert!(second_status.subscribed_topics.contains(&desired.topic));
        second
            .remove_desired_subscription(&desired)
            .await
            .expect("remove desired subscription");
        assert!(
            second
                .desired_subscriptions()
                .expect("list desired")
                .is_empty()
        );
        assert!(
            !second
                .runtime()
                .get_sync_status()
                .await
                .expect("status after removal")
                .subscribed_topics
                .contains(&desired.topic)
        );
        second.shutdown().await;
    }

    fn public(topic: &str) -> DesiredSubscription {
        DesiredSubscription {
            topic: topic.to_string(),
            scope: DesiredSubscriptionScope::Public,
        }
    }

    fn column(topic: &str) -> crate::ScopeDisplayRequest {
        crate::ScopeDisplayRequest {
            observer: format!("column-{topic}"),
            target: crate::ScopeDisplayTarget::Timeline {
                topic: topic.to_string(),
                scope: kukuri_core::TimelineScope::Public,
            },
            visible: true,
        }
    }

    // #1221 R2-C: desired は 64 件まで。列・参加と上限を共有し、起動時に上限を超えた分は購読しない。
    #[tokio::test]
    async fn desired_subscriptions_are_bounded_and_share_the_scope_limit() {
        let root = tempfile::tempdir().expect("tempdir");
        let db_path = root.path().join("kukuri.db");
        let desired = (0..70)
            .map(|index| public(&format!("kukuri:topic:desired-{index:02}")))
            .collect::<Vec<_>>();
        subscriptions::save_desired_subscriptions(&db_path, &desired).expect("seventy desired");
        let runtime = Arc::new(DesktopRuntime::new(&db_path).await.expect("runtime"));
        runtime
            .set_scope_display(column("kukuri:topic:column"))
            .await
            .expect("an open column");
        let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
            .await
            .expect("startup continues over the limit");
        assert_eq!(
            host.desired_subscriptions().expect("desired"),
            desired[..kukuri_app_api::MAX_ACTIVE_SCOPES].to_vec()
        );
        let subscribed = host
            .runtime()
            .get_sync_status()
            .await
            .expect("status")
            .subscribed_topics;
        assert_eq!(subscribed.len(), kukuri_app_api::MAX_ACTIVE_SCOPES);
        assert!(subscribed.contains(&"kukuri:topic:column".to_string()));
        assert!(!subscribed.contains(&"kukuri:topic:desired-63".to_string()));
        let error = host
            .add_desired_subscription(public("kukuri:topic:extra"))
            .await
            .expect_err("the 65th desired subscription");
        assert_eq!(error.kind, SubscriptionStateErrorKind::LimitReached);
        host.shutdown().await;
    }

    // #1221 R2-C: endpoint を作り直したら、lease のある topic だけを新しい stack で購読し直す。
    #[tokio::test]
    async fn endpoint_rebuild_resubscribes_only_leased_topics() {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime = DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .expect("runtime");
        runtime
            .set_scope_display(column("kukuri:topic:leased"))
            .await
            .expect("an open column");
        runtime
            .create_post(crate::CreatePostRequest {
                topic: "kukuri:topic:written".into(),
                content: "post".into(),
                reply_to: None,
                channel_ref: kukuri_core::ChannelRef::Public,
                attachments: Vec::new(),
                content_labels: Vec::new(),
            })
            .await
            .expect("post");
        let generation = runtime.iroh_stack.generation();
        runtime
            .reapply_community_node_connectivity()
            .await
            .expect("rebuild");
        assert_ne!(runtime.iroh_stack.generation(), generation);
        assert_eq!(
            runtime
                .get_sync_status()
                .await
                .expect("status")
                .subscribed_topics,
            vec!["kukuri:topic:leased".to_string()]
        );
        runtime.shutdown().await;
    }
}
