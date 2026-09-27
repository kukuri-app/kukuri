use super::super::*;

#[tokio::test]
async fn persisted_community_node_connectivity_is_not_applied_without_local_consent() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir
        .path()
        .join("community-node-unconsented-connectivity.db");
    let relay_url = "https://127.0.0.1:9";
    let seed_peer = CommunityNodeSeedPeer::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        Some("127.0.0.1:9".to_string()),
    )
    .expect("seed peer");
    let persisted = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: "https://community.example.com".to_string(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(
                    "https://community.example.com",
                    vec![relay_url.to_string()],
                    vec![seed_peer],
                )
                .expect("resolved urls"),
            ),
        }],
    };
    save_community_node_config(&db_path, &persisted).expect("save community-node config");

    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    assert!(
        runtime
            .community_node_connectivity
            .lock()
            .await
            .relay_urls
            .is_empty()
    );
    assert!(
        runtime
            .get_sync_status()
            .await
            .expect("sync status")
            .discovery
            .bootstrap_seed_peer_count
            == 0
    );
    let stored = runtime
        .get_community_node_config()
        .await
        .expect("stored community-node config");
    assert_eq!(stored, persisted);

    runtime.shutdown().await;
}

#[tokio::test]
async fn startup_does_not_apply_persisted_community_node_connectivity_before_preflight() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-node-preflight-connectivity.db");
    let base_url = "https://community.example.com";
    let relay_url = "https://127.0.0.1:9";
    let community_seed = CommunityNodeSeedPeer::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        Some("127.0.0.1:9".to_string()),
    )
    .expect("community seed");
    let configured_seed = SeedPeer {
        endpoint_id: "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        addr_hint: Some("127.0.0.1:11001".to_string()),
    };
    save_community_node_config(
        &db_path,
        &CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: base_url.to_string(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(
                        base_url,
                        vec![relay_url.to_string()],
                        vec![community_seed],
                    )
                    .expect("resolved urls"),
                ),
            }],
        },
    )
    .expect("save community-node config");
    let mut local_consent = crate::CommunityNodeLocalConsentState::default();
    crate::community_node::record_community_node_local_consents(
        &mut local_consent,
        &[crate::CommunityNodeConsentDocumentRef {
            policy_slug: MOCK_MANAGED_POLICY_SLUG.to_string(),
            policy_version: 1,
            policy_snapshot_revision: None,
        }],
        "ja",
        "test-app",
        Utc::now().timestamp(),
    );
    crate::community_node::persist_community_node_local_consents(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url,
        &local_consent,
    )
    .expect("persist local consent");
    let mut discovery_config = DiscoveryConfig::static_peer_default();
    discovery_config.seed_peers = vec![configured_seed.clone()];

    let runtime = DesktopRuntime::new_with_config_and_identity_and_discovery(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
        discovery_config,
        DhtDiscoveryOptions::disabled(),
        None,
    )
    .await
    .expect("runtime");

    let discovery = runtime.get_sync_status().await.expect("status").discovery;
    assert_eq!(
        runtime
            .connectivity_peer_ids(kukuri_transport::ConnectivityPeerKind::ConfiguredSeed)
            .await,
        vec![configured_seed.endpoint_id]
    );
    assert_eq!(discovery.bootstrap_seed_peer_count, 0);
    assert_eq!(discovery.connect_mode, ConnectMode::DirectOnly);

    runtime.shutdown().await;
}

#[tokio::test]
async fn connectivity_apply_ignores_local_consent_without_verified_ready_session() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-node-unverified-connectivity.db");
    let base_url = "https://community.example.com";
    let relay_url = "https://127.0.0.1:9";
    let community_seed = CommunityNodeSeedPeer::new(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        Some("127.0.0.1:9".to_string()),
    )
    .expect("community seed");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    seed_local_community_node_consents(&runtime, base_url, 1);
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.to_string(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(
                    base_url,
                    vec![relay_url.to_string()],
                    vec![community_seed],
                )
                .expect("resolved urls"),
            ),
        }],
    };

    runtime
        .sync_community_node_connectivity(base_url)
        .await
        .expect("sync connectivity");

    let applied = runtime.community_node_connectivity.lock().await;
    assert!(applied.relay_urls.is_empty());
    assert!(applied.seed_peers.is_empty());
    drop(applied);

    runtime.shutdown().await;
}

