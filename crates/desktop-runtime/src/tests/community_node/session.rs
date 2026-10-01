use super::super::*;

#[tokio::test]
async fn consented_node_bootstraps_session_on_maintenance_tick() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-consented-session.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let seed_peer = CommunityNodeSeedPeer::new(
        "2222222222222222222222222222222222222222222222222222222222222222",
        Some("127.0.0.1:44001".into()),
    )
    .expect("seed peer");
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        vec![seed_peer.clone()],
        false,
        Arc::new(Mutex::new(String::new())),
    ));
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route("/v1/policies", get(mock_managed_policies))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };
    // #857: ユーザーが同意モーダルで受諾済み(ローカル同意記録あり)の node だけが
    // スケジューラ tick でセッションを確立する。
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;

    // WP-Q2: セッション確立はスケジューラ tick が駆動し、getter は読み取り専用。
    runtime.run_community_node_session_maintenance_once().await;
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    // #857: 認証(JWT 発行)前に公開カタログで現行版への同意を確認している。
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.consent_status_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.consent_accept_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    assert_eq!(statuses.len(), 1);
    assert!(statuses[0].auth_state.authenticated);
    assert_eq!(
        statuses[0].session_phase,
        crate::CommunityNodeSessionPhase::Ready
    );
    assert_eq!(statuses[0].retry_after, None);
    assert!(
        statuses[0]
            .consent_state
            .as_ref()
            .expect("consent state")
            .all_required_accepted
    );
    assert_eq!(
        statuses[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![seed_peer]
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn status_getter_is_read_only_and_does_not_bootstrap_session() {
    // WP-Q2: get_community_node_statuses は読み取り専用。セッションの establish/refresh は
    // セッション維持スケジューラ(run_community_node_session_maintenance_once)が担い、
    // getter 単独では refresh 副作用(challenge/verify/heartbeat/bootstrap)を持たない。
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-getter-read-only.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        vec![],
        false,
        Arc::new(Mutex::new(String::new())),
    ));
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route("/v1/policies", get(mock_managed_policies))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;

    // getter 単独では同意済みノードでもセッションを bootstrap しない。
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 0);
    assert_eq!(statuses.len(), 1);
    assert_eq!(
        statuses[0].session_phase,
        crate::CommunityNodeSessionPhase::Idle
    );

    // スケジューラ tick を回すとセッションが確立する。
    runtime.run_community_node_session_maintenance_once().await;
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses after maintenance tick");
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        statuses[0].session_phase,
        crate::CommunityNodeSessionPhase::Ready
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn near_expiry_token_triggers_proactive_community_node_reauthentication() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-proactive-reauth.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let seed_peer = CommunityNodeSeedPeer::new(
        "3333333333333333333333333333333333333333333333333333333333333333",
        None,
    )
    .expect("seed peer");
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        vec![seed_peer.clone()],
        true,
        Arc::new(Mutex::new("near-expiry-token".into())),
    ));
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route("/v1/policies", get(mock_managed_policies))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "near-expiry-token".into(),
            expires_at: Utc::now().timestamp() + 60,
        },
    )
    .await
    .expect("persist near-expiry token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;

    // WP-Q2: 近接失効トークンの proactive 再認証はスケジューラ tick が駆動する。
    runtime.run_community_node_session_maintenance_once().await;
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.consent_status_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.consent_accept_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        statuses[0].session_phase,
        crate::CommunityNodeSessionPhase::Ready
    );
    assert!(statuses[0].auth_state.authenticated);

    let stored = crate::community_node::load_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
    )
    .await
    .expect("load token")
    .expect("stored token");
    assert_ne!(stored.access_token, "near-expiry-token");

    runtime.shutdown().await;
    server.abort();
}

