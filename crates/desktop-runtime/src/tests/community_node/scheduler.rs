use super::super::*;

/// mock CN のカウンタが `at_least` に達するまで timeout 付きで待つ。
/// スケジューラ tick は非同期に走るため、hit 数は「以上」でのみ判定する。
async fn wait_for_counter(counter: &AtomicUsize, at_least: usize, label: &str) {
    timeout(Duration::from_secs(30), async {
        loop {
            if counter.load(Ordering::SeqCst) >= at_least {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{label} did not reach {at_least} within timeout (current: {})",
            counter.load(Ordering::SeqCst)
        )
    });
}

/// トレイ常駐相当(getter を一切呼ばない)でも bootstrap heartbeat が継続することを固定する。
/// WP-C1 の failing test: スケジューラ導入前は heartbeat_hits が 0 のまま timeout で赤になる。
#[tokio::test]
async fn session_scheduler_keeps_bootstrap_registration_alive_without_getter_polling() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-scheduler-keepalive.db");
    let runtime = Arc::new(
        DesktopRuntime::new_with_config_and_identity(
            &db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
        .await
        .expect("runtime"),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(vec![
            CommunityNodeSeedPeer::new(
                "1111111111111111111111111111111111111111111111111111111111111111",
                None,
            )
            .expect("seed peer"),
        ])),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
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
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    // トレイ常駐相当: get_sync_status / get_community_node_statuses は一切呼ばない。
    runtime
        .start_community_node_session_scheduler_with_interval(Duration::from_millis(200))
        .await;

    wait_for_counter(&state.heartbeat_hits, 1, "heartbeat_hits").await;

    // heartbeat 期限を過去に注入し、2 発目(維持の継続)を観測する。カウンタは mock が
    // リクエストを受けた時点で増えるが、クライアント側の次回期限書き込みはその後のため、
    // 単発注入では初回 tick の書き込みに上書きされうる。観測できるまで注入を繰り返す。
    if timeout(Duration::from_secs(30), async {
        loop {
            if let Some(entry) = runtime
                .community_node_sessions
                .lock()
                .await
                .get_mut(base_url.as_str())
            {
                entry.heartbeat_deadline = Utc::now().timestamp() - 1;
            }
            if state.heartbeat_hits.load(Ordering::SeqCst) >= 2 {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_err()
    {
        let task_finished = runtime
            .community_node_scheduler_task
            .lock()
            .await
            .as_ref()
            .map(|handle| handle.is_finished());
        let sessions = runtime.community_node_sessions.lock().await;
        panic!(
            "heartbeat did not continue after deadline expiry\n  hits={} sessions={:?} now={} task_finished={:?}",
            state.heartbeat_hits.load(Ordering::SeqCst),
            sessions,
            Utc::now().timestamp(),
            task_finished,
        );
    }

    // shutdown でスケジューラ task が停止し、以後 heartbeat が打たれないこと。
    runtime.shutdown().await;
    let hits_after_shutdown = state.heartbeat_hits.load(Ordering::SeqCst);
    if let Some(entry) = runtime
        .community_node_sessions
        .lock()
        .await
        .get_mut(base_url.as_str())
    {
        entry.heartbeat_deadline = Utc::now().timestamp() - 1;
    }
    sleep(Duration::from_millis(600)).await;
    assert_eq!(
        state.heartbeat_hits.load(Ordering::SeqCst),
        hits_after_shutdown
    );

    server.abort();
}

/// get_sync_status が CN セッションの establish/refresh を一切行わない(読み取り専用)ことを
/// 固定する contract。駆動はスケジューラ(session maintenance tick)のみが担う(WP-C1 T4)。
#[tokio::test]
async fn get_sync_status_is_read_only_for_community_node_session() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-sync-status-read-only.db");
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
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(Vec::new())),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
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
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    // heartbeat deadline 未設定(= 常に due)の状態でも、getter は refresh を駆動しない。
    let _status = runtime.get_sync_status().await.expect("sync status");

    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 0);
    assert!(runtime.community_node_sessions.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}

/// トレイ常駐相当(getter を一切呼ばない)でも失効間近トークンの事前再認証が走ることを固定する。
/// WP-C1 の failing test: スケジューラ導入前は verify_hits が 0 のまま timeout で赤になる。
#[tokio::test]
async fn session_scheduler_reauthenticates_near_expiry_token_without_getter_polling() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-scheduler-reauth.db");
    let runtime = Arc::new(
        DesktopRuntime::new_with_config_and_identity(
            &db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
        .await
        .expect("runtime"),
    );

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
        vec![seed_peer],
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
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    // トレイ常駐相当: getter を一切呼ばない。
    runtime
        .start_community_node_session_scheduler_with_interval(Duration::from_millis(200))
        .await;

    wait_for_counter(&state.verify_hits, 1, "verify_hits").await;
    wait_for_counter(&state.heartbeat_hits, 1, "heartbeat_hits").await;

    let stored = crate::community_node::load_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
    )
    .expect("load token")
    .expect("stored token");
    assert_ne!(stored.access_token, "near-expiry-token");

    runtime.shutdown().await;
    server.abort();
}

/// topic rendezvous presence の refresh が bootstrap heartbeat の合間にも発火することを固定する
/// characterization test(#572)。修正前は rendezvous refresh が heartbeat 便乗(実効約 60 秒毎)
/// のみで、サーバ TTL 45 秒に対し毎サイクル約 15 秒 presence が失効していた。
/// heartbeat が not-due のままでも maintenance pass 毎に rendezvous deadline が判定され、
/// due なら refresh POST が独立して打たれることを検証する。
#[tokio::test]
async fn topic_rendezvous_refresh_fires_between_bootstrap_heartbeats() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-rendezvous-refresh.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    // rendezvous refresh は subscribed_topics が空だと no-op のため、先に topic を購読する。
    let _ = runtime
        .list_timeline(ListTimelineRequest {
            topic: "kukuri:topic:rendezvous-refresh-gap".into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe topic");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockRendezvousCommunityNodeState {
        base_url: base_url.clone(),
        // bootstrap nodes が空 seed だと metadata retry(5 秒周期)の便乗 refresh が
        // 走ってテストを汚染するため、非空 seed を返す。
        seed_peers: vec![
            CommunityNodeSeedPeer::new(
                "2222222222222222222222222222222222222222222222222222222222222222",
                None,
            )
            .expect("seed peer"),
        ],
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_requests: Arc::new(Mutex::new(Vec::new())),
        account_candidate: None,
        rendezvous_failure: Arc::new(AtomicBool::new(false)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_rendezvous_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_rendezvous_bootstrap_nodes))
        .route(
            "/v1/rendezvous/topics/heartbeat",
            post(mock_rendezvous_topics_heartbeat),
        )
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
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
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    // 1 回目: セッション確立(heartbeat 1 発 + 便乗 rendezvous refresh)。
    // 2 回目: ready 遷移で立った ready_refresh_pending の metadata refresh(便乗 refresh)を消化。
    runtime.run_community_node_session_maintenance_once().await;
    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    let baseline = state.rendezvous_hits.load(Ordering::SeqCst);
    assert!(baseline >= 1, "establishment should refresh rendezvous");

    // heartbeat は not-due のまま(mock の expires_at = now+300 → 次回期限 +270 秒)。
    // mock の expires_in_seconds(5)はクライアントのマージンより小さいため、修正後は
    // rendezvous deadline が毎 pass 即時 due になり、pass 毎に独立 refresh が打たれる。
    for _ in 0..3 {
        runtime.run_community_node_session_maintenance_once().await;
    }
    assert_eq!(
        state.heartbeat_hits.load(Ordering::SeqCst),
        1,
        "bootstrap heartbeat must stay not-due during the observation window"
    );
    assert!(
        state.rendezvous_hits.load(Ordering::SeqCst) > baseline,
        "rendezvous presence must be refreshed between bootstrap heartbeats \
         (hits stayed at {baseline}; presence would expire against the 45s server TTL)"
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn account_rendezvous_queries_one_due_recipient_without_public_topic_snapshot() {
    use kukuri_core::{
        BlobHash, FollowEdgeStatus, build_follow_edge_envelope, direct_message_id_for_participants,
        parse_follow_edge,
    };
    use kukuri_store::{DirectMessageOutboxRow, DirectMessageStore, Store};
    use kukuri_transport::HintTransport;

    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("account-rendezvous.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .unwrap();
    let recipient = KukuriKeys::generate();
    let receiver = kukuri_iroh_node::IrohDocsNode::memory().await.unwrap();
    receiver
        .install_receive_binding(Arc::new(recipient.clone()))
        .await
        .unwrap();
    for (subject, target) in [
        (runtime.author_keys.as_ref(), recipient.public_key()),
        (&recipient, runtime.author_keys.public_key()),
    ] {
        let edge = parse_follow_edge(
            &build_follow_edge_envelope(subject, &target, FollowEdgeStatus::Active).unwrap(),
        )
        .unwrap()
        .unwrap();
        Store::upsert_follow_edge(runtime.store.as_ref(), edge)
            .await
            .unwrap();
    }
    DirectMessageStore::put_direct_message_outbox(
        runtime.store.as_ref(),
        DirectMessageOutboxRow {
            dm_id: direct_message_id_for_participants(
                &runtime.author_keys.public_key(),
                &recipient.public_key(),
            ),
            message_id: "rendezvous-due-dm".into(),
            peer_pubkey: recipient.public_key_hex(),
            frame_blob_hash: BlobHash::new("aa".repeat(32)),
            created_at: Utc::now().timestamp_millis(),
            last_attempt_at: None,
        },
    )
    .await
    .unwrap();
    let receiver_addr = receiver.endpoint().addr();
    let hint = receiver_addr.ip_addrs().next().unwrap().to_string();
    let recipient_key = kukuri_core::public_topic_rendezvous_key(
        &kukuri_core::receive_route_for_account(&recipient.public_key()).unwrap(),
    );
    let own_key = kukuri_core::public_topic_rendezvous_key(
        &kukuri_core::receive_route_for_account(&runtime.author_keys.public_key()).unwrap(),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(MockRendezvousCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: vec![CommunityNodeSeedPeer::new("22".repeat(32), None).unwrap()],
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_requests: Arc::new(Mutex::new(Vec::new())),
        account_candidate: Some((
            recipient_key.clone(),
            kukuri_cn_protocol::TopicRendezvousCandidate {
                endpoint_id: receiver.endpoint().id().to_string(),
                addr_hint: Some(hint),
                relay_urls: Vec::new(),
            },
        )),
        rendezvous_failure: Arc::new(AtomicBool::new(false)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_rendezvous_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_rendezvous_bootstrap_nodes))
        .route(
            "/v1/rendezvous/topics/heartbeat",
            post(mock_rendezvous_topics_heartbeat),
        )
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let listener_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url_b = format!("http://{}", listener_b.local_addr().unwrap());
    let state_b = Arc::new(MockRendezvousCommunityNodeState {
        base_url: base_url_b.clone(),
        seed_peers: vec![CommunityNodeSeedPeer::new("33".repeat(32), None).unwrap()],
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_requests: Arc::new(Mutex::new(Vec::new())),
        account_candidate: None,
        rendezvous_failure: Arc::new(AtomicBool::new(false)),
    });
    let app_b = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_rendezvous_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_rendezvous_bootstrap_nodes))
        .route(
            "/v1/rendezvous/topics/heartbeat",
            post(mock_rendezvous_topics_heartbeat),
        )
        .with_state(state_b.clone());
    let server_b = tokio::spawn(async move {
        let _ = axum::serve(listener_b, app_b).await;
    });
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".into(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .unwrap();
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url_b.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".into(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .unwrap();
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: [base_url.clone(), base_url_b.clone()]
            .into_iter()
            .map(|url| CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: url.clone(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(url, Vec::new(), Vec::new()).unwrap(),
                ),
            })
            .collect(),
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);
    seed_local_community_node_consents(&runtime, base_url_b.as_str(), 1);
    for _ in 0..4 {
        runtime.run_community_node_session_maintenance_once().await;
        if state
            .rendezvous_requests
            .lock()
            .await
            .iter()
            .any(|request| {
                request.refreshes.contains(&own_key)
                    && request.refreshes.contains(&recipient_key)
                    && request.refreshes.len() <= 5
            })
            && state_b
                .rendezvous_requests
                .lock()
                .await
                .iter()
                .any(|request| {
                    request.refreshes.contains(&own_key)
                        && request.refreshes.contains(&recipient_key)
                        && request.refreshes.len() <= 5
                })
        {
            break;
        }
        if let Some(session) = runtime
            .community_node_sessions
            .lock()
            .await
            .get_mut(&base_url)
        {
            session.rendezvous_refresh_deadline = 0;
        }
        if let Some(session) = runtime
            .community_node_sessions
            .lock()
            .await
            .get_mut(&base_url_b)
        {
            session.rendezvous_refresh_deadline = 0;
        }
    }
    let requests = state.rendezvous_requests.lock().await;
    assert!(
        requests.iter().any(|request| {
            request.refreshes.contains(&own_key)
                && request.refreshes.contains(&recipient_key)
                && request.refreshes.len() <= 5
        }),
        "requests={requests:?}"
    );
    drop(requests);
    let requests_b = state_b.rendezvous_requests.lock().await;
    assert!(
        requests_b.iter().any(|request| {
            request.refreshes.contains(&own_key)
                && request.refreshes.contains(&recipient_key)
                && request.refreshes.len() <= 5
        }),
        "second CN must get its own recipient page: {requests_b:?}"
    );
    drop(requests_b);
    let resolved = runtime
        .iroh_stack
        .transport
        .resolve_receive_destination(&recipient.public_key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.id, receiver.endpoint().id());
    runtime
        .deactivate_community_node_connectivity(&base_url_b)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .iroh_stack
            .transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .unwrap()
            .id,
        receiver.endpoint().id(),
        "second CN deactivation must not remove the first CN's bound recipient"
    );
    {
        let mut sessions = runtime.community_node_sessions.lock().await;
        let session = sessions.get_mut(&base_url).unwrap();
        session.heartbeat_deadline = Utc::now().timestamp() + 60;
        session.metadata_refresh_deadline = Utc::now().timestamp() + 60;
        session.ready_refresh_pending = false;
        session.rendezvous_refresh_deadline = 0;
    }
    state.rendezvous_failure.store(true, Ordering::SeqCst);
    let hits_before_failure = state.rendezvous_hits.load(Ordering::SeqCst);
    runtime.run_community_node_session_maintenance_once().await;
    let hits_after_failure = state.rendezvous_hits.load(Ordering::SeqCst);
    assert!(hits_after_failure > hits_before_failure);
    assert!(
        runtime
            .community_node_sessions
            .lock()
            .await
            .get(&base_url)
            .unwrap()
            .rendezvous_refresh_deadline
            >= Utc::now().timestamp() + 4,
        "failed account rendezvous must back off before another HTTP request"
    );
    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(
        state.rendezvous_hits.load(Ordering::SeqCst),
        hits_after_failure
    );
    runtime.clear_community_node_config().await.unwrap();
    assert!(
        runtime
            .iroh_stack
            .transport
            .resolve_receive_destination(&recipient.public_key())
            .await
            .unwrap()
            .is_none(),
        "removed CN candidates must not survive config clear"
    );
    runtime.shutdown().await;
    receiver.shutdown().await.unwrap();
    server.abort();
    server_b.abort();
}

#[tokio::test]
async fn private_channel_rendezvous_refresh_uses_only_the_current_epoch_secret() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("一時ディレクトリを作成できる");
    let db_path = dir.path().join("community-private-rendezvous.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("実行環境を作成できる");
    let topic = "kukuri:topic:private-rendezvous";
    runtime
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("公開話題を購読できる");
    let channel = runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "非公開ランデブー".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("非公開チャンネルを作成できる");
    let invite = runtime
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("現在の招待を出力できる");
    let preview = kukuri_core::parse_private_channel_invite_token(invite.as_str())
        .expect("現在の招待を解析できる");
    let private_topic = kukuri_core::wire::hint_topic_id(
        &kukuri_docs_sync::private_channel_hint_topic(channel.channel_id.as_str()),
    );
    let expected_private = kukuri_core::private_topic_rendezvous_key_hex_secret(
        preview.namespace_secret_hex.as_str(),
        &private_topic,
    )
    .expect("現在の非公開ランデブー鍵を派生できる");
    let forbidden_public_private = kukuri_core::public_topic_rendezvous_key(&private_topic);
    let public_hint_topic = kukuri_core::wire::hint_topic_id(&kukuri_core::TopicId::new(topic));
    let expected_public = kukuri_core::public_topic_rendezvous_key(&public_hint_topic);
    let missing_private_base = kukuri_docs_sync::private_channel_hint_topic("missing-secret");
    let missing_private_topic = kukuri_core::wire::hint_topic_id(&missing_private_base);
    let forbidden_missing_private =
        kukuri_core::public_topic_rendezvous_key(&missing_private_topic);
    let _missing_private_stream = kukuri_transport::HintTransport::subscribe_hints(
        runtime.iroh_stack.transport.as_ref(),
        &missing_private_base,
    )
    .await
    .expect("現在の秘密を持たない非公開話題を購読できる");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("待受先を確保できる");
    let base_url = format!(
        "http://{}",
        listener.local_addr().expect("待受先を取得できる")
    );
    let state = Arc::new(MockRendezvousCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: vec![
            CommunityNodeSeedPeer::new("2".repeat(64), None).expect("初期接続先を作成できる"),
        ],
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_hits: Arc::new(AtomicUsize::new(0)),
        rendezvous_requests: Arc::new(Mutex::new(Vec::new())),
        account_candidate: None,
        rendezvous_failure: Arc::new(AtomicBool::new(false)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_rendezvous_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_rendezvous_bootstrap_nodes))
        .route(
            "/v1/rendezvous/topics/heartbeat",
            post(mock_rendezvous_topics_heartbeat),
        )
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("コミュニティノードの認証情報を保存できる");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("接続先を解決できる"),
            ),
        }],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    runtime.run_community_node_session_maintenance_once().await;
    runtime.run_community_node_session_maintenance_once().await;
    let requests = state.rendezvous_requests.lock().await.clone();
    assert!(!requests.is_empty(), "ランデブー更新が送信される");
    assert!(
        requests
            .iter()
            .any(|request| request.refreshes.contains(&expected_private)),
        "現在の非公開ランデブー鍵が更新される"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.refreshes.contains(&expected_public)),
        "公開話題のランデブー鍵は変わらない"
    );
    assert!(requests.iter().all(|request| {
        !request.refreshes.contains(&forbidden_public_private)
            && !request.refreshes.contains(&forbidden_missing_private)
            && !request
                .refreshes
                .iter()
                .any(|key| key == private_topic.as_str())
            && !request
                .refreshes
                .iter()
                .any(|key| key == missing_private_topic.as_str())
    }));

    state.rendezvous_requests.lock().await.clear();
    let old_private = expected_private;
    runtime
        .rotate_private_channel(RotatePrivateChannelRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
        })
        .await
        .expect("非公開チャンネルの世代を切り替えられる");
    assert_eq!(
        runtime
            .community_node_sessions
            .lock()
            .await
            .get(base_url.as_str())
            .expect("コミュニティノードの接続状態がある")
            .rendezvous_refresh_deadline,
        0,
        "世代切替後はランデブー更新を直ちに実行する"
    );
    let fresh_invite = runtime
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("新しい招待を出力できる");
    let fresh_preview = kukuri_core::parse_private_channel_invite_token(fresh_invite.as_str())
        .expect("新しい招待を解析できる");
    let current_private = kukuri_core::private_topic_rendezvous_key_hex_secret(
        fresh_preview.namespace_secret_hex.as_str(),
        &private_topic,
    )
    .expect("切替後の非公開ランデブー鍵を派生できる");
    assert_ne!(old_private, current_private);

    runtime.run_community_node_session_maintenance_once().await;
    let rotated_requests = state.rendezvous_requests.lock().await.clone();
    assert!(
        rotated_requests
            .iter()
            .any(|request| request.refreshes.contains(&current_private)),
        "切替後の非公開ランデブー鍵が更新される"
    );
    assert!(
        rotated_requests
            .iter()
            .all(|request| !request.refreshes.contains(&old_private))
    );

    runtime.shutdown().await;
    server.abort();
}