#[tokio::test]
async fn connectivity_apply_keeps_only_the_node_with_verified_ready_session() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-node-mixed-verification.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let verified_base_url = "https://verified.example.com";
    let unverified_base_url = "https://unverified.example.com";
    seed_local_community_node_consents(&runtime, verified_base_url, 1);
    seed_local_community_node_consents(&runtime, unverified_base_url, 1);
    mark_community_node_session_ready_for_test(&runtime, verified_base_url).await;
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![
            CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: verified_base_url.to_string(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(
                        verified_base_url,
                        vec!["https://relay-verified.example.com".to_string()],
                        Vec::new(),
                    )
                    .expect("verified urls"),
                ),
            },
            CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: unverified_base_url.to_string(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(
                        unverified_base_url,
                        vec!["https://relay-unverified.example.com".to_string()],
                        Vec::new(),
                    )
                    .expect("unverified urls"),
                ),
            },
        ],
    };

    for base_url in [verified_base_url, unverified_base_url] {
        runtime
            .sync_community_node_connectivity(base_url)
            .await
            .expect("sync connectivity");
    }

    assert_eq!(
        runtime.community_node_connectivity.lock().await.relay_urls,
        vec!["https://relay-verified.example.com".to_string()]
    );

    runtime.shutdown().await;
}

#[tokio::test]
async fn withdrawing_community_node_consent_removes_transport_assist() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-node-withdraw-connectivity.db");
    let base_url = "https://community.example.com";
    let relay_url = "https://127.0.0.1:9";
    let configured_seed = SeedPeer {
        endpoint_id: "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        addr_hint: Some("127.0.0.1:11001".to_string()),
    };
    let bootstrap_seed = CommunityNodeSeedPeer::new(
        "2222222222222222222222222222222222222222222222222222222222222222",
        Some("127.0.0.1:22002".to_string()),
    )
    .expect("bootstrap seed");
    let mut discovery_config = DiscoveryConfig::static_peer_default();
    discovery_config.seed_peers = vec![configured_seed.clone()];
    let runtime = DesktopRuntime::new_with_config_and_identity_and_discovery(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
        discovery_config,
        DhtDiscoveryOptions::disabled(),
        None,
    )
    .await
    .expect("runtime");
    seed_local_community_node_consents(&runtime, base_url, 1);
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.to_string(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(
                    base_url,
                    vec![relay_url.to_string()],
                    vec![bootstrap_seed],
                )
                .expect("resolved urls"),
            ),
        }],
    };
    mark_community_node_session_ready_for_test(&runtime, base_url).await;
    runtime
        .sync_community_node_connectivity(base_url)
        .await
        .expect("sync connectivity");

    let before = runtime.get_sync_status().await.expect("status").discovery;
    assert_eq!(before.connect_mode, ConnectMode::DirectOrRelay);
    assert_eq!(
        runtime
            .connectivity_peer_ids(kukuri_transport::ConnectivityPeerKind::ConfiguredSeed)
            .await,
        vec![configured_seed.endpoint_id.clone()]
    );
    assert_eq!(before.bootstrap_seed_peer_count, 1);

    runtime
        .withdraw_community_node_consents(crate::CommunityNodeTargetRequest {
            base_url: base_url.to_string(),
        })
        .await
        .expect("withdraw consents");

    let after = runtime.get_sync_status().await.expect("status").discovery;
    assert_eq!(after.connect_mode, ConnectMode::DirectOnly);
    assert_eq!(
        runtime
            .connectivity_peer_ids(kukuri_transport::ConnectivityPeerKind::ConfiguredSeed)
            .await,
        vec![configured_seed.endpoint_id]
    );
    assert_eq!(after.bootstrap_seed_peer_count, 0);

    runtime.shutdown().await;
}

