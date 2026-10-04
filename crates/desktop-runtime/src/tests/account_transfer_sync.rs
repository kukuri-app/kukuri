//! #1211 AC-4: 移行の後、移行先の端末で、受け取ったアカウントの Community Node に同意して認証すると、rendezvous で
//! 移行元を見つけ、本人の端末間の自動同期（W5）へつながる。identity・同意・担当は端末ごとのまま移らない。
//! #1211 AC-6: フォロー・ブロックは、移行の必須 bundle と、その後の同期で届く。

use super::account_transfer::{runtime_at, target_host, transfer_with};
use super::*;
use crate::accounts::account_db_path;
use crate::community_node::{load_community_node_local_consents, load_community_node_token};
use crate::requests::{RotatePrivateChannelRequest, SetMyProfileRequest};
use kukuri_app_api::{PrivateChannelControllerState, SocialConnectionKind};
use kukuri_cn_protocol::{
    TopicRendezvousCandidate, TopicRendezvousHeartbeat, TopicRendezvousHeartbeatResponse,
    TopicRendezvousTopicResponse,
};
use kukuri_docs_sync::ReplicaNotice;
use std::collections::BTreeMap;

/// rendezvous の在席（topic の鍵ごとの端末と addr）。同じ鍵に居る他の端末を返す（cn-user-api の rendezvous と同じ）。
#[derive(Default)]
struct Presence(std::sync::Mutex<BTreeMap<String, BTreeMap<String, Option<String>>>>);

async fn presence_heartbeat(
    State(presence): State<Arc<Presence>>,
    Json(request): Json<TopicRendezvousHeartbeat>,
) -> Json<TopicRendezvousHeartbeatResponse> {
    let mut topics = presence.0.lock().unwrap();
    let topics = request
        .joins
        .iter()
        .chain(&request.refreshes)
        .map(|key| {
            let peers = topics.entry(key.clone()).or_default();
            peers.insert(request.endpoint_id.clone(), request.addr_hint.clone());
            TopicRendezvousTopicResponse {
                topic_key: key.clone(),
                peers: peers
                    .iter()
                    .filter(|(id, _)| **id != request.endpoint_id)
                    .map(|(id, addr_hint)| TopicRendezvousCandidate {
                        endpoint_id: id.clone(),
                        addr_hint: addr_hint.clone(),
                        relay_urls: Vec::new(),
                    })
                    .collect(),
            }
        })
        .collect();
    // 期限はクライアントのマージンより短くし、session の維持の度に rendezvous を更新させる。
    Json(TopicRendezvousHeartbeatResponse {
        expires_in_seconds: 5,
        topics,
    })
}

/// 端末ごとの Community Node（認証の token は端末ごと）。rendezvous の在席は共有する。
async fn community_node(presence: Arc<Presence>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        Vec::new(),
        false,
        Arc::new(Mutex::new(String::new())),
    ));
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route("/v1/policies", get(mock_managed_policies))
        .route("/v1/node/manifest", get(mock_managed_manifest))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(state)
        .merge(
            Router::new()
                .route("/v1/rendezvous/topics/heartbeat", post(presence_heartbeat))
                .with_state(presence),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    base_url
}

/// 同意のダイアログと同じ手順で、node の文書に同意して session を張る。
async fn consent(runtime: &DesktopRuntime, base_url: &str) {
    runtime
        .set_community_node_config(crate::SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: vec![crate::SetCommunityNodeConfigNode::new(base_url)],
        })
        .await
        .unwrap();
    let catalog = runtime
        .fetch_community_node_policies(crate::FetchCommunityNodePoliciesRequest {
            base_url: base_url.to_string(),
            language: Some("ja".into()),
        })
        .await
        .unwrap();
    let accepted = runtime
        .accept_community_node_consents(
            crate::AcceptCommunityNodeConsentsRequest {
                base_url: base_url.to_string(),
                documents: catalog
                    .policies
                    .iter()
                    .map(|policy| crate::CommunityNodeConsentDocumentRef {
                        policy_slug: policy.policy_slug.clone(),
                        policy_version: policy.policy_version,
                        policy_snapshot_revision: policy.policy_snapshot_revision.clone(),
                    })
                    .collect(),
                language: "ja".into(),
            },
            "test",
        )
        .await
        .unwrap();
    assert!(accepted.auth_state.authenticated);
}