// #857 受入条件: Node 同意前のネットワーク通信が許可リストどおりであること。
// ローカル同意記録の無い node へは、スケジューラ tick を回しても一切の HTTP
// (認証 challenge / verify、consent status、policies、heartbeat、bootstrap)が出ない。
#[tokio::test]
async fn node_without_local_consent_is_never_contacted() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-consent-gate.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        vec![],
        false,
        Arc::new(Mutex::new("legacy-token".into())),
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
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // 旧クライアント由来の有効トークンが残っていても、ローカル同意記録が無ければ
    // 通信しない(#857 の責任分界: 同意が SSoT)。
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "legacy-token".into(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };

    runtime.run_community_node_session_maintenance_once().await;
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.consent_status_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.consent_accept_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        statuses[0].session_phase,
        crate::CommunityNodeSessionPhase::Idle
    );
    assert!(!statuses[0].local_consent.has_active_consent());

    // 同意判断のための公開カタログ取得(UI 操作起点)は許可リストに含まれる。
    let catalog = runtime
        .fetch_community_node_policies(crate::FetchCommunityNodePoliciesRequest {
            base_url: base_url.clone(),
            language: Some("ja".to_string()),
        })
        .await
        .expect("fetch policies");
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(catalog.policies.len(), 1);

    // 公開manifestも同意判断に必要な明示取得として許可する。
    let manifest = runtime
        .fetch_community_node_manifest(crate::CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("fetch manifest");
    assert_eq!(
        manifest.status,
        crate::CommunityNodeManifestFetchStatus::Absent
    );
    assert_eq!(state.manifest_hits.load(Ordering::SeqCst), 1);

    // 同意モーダルでの受諾 → ローカル記録 → セッション確立(認証・サーバ同期)。
    let accepted = runtime
        .accept_community_node_consents(
            crate::AcceptCommunityNodeConsentsRequest {
                base_url: base_url.clone(),
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
            "0.1.8-test",
        )
        .await
        .expect("accept consents");
    assert!(accepted.local_consent.has_active_consent());
    let record = &accepted.local_consent.records[0];
    assert_eq!(record.policy_slug, MOCK_MANAGED_POLICY_SLUG);
    assert_eq!(record.policy_version, 1);
    assert_eq!(record.language, "ja");
    assert_eq!(record.app_version, "0.1.8-test");
    assert!(record.accepted_at > 0);
    assert_eq!(state.consent_accept_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        accepted.session_phase,
        crate::CommunityNodeSessionPhase::Ready
    );

    // 撤回するとトークンが破棄され、以後の tick で通信が再発しない。
    let withdrawn = runtime
        .withdraw_community_node_consents(crate::CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("withdraw consents");
    assert!(!withdrawn.local_consent.has_active_consent());
    assert!(withdrawn.local_consent.withdrawn_at.is_some());
    // 過去の同意記録は履歴として残る。
    assert!(!withdrawn.local_consent.records.is_empty());
    assert!(!withdrawn.auth_state.authenticated);
    let hits_after_withdraw = (
        state.policies_hits.load(Ordering::SeqCst),
        state.challenge_hits.load(Ordering::SeqCst),
        state.heartbeat_hits.load(Ordering::SeqCst),
    );
    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(
        (
            state.policies_hits.load(Ordering::SeqCst),
            state.challenge_hits.load(Ordering::SeqCst),
            state.heartbeat_hits.load(Ordering::SeqCst),
        ),
        hits_after_withdraw
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_status_does_not_require_restart_when_verified_connectivity_is_active() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-status.db");
    let test_timeout = Duration::from_secs(15);
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let base_url = "https://community.example.com".to_string();
    let connectivity_url = "http://127.0.0.1:9".to_string();
    let resolved_urls = CommunityNodeResolvedUrls::new(
        base_url.clone(),
        vec![connectivity_url.clone()],
        Vec::new(),
    )
    .expect("resolved urls");
    let node = CommunityNodeNodeConfig {
        content_advisory_enabled: true,
        base_url: base_url.clone(),
        resolved_urls: Some(resolved_urls.clone()),
    };
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist community-node token");
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![node.clone()],
    };
    mark_community_node_session_ready_for_test(&runtime, base_url.as_str()).await;
    runtime.community_node_connectivity.lock().await.relay_urls = vec![connectivity_url.clone()];

    let status = timeout(
        test_timeout,
        runtime.community_node_status(
            node,
            Some(CommunityNodeConsentStatus {
                all_required_accepted: true,
                items: vec![kukuri_cn_protocol::CommunityNodeConsentItem {
                    policy_slug: "community-basic".to_string(),
                    policy_version: 1,
                    title: "Community Basic".to_string(),
                    body: "Community basic policy body.".to_string(),
                    required: true,
                    effective_date: Some("2026-09-02".to_string()),
                    language: Some("ja".to_string()),
                    policy_snapshot_revision: None,
                    accepted_at: Some(Utc::now().timestamp()),
                    previously_accepted_version: Some(1),
                }],
                policy_snapshot_revision: None,
            }),
            None,
        ),
    )
    .await
    .expect("community-node status timeout")
    .expect("community-node status");
    assert!(status.auth_state.authenticated);
    assert!(
        status
            .consent_state
            .as_ref()
            .expect("consent state")
            .all_required_accepted
    );
    assert_eq!(
        status
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .connectivity_urls,
        vec![connectivity_url]
    );
    assert_eq!(
        status.session_phase,
        crate::CommunityNodeSessionPhase::Ready
    );
    assert!(!status.restart_required);

    timeout(test_timeout, runtime.shutdown())
        .await
        .expect("runtime shutdown timeout");
}

#[tokio::test]
async fn policy_update_is_not_silently_reaccepted() {
    // #384 / #857: 版が上がった「更新」のときは、ローカル同意が旧版のままなら
    // 黙って再受諾せず Idle に留め、ユーザーの再同意後にのみセッションを進める。
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-consent-update.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        vec![],
        false,
        Arc::new(Mutex::new("update-pending-token".into())),
    ));
    state.simulate_pending_update.store(true, Ordering::SeqCst);
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route("/v1/policies", get(mock_managed_policies))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "update-pending-token".into(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };
    // ユーザーは旧版(1)に同意済み。サーバの現行版は 2(simulate_pending_update)。
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;

    // WP-Q2: consent 判定もスケジューラ tick が駆動する。
    runtime.run_community_node_session_maintenance_once().await;
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    // 更新時は auto 受諾しない。
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.consent_status_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.consent_accept_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        statuses[0].session_phase,
        crate::CommunityNodeSessionPhase::Idle
    );
    // 再同意が必要なことが status で client に見えている(#857)。
    assert!(statuses[0].consent_update_pending);
    assert!(statuses[0].consent_state.is_none());

    // ユーザーが新版(2)へ明示的に再同意すれば ready になる。
    let accepted = runtime
        .accept_community_node_consents(
            crate::AcceptCommunityNodeConsentsRequest {
                base_url: base_url.clone(),
                documents: vec![crate::CommunityNodeConsentDocumentRef {
                    policy_slug: MOCK_MANAGED_POLICY_SLUG.to_string(),
                    policy_version: 2,
                    policy_snapshot_revision: None,
                }],
                language: "ja".into(),
            },
            "0.1.8-test",
        )
        .await
        .expect("accept consents");
    assert!(
        accepted
            .consent_state
            .as_ref()
            .expect("consent state")
            .all_required_accepted
    );
    assert!(!accepted.consent_update_pending);
    assert_eq!(state.consent_accept_hits.load(Ordering::SeqCst), 1);
    // 旧版と新版の記録が両方履歴に残る。
    assert_eq!(accepted.local_consent.records.len(), 2);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn saved_token_does_not_bypass_same_version_snapshot_preflight() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-consent-snapshot-update.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        vec![],
        true,
        Arc::new(Mutex::new("saved-token".into())),
    ));
    state.simulate_snapshot_update.store(true, Ordering::SeqCst);
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route("/v1/policies", get(mock_managed_policies))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "saved-token".into(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };
    seed_local_community_node_consents_with_snapshot(
        &runtime,
        base_url.as_str(),
        1,
        Some("snapshot-1"),
    )
    .await;

    runtime.run_community_node_session_maintenance_once().await;
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");

    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.consent_status_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        statuses[0].session_phase,
        crate::CommunityNodeSessionPhase::Idle
    );
    assert!(statuses[0].consent_update_pending);

    runtime.shutdown().await;
    server.abort();
}