#[tokio::test]
async fn community_node_connectivity_filter_is_scoped_per_node() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-node-consent-filter.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let consented_base_url = "https://consented.example.com";
    let pending_base_url = "https://pending.example.com";
    seed_local_community_node_consents(&runtime, consented_base_url, 1);
    let config = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![
            CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: consented_base_url.to_string(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(
                        consented_base_url,
                        vec!["https://relay-consented.example.com".to_string()],
                        Vec::new(),
                    )
                    .expect("consented resolved urls"),
                ),
            },
            CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: pending_base_url.to_string(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(
                        pending_base_url,
                        vec!["https://relay-pending.example.com".to_string()],
                        Vec::new(),
                    )
                    .expect("pending resolved urls"),
                ),
            },
        ],
    };

    *runtime.community_node_config.lock().await = config;
    // 同意の無い node は、session の状態がどうであれ relay と seed を足さない。
    for base_url in [consented_base_url, pending_base_url] {
        mark_community_node_session_ready_for_test(&runtime, base_url).await;
        runtime
            .sync_community_node_connectivity(base_url)
            .await
            .expect("sync connectivity");
    }

    assert_eq!(
        runtime.community_node_connectivity.lock().await.relay_urls,
        vec!["https://relay-consented.example.com".to_string()]
    );

    runtime.shutdown().await;
}

#[test]
fn community_node_config_normalizes_base_urls_and_connectivity_urls() {
    let config = normalize_community_node_config(CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![
            CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://community.example.com/".into(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(
                        "https://public.example.com/",
                        vec![
                            "https://relay-b.example.com/".into(),
                            "https://relay-a.example.com/".into(),
                            "https://relay-a.example.com/".into(),
                        ],
                        vec![CommunityNodeSeedPeer::new("peer-b", None).expect("seed peer")],
                    )
                    .expect("resolved urls"),
                ),
            },
            CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://community.example.com".into(),
                resolved_urls: None,
            },
        ],
    })
    .expect("normalized config");

    assert_eq!(config.nodes.len(), 1);
    assert_eq!(config.nodes[0].base_url, "https://community.example.com");
    assert_eq!(
        config.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .connectivity_urls,
        vec![
            "https://relay-a.example.com".to_string(),
            "https://relay-b.example.com".to_string(),
        ]
    );
    assert_eq!(
        config.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![CommunityNodeSeedPeer::new("peer-b", None).expect("seed peer")]
    );
}

#[test]
fn community_node_config_preserves_public_kukuri_urls() {
    let config = normalize_community_node_config(CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: "https://api.kukuri.app/".into(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(
                    "https://api.kukuri.app/",
                    vec!["https://iroh-relay.kukuri.app/".into()],
                    Vec::new(),
                )
                .expect("resolved urls"),
            ),
        }],
    })
    .expect("normalized config");

    let resolved = config.nodes[0]
        .resolved_urls
        .as_ref()
        .expect("resolved urls");

    assert_eq!(config.nodes[0].base_url, "https://api.kukuri.app");
    assert_eq!(resolved.public_base_url, "https://api.kukuri.app");
    assert_eq!(
        resolved.connectivity_urls,
        vec!["https://iroh-relay.kukuri.app".to_string()]
    );
    assert!(
        resolved
            .connectivity_urls
            .iter()
            .all(|url| !url.contains("api.kukuri.app/relay"))
    );
}

#[tokio::test]
async fn local_community_node_seed_peer_includes_addr_hint() {
    let _resource = lock_test_resource(TestResource::ProcessEnvironment).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-seed-peer-addr-hint.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let seed_peer = runtime
        .local_community_node_seed_peer("test")
        .await
        .expect("seed peer");

    assert!(seed_peer.addr_hint.is_some());

    runtime.shutdown().await;
}