/// 両端末の session の維持（rendezvous の更新）を回しながら、`done` を待つ。
async fn converge(what: &str, runtimes: [&DesktopRuntime; 2], mut done: impl AsyncFnMut() -> bool) {
    timeout(Duration::from_secs(60), async {
        while !done().await {
            for runtime in runtimes {
                runtime.run_community_node_session_maintenance_once().await;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

/// 端末のアプリの同意（`attested` なら年齢の申告も）。
fn device_consent(language: &str, attested: bool) -> crate::AppConsentStore {
    crate::AppConsentStore {
        records: crate::APP_LEGAL_DOCUMENTS
            .iter()
            .map(|(slug, version)| crate::AppConsentDocumentRecord {
                slug: slug.to_string(),
                version: *version,
                accepted_at: 1,
                language: language.into(),
                app_version: "test".into(),
                build_profile: None,
            })
            .collect(),
        age_attestations: attested
            .then(|| crate::AgeAttestationRecord {
                version: crate::AGE_ATTESTATION_VERSION,
                attested_at: 1,
                language: language.into(),
                app_version: "test".into(),
                build_profile: None,
            })
            .into_iter()
            .collect(),
    }
}

fn profile(name: &str) -> SetMyProfileRequest {
    SetMyProfileRequest {
        name: Some(name.into()),
        display_name: None,
        about: None,
        picture_upload: None,
        clear_picture: false,
    }
}

async fn profile_name(runtime: &DesktopRuntime) -> Option<String> {
    runtime.get_my_profile().await.unwrap().name
}

async fn channel_view(runtime: &DesktopRuntime, topic: &str) -> Option<JoinedPrivateChannelView> {
    runtime
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
            cursor: None,
        })
        .await
        .unwrap()
        .items
        .pop()
}

async fn social(runtime: &DesktopRuntime, kind: SocialConnectionKind) -> Vec<String> {
    runtime
        .app_service
        .list_social_connections(kind)
        .await
        .unwrap()
        .into_iter()
        .map(|view| view.author_pubkey)
        .collect()
}

async fn has_own_peer(runtime: &DesktopRuntime) -> bool {
    !runtime
        .get_sync_status()
        .await
        .unwrap()
        .account_sync
        .no_peers
}

/// 4a〜4f: 移行 → 切替 → 移行先の node への同意 → 相手の設定・鍵の更新の自動反映。identity・同意・担当は複製しない。
/// 移行先の account の replica には手元の書込みだけが入る（iroh-docs の replica の同期を始めない）。
#[tokio::test]
async fn a_transferred_account_syncs_with_the_source_after_consent() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let presence = Arc::new(Presence::default());
    let source_node = community_node(presence.clone()).await;
    let target_node = community_node(presence).await;

    let source = runtime_at(dir.path().join("source.db")).await;
    consent(&source, &source_node).await;
    source.set_my_profile(profile("before")).await.unwrap();
    let (followed, blocked) = (
        KukuriKeys::generate().public_key_hex(),
        KukuriKeys::generate().public_key_hex(),
    );
    source.app_service.follow_author(&followed).await.unwrap();
    source.app_service.block_author(&blocked).await.unwrap();
    source
        .set_adult_content_display_enabled(true)
        .await
        .unwrap();
    let topic = "kukuri:topic:account-transfer-sync";
    let channel = source
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "moved".into(),
            audience_kind: Default::default(),
        })
        .await
        .unwrap()
        .channel_id;
    let target_dir = dir.path().join("target");
    let host = target_host(&target_dir).await;
    // アプリの同意・年齢の申告は端末（app data）ごと。
    let consent_db = target_dir.join(crate::paths::DB_FILE_NAME);
    crate::save_app_consent_store(&consent_db, &device_consent("ja", false))
        .await
        .unwrap();
    crate::save_app_consent_store(
        &dir.path().join(crate::paths::DB_FILE_NAME),
        &device_consent("en", true),
    )
    .await
    .unwrap();

    let id = match transfer_with(&source, &host, None).await {
        kukuri_core::AccountTransferStatus::Completed {
            account_id: Some(id),
            ..
        } => id,
        status => panic!("unexpected {status:?}"),
    };
    // 切替: 受け取ったアカウントの runtime へ差し替える（起動で置き場を反映し、account 同期の lease を取る）。
    let db = account_db_path(&target_dir, &id);
    host.replace_runtime(Arc::new(runtime_at(&db).await))
        .await
        .unwrap()
        .shutdown()
        .await;
    let target = host.runtime();
    let replica = target
        .author_keys
        .derive_account_sync()
        .replica_id()
        .clone();
    let mut notices = target
        .iroh_stack
        .docs_sync
        .subscribe_replica_notices(&replica)
        .await
        .unwrap();
    let (local_entries, synced) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let watcher = tokio::spawn({
        let (local_entries, synced) = (local_entries.clone(), synced.clone());
        async move {
            while let Some(notice) = notices.next().await {
                match notice {
                    Ok(ReplicaNotice::Entry(event)) if event.source_peer.is_none() => {
                        local_entries.fetch_add(1, Ordering::SeqCst)
                    }
                    Ok(ReplicaNotice::Lagged { .. }) => 0,
                    _ => synced.fetch_add(1, Ordering::SeqCst),
                };
            }
        }
    });
    // 同意の前は、本人の端末の候補が無い。
    converge("the bundle is merged", [&source, &target], async || {
        profile_name(&target).await.as_deref() == Some("before")
            && channel_view(&target, topic)
                .await
                .is_some_and(|view| view.channel_id == channel)
            && social(&target, SocialConnectionKind::Following).await == [followed.clone()]
            && social(&target, SocialConnectionKind::Blocking).await == [blocked.clone()]
            && !has_own_peer(&target).await
    })
    .await;

    // 4c: 同じ docs author、別の endpoint（端末 ID）。
    let (source_status, target_status) = (
        source.get_sync_status().await.unwrap(),
        target.get_sync_status().await.unwrap(),
    );
    assert_eq!(
        source_status.local_author_pubkey,
        target_status.local_author_pubkey
    );
    assert_eq!(
        source
            .iroh_stack
            .docs_sync
            .local_docs_author()
            .await
            .unwrap(),
        target
            .iroh_stack
            .docs_sync
            .local_docs_author()
            .await
            .unwrap()
    );
    assert_ne!(
        source_status.discovery.local_endpoint_id,
        target_status.discovery.local_endpoint_id
    );
    // 4d: 移行元の node の token・同意、アプリの同意・年齢の申告、成人向けの表示の設定は移らない。
    assert!(
        load_community_node_token(&db, target.identity_mode, &source_node)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !load_community_node_local_consents(&db, target.identity_mode, &source_node)
            .await
            .unwrap()
            .has_active_consent()
    );
    assert_eq!(
        crate::load_app_consent_store(&consent_db).await,
        device_consent("ja", false)
    );
    assert!(!target.get_content_display_settings().adult_content_enabled);
    // 4e: 担当は移行（import）だけでは移らない。
    assert_eq!(
        channel_view(&source, topic).await.unwrap().controller,
        Some(PrivateChannelControllerState::ThisDevice)
    );
    assert_eq!(
        channel_view(&target, topic).await.unwrap().controller,
        Some(PrivateChannelControllerState::OtherDevice)
    );

    // 4a: 移行先の端末で同意・認証すると、rendezvous で移行元を見つけ、両方向の差分の取得が始まる。
    consent(&target, &target_node).await;
    converge(
        "the devices find each other",
        [&source, &target],
        async || has_own_peer(&target).await && has_own_peer(&source).await,
    )
    .await;
    // 4b: 以後は、相手の設定・鍵の更新が再 QR なしで届く。
    source
        .set_my_profile(profile("edited on source"))
        .await
        .unwrap();
    converge(
        "the source's edit reaches the target",
        [&source, &target],
        async || profile_name(&target).await.as_deref() == Some("edited on source"),
    )
    .await;
    let author = KukuriKeys::generate().public_key_hex();
    target
        .app_service
        .set_trust_always_visible(&author, true)
        .await
        .unwrap();
    converge(
        "the target's edit reaches the source",
        [&source, &target],
        async || {
            source
                .app_service
                .list_trust_always_visible()
                .await
                .unwrap()
                == vec![author.clone()]
        },
    )
    .await;
    // AC-6: 移行の後のフォロー・解除は、両方向に届く。
    let later = KukuriKeys::generate().public_key_hex();
    target.app_service.follow_author(&later).await.unwrap();
    source.app_service.unfollow_author(&followed).await.unwrap();
    converge(
        "the follows reach both devices",
        [&source, &target],
        async || {
            social(&source, SocialConnectionKind::Following).await == [later.clone()]
                && social(&target, SocialConnectionKind::Following).await == [later.clone()]
        },
    )
    .await;
    let rotated = source
        .rotate_private_channel(RotatePrivateChannelRequest {
            topic: topic.into(),
            channel_id: channel.clone(),
        })
        .await
        .unwrap()
        .current_epoch_id;
    converge(
        "the rotated key reaches the target",
        [&source, &target],
        async || {
            channel_view(&target, topic)
                .await
                .is_some_and(|view| view.current_epoch_id == rotated)
        },
    )
    .await;
    // 4f: 移行先の account の replica には、相手からの entry も同期の区切りも入らない。
    assert!(local_entries.load(Ordering::SeqCst) > 0);
    assert_eq!(synced.load(Ordering::SeqCst), 0);
    watcher.abort();

    source.shutdown().await;
    host.shutdown().await;
}
