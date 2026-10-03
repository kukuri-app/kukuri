//! #1214 AC-3: host の runtime の世代。差し替え・停止で世代が進み、前の世代の event は購読側へ届かない。
//! 切替で戻す購読は desired subscriptions だけで、保存履歴・peer candidate の件数に依らない。

use super::*;
use crate::accounts::{add_account, ensure_accounts_initialized};
use kukuri_store::PeerCandidateStore as _;

const MODE: IdentityStorageMode = IdentityStorageMode::FileOnly;

async fn test_host(dir: &Path) -> Arc<ClientHost> {
    let db = ensure_accounts_initialized(dir, MODE).await.unwrap();
    let runtime =
        DesktopRuntime::new_with_config_and_identity(&db, TransportNetworkConfig::loopback(), MODE)
            .await
            .unwrap();
    ClientHost::from_runtime(dir.to_path_buf(), Arc::new(runtime))
        .await
        .unwrap()
}

async fn new_account_db(dir: &Path) -> std::path::PathBuf {
    let account = add_account(dir, MODE, &KukuriKeys::generate(), None, false)
        .await
        .unwrap();
    account_db_path(dir, &account.id)
}

/// `db` の account の runtime へ差し替え、前の runtime を止める（account の切替と同じ形）。
async fn replace_runtime(host: &ClientHost, db: &Path) {
    let next =
        DesktopRuntime::new_with_config_and_identity(db, TransportNetworkConfig::loopback(), MODE)
            .await
            .unwrap();
    host.replace_runtime(Arc::new(next))
        .await
        .unwrap()
        .shutdown()
        .await;
}

/// 試験が送った event を受け取る。runtime の起動時の account 同期の契機が送る通信状態の event（W5 AC-6a）は飛ばす。
async fn recv_emitted(events: &mut crate::ClientEventReceiver) -> RuntimeEvent {
    loop {
        match events.recv().await.unwrap() {
            RuntimeEvent::SyncStatusChanged { .. } => continue,
            event => return event,
        }
    }
}

/// host の broadcast まで届いた（別の購読で受け取れた）ことを確かめてから返す。
async fn emit_and_forward(host: &ClientHost, event: RuntimeEvent) {
    let mut probe = host.subscribe_events();
    host.runtime().emit_event(event.clone());
    assert_eq!(recv_emitted(&mut probe).await, event);
}

#[tokio::test]
async fn replacing_and_stopping_the_runtime_advance_the_generation_and_drop_stale_events() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let host = test_host(dir.path()).await;
    assert_eq!(host.generation(), 1);
    let mut events = host.subscribe_events();

    // 差し替えの前に host まで届き、まだ受け取っていない event は、差し替えの後に届かない。
    emit_and_forward(&host, RuntimeEvent::NotificationStatusChanged).await;
    replace_runtime(&host, &new_account_db(dir.path()).await).await;
    assert_eq!(host.generation(), 2);
    let current = RuntimeEvent::AdultMediaLabelEvicted {
        hash: Some("current".into()),
    };
    emit_and_forward(&host, current.clone()).await;
    assert_eq!(recv_emitted(&mut events).await, current);

    // 停止でも世代が進み、停止の前に届いた event は受け取れない。
    emit_and_forward(&host, RuntimeEvent::NotificationStatusChanged).await;
    host.shutdown().await;
    assert_eq!(host.generation(), 3);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_replaced_runtime_restores_only_the_desired_subscriptions_whatever_the_history() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let host = test_host(dir.path()).await;
    let desired = ["kukuri:topic:desired-a", "kukuri:topic:desired-b"];
    for topic in desired {
        host.add_desired_subscription(DesiredSubscription {
            topic: topic.into(),
            scope: DesiredSubscriptionScope::Public,
        })
        .await
        .unwrap();
    }
    // 履歴: 購読していない topic への投稿と、保存した peer candidate。
    let runtime = host.runtime();
    for index in 0..20 {
        runtime
            .create_post(CreatePostRequest {
                topic: format!("kukuri:topic:history-{index}"),
                content: "history".into(),
                reply_to: None,
                channel_ref: ChannelRef::Public,
                attachments: Vec::new(),
                content_labels: Vec::new(),
            })
            .await
            .unwrap();
    }
    for _ in 0..300 {
        let id = iroh::SecretKey::generate().public();
        runtime
            .sqlite
            .put_peer_candidate(
                "docs",
                "learned",
                &id.to_string(),
                &serde_json::to_vec(&iroh::EndpointAddr::new(id)).unwrap(),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
    }
    let history = runtime.db_path().to_path_buf();
    drop(runtime);

    // 別の account へ切り替えてから、履歴のある account へ戻す。
    replace_runtime(&host, &new_account_db(dir.path()).await).await;
    replace_runtime(&host, &history).await;
    let mut subscribed = host
        .runtime()
        .get_sync_status()
        .await
        .unwrap()
        .subscribed_topics;
    subscribed.sort();
    assert_eq!(subscribed, desired);
    host.shutdown().await;
}

// #1218 AC-2: 切替の後は、新しい account の同期の hint だけを購読し、前の account の同期の replica は開けない。
#[tokio::test]
async fn switching_accounts_moves_the_account_sync_scope() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let host = test_host(dir.path()).await;
    let hint = |runtime: &DesktopRuntime| {
        let sync = runtime.author_keys.derive_account_sync();
        kukuri_core::wire::hint_topic_id(sync.hint_topic())
            .as_str()
            .to_string()
    };
    let subscribed = |runtime: Arc<DesktopRuntime>| async move {
        kukuri_transport::Transport::subscribed_topics(runtime.iroh_stack.transport.as_ref())
            .await
            .unwrap()
    };
    let first = host.runtime();
    let first_hint = hint(&first);
    let first_replica = first.author_keys.derive_account_sync().replica_id().clone();
    assert!(subscribed(first).await.contains(&first_hint));

    replace_runtime(&host, &new_account_db(dir.path()).await).await;
    let second = host.runtime();
    let topics = subscribed(second.clone()).await;
    assert!(topics.contains(&hint(&second)));
    assert!(!topics.contains(&first_hint));
    assert!(
        kukuri_docs_sync::DocsSync::query_replica(
            second.iroh_stack.docs_sync.as_ref(),
            &first_replica,
            kukuri_docs_sync::DocQuery::All,
        )
        .await
        .is_err()
    );
    host.shutdown().await;
}