#[tokio::test]
async fn local_community_node_seed_peer_keeps_addr_hint_when_relay_urls_exist() {
    let _resource = lock_test_resource(TestResource::ProcessEnvironment).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-seed-peer-relay-auto-hint.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::default(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: "https://api.example.com".to_string(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(
                    "https://api.example.com",
                    vec!["https://relay.example.com".to_string()],
                    Vec::new(),
                )
                .expect("resolved urls"),
            ),
        }],
    };

    let seed_peer = runtime
        .local_community_node_seed_peer("test")
        .await
        .expect("seed peer");

    assert!(seed_peer.addr_hint.is_some());

    runtime.shutdown().await;
}

#[test]
fn stored_community_node_config_restores_cached_connectivity_union() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-relay.db");
    save_community_node_config(
        &db_path,
        &CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://community.example.com".into(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(
                        "https://public.example.com",
                        vec!["https://relay.example.com".into()],
                        vec![CommunityNodeSeedPeer::new("peer-a", None).expect("seed peer")],
                    )
                    .expect("resolved urls"),
                ),
            }],
        },
    )
    .expect("save community node config");
    let restored = load_community_node_config_from_file(&db_path)
        .expect("load community node config")
        .expect("community node config");
    assert_eq!(
        restored.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .connectivity_urls,
        vec!["https://relay.example.com".to_string()]
    );
    assert_eq!(
        restored.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![CommunityNodeSeedPeer::new("peer-a", None).expect("seed peer")]
    );
}

#[test]
fn legacy_auto_approve_field_is_ignored_and_removed_on_save() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-legacy-auto-approve.db");
    let config_path = community_node_config_path(&db_path);
    std::fs::write(
        &config_path,
        r#"{
  "nodes": [
    {
      "base_url": "https://community.example.com/",
      "auto_approve": true,
      "resolved_urls": null
    }
  ]
}"#,
    )
    .expect("write legacy config");

    let config = load_community_node_config_from_file(&db_path)
        .expect("load legacy config")
        .expect("stored config");
    assert_eq!(config.nodes.len(), 1);
    assert_eq!(config.nodes[0].base_url, "https://community.example.com");

    save_community_node_config(&db_path, &config).expect("save normalized config");
    let saved = std::fs::read_to_string(config_path).expect("read normalized config");
    assert!(!saved.contains("auto_approve"));
}

#[tokio::test]
async fn runtime_preloads_distribution_community_node_only_when_config_file_is_missing() {
    let _resource = lock_test_resource(TestResource::ProcessEnvironment).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-preview-preload.db");

    let runtime = DesktopRuntime::new_with_config_and_identity_and_discovery(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
        DiscoveryConfig::static_peer_default(),
        DhtDiscoveryOptions::disabled(),
        Some(CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://distribution.example.com".to_string(),
                resolved_urls: None,
            }],
        }),
    )
    .await
    .expect("runtime");

    let config = runtime
        .get_community_node_config()
        .await
        .expect("community node config");
    assert_eq!(config.nodes.len(), 1);
    assert_eq!(config.nodes[0].base_url, "https://distribution.example.com");
    assert!(
        community_node_config_path(&db_path).exists(),
        "preloaded preview config should be persisted"
    );

    runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_does_not_restore_distribution_node_after_user_clears_config() {
    let _resource = lock_test_resource(TestResource::ProcessEnvironment).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-distribution-cleared.db");
    save_community_node_config(&db_path, &CommunityNodeConfig::default())
        .expect("save cleared config");

    let runtime = DesktopRuntime::new_with_config_and_identity_and_discovery(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
        DiscoveryConfig::static_peer_default(),
        DhtDiscoveryOptions::disabled(),
        Some(CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://distribution.example.com".to_string(),
                resolved_urls: None,
            }],
        }),
    )
    .await
    .expect("runtime");

    assert!(
        runtime
            .get_community_node_config()
            .await
            .unwrap()
            .nodes
            .is_empty()
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_does_not_restore_distribution_node_after_user_replaces_config() {
    let _resource = lock_test_resource(TestResource::ProcessEnvironment).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-distribution-replaced.db");
    save_community_node_config(
        &db_path,
        &CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://user-selected.example.com".to_string(),
                resolved_urls: None,
            }],
        },
    )
    .expect("save replacement config");

    let runtime = DesktopRuntime::new_with_config_and_identity_and_discovery(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
        DiscoveryConfig::static_peer_default(),
        DhtDiscoveryOptions::disabled(),
        Some(CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://distribution.example.com".to_string(),
                resolved_urls: None,
            }],
        }),
    )
    .await
    .expect("runtime");

    let config = runtime.get_community_node_config().await.unwrap();
    assert_eq!(config.nodes.len(), 1);
    assert_eq!(
        config.nodes[0].base_url,
        "https://user-selected.example.com"
    );
    runtime.shutdown().await;
}

// #1056: 採用設定の欄が無い既存の設定ファイルは「採用」として読み、保存で欄を持つ。
#[test]
fn legacy_config_without_content_advisory_field_defaults_to_enabled() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-legacy-advisory.db");
    let config_path = community_node_config_path(&db_path);
    std::fs::write(
        &config_path,
        r#"{"nodes":[{"base_url":"https://community.example.com","resolved_urls":null}]}"#,
    )
    .expect("write legacy config");

    let config = load_community_node_config_from_file(&db_path)
        .expect("load legacy config")
        .expect("stored config");
    assert!(config.nodes[0].content_advisory_enabled);

    let mut disabled = config.clone();
    disabled.nodes[0].content_advisory_enabled = false;
    save_community_node_config(&db_path, &disabled).expect("save");
    let reloaded = load_community_node_config_from_file(&db_path)
        .expect("reload")
        .expect("stored config");
    assert!(!reloaded.nodes[0].content_advisory_enabled);
}

// #1056: 重複指定では OFF を維持する(外部送信を増やさない側)。
#[test]
fn duplicate_nodes_keep_content_advisory_disabled() {
    let config = normalize_community_node_config(CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![
            CommunityNodeNodeConfig {
                content_advisory_enabled: false,
                base_url: "https://community.example.com/".into(),
                resolved_urls: None,
            },
            CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: "https://community.example.com".into(),
                resolved_urls: None,
            },
        ],
    })
    .expect("normalize");
    assert_eq!(config.nodes.len(), 1);
    assert!(!config.nodes[0].content_advisory_enabled);
}

// #1056 / TR-10: 保存時に未指定なら既存の採用設定を維持し、新規 node は採用、明示値は反映する。
#[tokio::test]
async fn set_community_node_config_keeps_or_updates_content_advisory_adoption() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-advisory-adoption.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    // 到達不能な node(設定保存だけを検証する。session 確立は失敗してよい)。
    let first = "http://127.0.0.1:9";
    let second = "http://127.0.0.1:10";

    let saved = runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: vec![SetCommunityNodeConfigNode {
                content_advisory_enabled: Some(false),
                base_url: first.to_string(),
            }],
        })
        .await
        .expect("save disabled");
    assert!(!saved.nodes[0].content_advisory_enabled);

    let saved = runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: vec![
                SetCommunityNodeConfigNode {
                    content_advisory_enabled: None,
                    base_url: first.to_string(),
                },
                SetCommunityNodeConfigNode {
                    content_advisory_enabled: None,
                    base_url: second.to_string(),
                },
            ],
        })
        .await
        .expect("save with unspecified adoption");
    let by_url = |url: &str| {
        saved
            .nodes
            .iter()
            .find(|node| node.base_url == url)
            .expect("node")
            .content_advisory_enabled
    };
    assert!(!by_url(first), "unspecified keeps the stored value");
    assert!(by_url(second), "new node defaults to enabled");

    let saved = runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: vec![SetCommunityNodeConfigNode {
                content_advisory_enabled: Some(true),
                base_url: first.to_string(),
            }],
        })
        .await
        .expect("save enabled");
    assert!(saved.nodes[0].content_advisory_enabled);
    let reloaded = load_community_node_config_from_file(&db_path)
        .expect("reload")
        .expect("stored config");
    assert!(reloaded.nodes[0].content_advisory_enabled);
    runtime.shutdown().await;
}
