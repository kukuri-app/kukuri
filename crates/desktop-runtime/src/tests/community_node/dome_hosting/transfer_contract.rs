use super::*;
use kukuri_app_api::{CreateMetaverseRoomInput, DomeLayoutCommitOutcome, SubmitDomeSessionInput};
use kukuri_cn_protocol::{
    DOME_HOSTING_ACTIVATE_PATH, DOME_HOSTING_ASSIGNMENTS_PATH, DOME_HOSTING_LAYOUT_CANDIDATE_PATH,
    DomeHostingActivationRequest, DomeHostingAssignmentRequest, DomeHostingAssignmentResponse,
    DomeHostingLayoutCandidateRequest, DomeHostingLayoutCandidateResponse,
    DomeHostingStatusResponse,
};
use kukuri_core::{
    DomeHostingStateKindV1, DomeLayoutCandidateV1, DomeSessionInputKindV1,
    MetaversePersistentPropV1, MetaversePrimitive, SpatialContextV1, TopicId,
    accept_dome_hosting_lease, build_signed_dome_layout_candidate,
    verify_signed_dome_hosting_lease,
};

// 0 success / 1 assignment HTTP failure / 2 invalid signed acceptance / 3 activation HTTP failure.
struct TransferNode {
    keys: KukuriKeys,
    failure: AtomicUsize,
    changed_layout: AtomicBool,
    assignments: Mutex<Vec<DomeHostingAssignmentRequest>>,
    acceptances: Mutex<Vec<kukuri_core::SignedDomeHostingAcceptanceV1>>,
    activations: Mutex<Vec<DomeHostingActivationRequest>>,
    candidates: AtomicUsize,
    inputs: AtomicUsize,
    releases: Mutex<Vec<kukuri_cn_protocol::DomeHostingReleaseRequest>>,
    policy_update_after_assignment: Mutex<Option<Arc<MockManagedCommunityNodeState>>>,
}

impl TransferNode {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            keys: KukuriKeys::generate(),
            failure: AtomicUsize::new(0),
            changed_layout: AtomicBool::new(false),
            assignments: Mutex::new(Vec::new()),
            acceptances: Mutex::new(Vec::new()),
            activations: Mutex::new(Vec::new()),
            candidates: AtomicUsize::new(0),
            inputs: AtomicUsize::new(0),
            releases: Mutex::new(Vec::new()),
            policy_update_after_assignment: Mutex::new(None),
        })
    }
    fn routes(self: &Arc<Self>) -> Router {
        Router::new()
            .route(
                kukuri_cn_protocol::DOME_HOSTING_SESSION_INPUT_PATH,
                post(reject_input),
            )
            .route(DOME_HOSTING_ASSIGNMENTS_PATH, post(assign))
            .route(DOME_HOSTING_ACTIVATE_PATH, post(activate))
            .route(kukuri_cn_protocol::DOME_HOSTING_RELEASE_PATH, post(release))
            .route(DOME_HOSTING_LAYOUT_CANDIDATE_PATH, post(candidate))
            .with_state(self.clone())
    }
    async fn counts(&self) -> (usize, usize) {
        (
            self.assignments.lock().await.len(),
            self.activations.lock().await.len(),
        )
    }
}

async fn assign(
    State(node): State<Arc<TransferNode>>,
    headers: HeaderMap,
    Json(request): Json<DomeHostingAssignmentRequest>,
) -> Result<Json<DomeHostingAssignmentResponse>, StatusCode> {
    assert!(
        headers
            .get(AUTHORIZATION)
            .expect("authenticated assignment")
            .to_str()
            .unwrap()
            .starts_with("Bearer ")
    );
    verify_signed_dome_hosting_lease(&request.signed_lease, &request.instance_manifest)
        .expect("owner-signed binding");
    assert_eq!(
        request.instance_manifest.preset_ref.revision,
        request.preset_manifest.revision
    );
    assert_eq!(
        request.instance_manifest.preset_ref.owner_pubkey,
        request.preset_manifest.owner_pubkey
    );
    node.assignments.lock().await.push(request.clone());
    if node.failure.load(Ordering::SeqCst) == 1 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let session = format!("node-session-{}", request.signed_lease.lease.epoch);
    let mut signed_acceptance = accept_dome_hosting_lease(
        &node.keys,
        &request.signed_lease,
        session.clone(),
        Utc::now().timestamp_millis(),
    )
    .expect("node-signed acceptance");
    if node.failure.load(Ordering::SeqCst) == 2 {
        signed_acceptance.acceptance.session_id = "tampered-session".into();
    }
    node.acceptances
        .lock()
        .await
        .push(signed_acceptance.clone());
    if let Some(managed) = node.policy_update_after_assignment.lock().await.as_ref() {
        managed
            .simulate_pending_update
            .store(true, Ordering::SeqCst);
    }
    Ok(Json(DomeHostingAssignmentResponse {
        signed_acceptance,
        state: DomeHostingStateKindV1::Transferring,
        session_id: session,
    }))
}

async fn activate(
    State(node): State<Arc<TransferNode>>,
    Json(request): Json<DomeHostingActivationRequest>,
) -> Result<Json<DomeHostingStatusResponse>, StatusCode> {
    request
        .signed_activation
        .envelope
        .verify()
        .expect("owner-signed activation");
    let last = node
        .assignments
        .lock()
        .await
        .last()
        .cloned()
        .expect("assigned first");
    let activation = &request.signed_activation.activation;
    assert_eq!(request.instance_id, last.instance_manifest.instance_id);
    assert_eq!(activation.lease_epoch, last.signed_lease.lease.epoch);
    let accepted = node
        .acceptances
        .lock()
        .await
        .last()
        .cloned()
        .expect("acceptance");
    assert_eq!(
        activation.host_acceptance_envelope_id,
        accepted.envelope.id.as_str()
    );
    node.activations.lock().await.push(request.clone());
    if node.failure.load(Ordering::SeqCst) == 3 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(DomeHostingStatusResponse {
        instance_id: request.instance_id,
        state: DomeHostingStateKindV1::CommunityNodeHosted,
        lease_epoch: activation.lease_epoch,
        session_id: Some(accepted.acceptance.session_id),
        participants: 0,
        sleeping: true,
        signed_heartbeat: None,
        expires_at: last.signed_lease.lease.expires_at,
        resource_budget: Default::default(),
        resource_metrics: Default::default(),
    }))
}

fn prop() -> MetaversePersistentPropV1 {
    MetaversePersistentPropV1 {
        prop_id: "contract-cube".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: [120, 100, 120],
        rotation: [0, 0, 0],
        scale: [100, 100, 100],
        visual_only: false,
        interactions: Vec::new(),
        collider: None,
    }
}

async fn candidate(
    State(node): State<Arc<TransferNode>>,
    Json(request): Json<DomeHostingLayoutCandidateRequest>,
) -> Json<DomeHostingLayoutCandidateResponse> {
    node.candidates.fetch_add(1, Ordering::SeqCst);
    let current = node
        .assignments
        .lock()
        .await
        .last()
        .cloned()
        .expect("assigned");
    let lease = &current.signed_lease.lease;
    let persistent_props = if node.changed_layout.load(Ordering::SeqCst) {
        vec![prop()]
    } else {
        current
            .preset_manifest
            .dome
            .customization
            .persistent_props
            .clone()
    };
    Json(DomeHostingLayoutCandidateResponse {
        signed_candidate: build_signed_dome_layout_candidate(
            &node.keys,
            lease,
            DomeLayoutCandidateV1 {
                operation_id: request.operation_id,
                instance_id: request.instance_id,
                instance_generation: lease.instance_generation,
                lease_epoch: lease.epoch,
                session_id: format!("node-session-{}", lease.epoch),
                host_pubkey: node.keys.public_key(),
                base_manifest_revision: current.preset_manifest.revision,
                snapshot_sequence: 1,
                captured_at: Utc::now().timestamp_millis(),
                persistent_props,
            },
        )
        .expect("signed candidate"),
    })
}

async fn create_owner(runtime: &DesktopRuntime) -> (SpatialContextV1, String) {
    let topic = "kukuri:topic:transfer-contract";
    let instance = runtime
        .app_service
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "transfer boundary".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("Dome");
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(topic),
    };
    runtime
        .start_owner_dome_hosting(crate::StartOwnerDomeHostingRequest {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: instance.clone(),
            endpoint_id: "owner-device".into(),
            lease_duration_millis: 600_000,
        })
        .await
        .expect("owner hosting");
    (context, instance)
}

fn delegate_request(
    node: &TransferNode,
    base_url: &str,
    context: &SpatialContextV1,
    instance: &str,
) -> crate::DelegateDomeHostingRequest {
    crate::DelegateDomeHostingRequest {
        expected_generation: None,
        spatial_context: context.clone(),
        instance_id: instance.into(),
        node_id: node.keys.public_key_hex(),
        base_url: base_url.into(),
        lease_duration_millis: 600_000,
    }
}

fn layout_request(context: &SpatialContextV1, instance: &str) -> crate::CommitDomeLayoutRequest {
    crate::CommitDomeLayoutRequest {
        spatial_context: context.clone(),
        instance_id: instance.into(),
        operation_id: "layout-once".into(),
    }
}

async fn saved_layout_operations(
    runtime: &DesktopRuntime,
    context: &SpatialContextV1,
    instance: &str,
) -> usize {
    let current = runtime.iroh_stack.current.lock().await;
    let docs = current.as_ref().expect("runtime stack").docs_sync.clone();
    drop(current);
    // #1221 R5-H: 公開の context の layout の commit は owner の制御領域に置く。
    assert!(matches!(context, SpatialContextV1::Topic { .. }));
    let owner = runtime.author_keys.public_key_hex();
    docs.query_replica(
        &kukuri_docs_sync::author_replica_id(owner.as_str()),
        DocQuery::Prefix(format!("metaverse/dome-layout-commits/{instance}/")),
    )
    .await
    .expect("saved layout operations")
    .len()
}

#[tokio::test]
async fn public_delegation_preserves_each_transfer_failure_boundary_and_retry() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    for failure in 0..=3 {
        let node = TransferNode::new();
        let (runtime, base_url, _, server, _dir) = dome_runtime_with_routes(node.routes()).await;
        seed_local_community_node_consents(&runtime, &base_url, 1).await;
        let (context, instance) = create_owner(&runtime).await;
        node.failure.store(failure, Ordering::SeqCst);
        let result = runtime
            .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
            .await;
        assert_eq!(result.is_ok(), failure == 0, "failure {failure}");
        let saved = runtime
            .app_service
            .get_dome_hosting(context.clone(), &instance)
            .await
            .expect("saved hosting");
        assert_eq!(saved.state.lease_epoch, Some(2));
        if failure == 0 {
            let returned = result.expect("successful delegation view");
            assert_eq!(returned.state.kind, saved.state.kind);
            assert_eq!(returned.state.lease_epoch, saved.state.lease_epoch);
            assert_eq!(returned.state.session_id, saved.state.session_id);
            assert_eq!(returned.signed_lease_json, saved.signed_lease_json);
            assert_eq!(
                returned.signed_activation_json,
                saved.signed_activation_json
            );
        }
        assert_eq!(
            node.counts().await,
            (1, usize::from(failure == 0 || failure == 3))
        );
        if failure == 1 || failure == 2 {
            assert_eq!(saved.state.kind, DomeHostingStateKindV1::Transferring);
            assert!(saved.signed_activation_json.is_none());
        } else {
            assert_eq!(
                saved.state.kind,
                DomeHostingStateKindV1::CommunityNodeHosted
            );
            assert_eq!(saved.state.session_id.as_deref(), Some("node-session-2"));
            assert!(
                saved.signed_activation_json.is_some(),
                "HTTP failure must not undo saved activation"
            );
        }
        if failure != 0 {
            node.failure.store(0, Ordering::SeqCst);
            let retried = runtime
                .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
                .await
                .expect("retry delegation");
            assert_eq!(retried.state.lease_epoch, Some(3));
            assert_eq!(retried.state.session_id.as_deref(), Some("node-session-3"));
        }
        runtime.shutdown().await;
        server.abort();
    }
}

#[tokio::test]
async fn public_layout_restart_preserves_transfer_failure_and_operation_retry() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    for failure in 0..=3 {
        let node = TransferNode::new();
        let (runtime, base_url, _, server, _dir) = dome_runtime_with_routes(node.routes()).await;
        seed_local_community_node_consents(&runtime, &base_url, 1).await;
        let (context, instance) = create_owner(&runtime).await;
        runtime
            .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
            .await
            .expect("initial delegation");
        node.changed_layout.store(true, Ordering::SeqCst);
        node.failure.store(failure, Ordering::SeqCst);
        let result = runtime
            .commit_dome_layout(layout_request(&context, &instance))
            .await;
        assert_eq!(result.is_ok(), failure == 0, "failure {failure}");
        let saved = runtime
            .app_service
            .get_dome_hosting(context.clone(), &instance)
            .await
            .expect("saved hosting");
        assert_eq!(saved.state.lease_epoch, Some(3));
        if failure == 0 {
            let returned = result.expect("successful layout view");
            assert_eq!(returned.outcome, DomeLayoutCommitOutcome::Committed);
            assert_eq!(returned.revision, 2);
            assert_eq!(returned.operation_id, "layout-once");
            assert!(returned.signed_commit_json.is_some());
            assert_eq!(returned.hosting.state.lease_epoch, saved.state.lease_epoch);
            assert_eq!(returned.hosting.state.session_id, saved.state.session_id);
            assert_eq!(returned.hosting.signed_lease_json, saved.signed_lease_json);
            assert_eq!(
                returned.hosting.signed_activation_json,
                saved.signed_activation_json
            );
        }
        let manifest: kukuri_core::DomePresetManifestV1 = serde_json::from_str(
            saved
                .preset_manifest_json
                .as_deref()
                .expect("available preset"),
        )
        .expect("preset");
        assert_eq!(manifest.revision, 2);
        assert_eq!(manifest.dome.customization.persistent_props, vec![prop()]);
        assert_eq!(
            saved_layout_operations(&runtime, &context, &instance).await,
            1
        );
        let before_retry = node.counts().await;
        assert_eq!(
            before_retry,
            (2, 1 + usize::from(failure == 0 || failure == 3))
        );
        assert_eq!(
            saved.signed_activation_json.is_some(),
            failure == 0 || failure == 3
        );
        node.failure.store(0, Ordering::SeqCst);
        let retried = runtime
            .commit_dome_layout(layout_request(&context, &instance))
            .await
            .expect("retry same operation");
        assert_eq!(retried.revision, 2);
        assert_eq!(retried.hosting.state.lease_epoch, Some(3));
        assert_eq!(
            saved_layout_operations(&runtime, &context, &instance).await,
            1
        );
        if failure == 1 || failure == 2 {
            assert_eq!(node.counts().await, (3, 2));
        } else {
            assert_eq!(
                node.counts().await,
                before_retry,
                "already activated operation sends no new transfer"
            );
        }
        assert_eq!(node.candidates.load(Ordering::SeqCst), 2);
        runtime.shutdown().await;
        server.abort();
    }
}

#[tokio::test]
async fn cn_noop_and_owner_layout_changes_do_not_send_transfer_requests() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let node = TransferNode::new();
    let (runtime, base_url, _, server, _dir) = dome_runtime_with_routes(node.routes()).await;
    seed_local_community_node_consents(&runtime, &base_url, 1).await;
    let (context, instance) = create_owner(&runtime).await;
    let no_op = runtime
        .commit_dome_layout(layout_request(&context, &instance))
        .await
        .expect("owner no-op");
    assert_eq!(no_op.outcome, DomeLayoutCommitOutcome::NoOp);
    assert_eq!(no_op.revision, 1);
    assert_eq!(
        saved_layout_operations(&runtime, &context, &instance).await,
        0
    );
    runtime
        .app_service
        .submit_dome_session_input(SubmitDomeSessionInput {
            expected_generation: None,
            spatial_context: context.clone(),
            instance_id: instance.clone(),
            sequence: 1,
            input: DomeSessionInputKindV1::UpsertPersistentProp { prop: prop() },
        })
        .await
        .expect("owner changes layout");
    let committed = runtime
        .commit_dome_layout(layout_request(&context, &instance))
        .await
        .expect("owner commit");
    assert_eq!(committed.revision, 2);
    assert_eq!(
        committed.hosting.state.kind,
        DomeHostingStateKindV1::OwnerHosted
    );
    assert_eq!(committed.hosting.state.lease_epoch, Some(2));
    let retry = runtime
        .commit_dome_layout(layout_request(&context, &instance))
        .await
        .expect("owner retry");
    assert_eq!(retry.revision, committed.revision);
    assert_eq!(retry.signed_commit_json, committed.signed_commit_json);
    assert_eq!(
        saved_layout_operations(&runtime, &context, &instance).await,
        1
    );
    assert_eq!(node.counts().await, (0, 0));
    runtime
        .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
        .await
        .expect("delegate");
    let no_op = runtime
        .commit_dome_layout(crate::CommitDomeLayoutRequest {
            operation_id: "cn-noop".into(),
            ..layout_request(&context, &instance)
        })
        .await
        .expect("CN no-op");
    assert_eq!(no_op.outcome, DomeLayoutCommitOutcome::NoOp);
    assert_eq!(no_op.revision, 2);
    assert_eq!(
        saved_layout_operations(&runtime, &context, &instance).await,
        1
    );
    assert_eq!(node.counts().await, (1, 1));
    assert_eq!(node.candidates.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn activation_rechecks_current_consent_after_assignment_for_both_entries() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    for layout in [false, true] {
        let node = TransferNode::new();
        let (runtime, base_url, managed, server, _dir) =
            dome_runtime_with_routes(node.routes()).await;
        seed_local_community_node_consents(&runtime, &base_url, 1).await;
        let (context, instance) = create_owner(&runtime).await;
        if layout {
            runtime
                .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
                .await
                .expect("initial delegation");
            node.changed_layout.store(true, Ordering::SeqCst);
        }
        *node.policy_update_after_assignment.lock().await = Some(managed.clone());
        let before = node.counts().await;
        let result = if layout {
            runtime
                .commit_dome_layout(layout_request(&context, &instance))
                .await
                .map(|_| ())
        } else {
            runtime
                .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
                .await
                .map(|_| ())
        };
        let error = result.expect_err("new policy stops activation HTTP");
        assert_eq!(
            error
                .downcast_ref::<crate::DomeHostingRequestError>()
                .expect("typed guard")
                .code,
            CONSENT_REQUIRED_CODE
        );
        assert_eq!(node.counts().await, (before.0 + 1, before.1));
        let saved = runtime
            .app_service
            .get_dome_hosting(context, &instance)
            .await
            .expect("saved activation");
        assert!(saved.signed_activation_json.is_some());
        assert_eq!(saved.state.lease_epoch, Some(if layout { 3 } else { 2 }));
        runtime.shutdown().await;
        server.abort();
    }
}

#[tokio::test]
async fn public_transfer_entries_cannot_bypass_current_consent_or_configured_node() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    for layout in [false, true] {
        for guard in 0..4 {
            if layout && guard == 0 {
                continue;
            } // CN layout starts from a previously consented lease.
            let node = TransferNode::new();
            let (runtime, base_url, managed, server, _dir) =
                dome_runtime_with_routes(node.routes()).await;
            if guard != 0 {
                seed_local_community_node_consents(&runtime, &base_url, 1).await;
            }
            let (context, instance) = create_owner(&runtime).await;
            if layout {
                runtime
                    .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
                    .await
                    .expect("initial delegation");
            }
            match guard {
                0 => {}
                1 => {
                    runtime
                        .withdraw_community_node_consents(crate::CommunityNodeTargetRequest {
                            base_url: base_url.clone(),
                        })
                        .await
                        .expect("withdraw consent");
                }
                2 => {
                    managed
                        .simulate_pending_update
                        .store(true, Ordering::SeqCst);
                }
                _ => {
                    *runtime.community_node_config.lock().await = CommunityNodeConfig::default();
                }
            }
            let requests = node.counts().await;
            let auth = (
                managed.challenge_hits.load(Ordering::SeqCst),
                managed.verify_hits.load(Ordering::SeqCst),
            );
            let result = if layout {
                runtime
                    .commit_dome_layout(layout_request(&context, &instance))
                    .await
                    .map(|_| ())
            } else {
                runtime
                    .delegate_dome_hosting(delegate_request(&node, &base_url, &context, &instance))
                    .await
                    .map(|_| ())
            };
            let error = result.expect_err("guard rejects public entry");
            let error = error
                .downcast_ref::<crate::DomeHostingRequestError>()
                .expect("typed rejection");
            assert_eq!(
                error.code,
                if guard == 3 {
                    "DOME_HOSTING_TARGET_NOT_CONFIGURED"
                } else {
                    CONSENT_REQUIRED_CODE
                }
            );
            assert_eq!(node.counts().await, requests);
            assert_eq!(node.candidates.load(Ordering::SeqCst), 0);
            assert_eq!(
                (
                    managed.challenge_hits.load(Ordering::SeqCst),
                    managed.verify_hits.load(Ordering::SeqCst)
                ),
                auth
            );
            runtime.shutdown().await;
            server.abort();
        }
    }
}

async fn release(
    State(node): State<Arc<TransferNode>>,
    Json(request): Json<kukuri_cn_protocol::DomeHostingReleaseRequest>,
) -> Result<Json<DomeHostingStatusResponse>, StatusCode> {
    request
        .signed_close
        .envelope
        .verify()
        .expect("signed close");
    let epoch = request.signed_close.close.lease_epoch;
    node.releases.lock().await.push(request.clone());
    if node.failure.load(Ordering::SeqCst) == 4 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(DomeHostingStatusResponse {
        instance_id: request.instance_id,
        state: DomeHostingStateKindV1::Closed,
        lease_epoch: epoch,
        session_id: None,
        participants: 0,
        sleeping: true,
        signed_heartbeat: None,
        expires_at: 0,
        resource_budget: Default::default(),
        resource_metrics: Default::default(),
    }))
}

#[tokio::test]
async fn deletion_preserves_canonical_close_and_retries_the_same_node_release() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let node = TransferNode::new();
    let (runtime, url, _, server, _dir) = dome_runtime_with_routes(node.routes()).await;
    seed_local_community_node_consents(&runtime, &url, 1).await;
    let (context, instance) = create_owner(&runtime).await;
    runtime
        .delegate_dome_hosting(delegate_request(&node, &url, &context, &instance))
        .await
        .unwrap();
    let input = crate::DeleteDomeRequest {
        spatial_context: context.clone(),
        instance_id: instance.clone(),
        expected_generation: 1,
        operation_id: "delete-cn".into(),
    };
    let mut stale = input.clone();
    stale.expected_generation = 2;
    assert!(runtime.delete_dome(stale).await.is_err());
    assert!(node.releases.lock().await.is_empty());
    node.failure.store(4, Ordering::SeqCst);
    let result = runtime.delete_dome(input.clone()).await.unwrap();
    assert!(result.deleted && result.cleanup_pending);
    assert!(
        runtime
            .app_service
            .list_game_rooms(context.topic_id().as_str())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        runtime
            .list_pending_dome_deletions(context.clone())
            .await
            .unwrap()
            .len(),
        1
    );
    node.failure.store(0, Ordering::SeqCst);
    assert!(!runtime.delete_dome(input).await.unwrap().cleanup_pending);
    assert!(
        runtime
            .list_pending_dome_deletions(context)
            .await
            .unwrap()
            .is_empty()
    );
    let releases = node.releases.lock().await;
    assert!(releases.len() >= 2);
    assert!(
        releases
            .iter()
            .all(|r| r.signed_close == releases[0].signed_close)
    );
    drop(releases);
    runtime.shutdown().await;
    server.abort();
}

async fn reject_input(State(node): State<Arc<TransferNode>>) -> StatusCode {
    node.inputs.fetch_add(1, Ordering::SeqCst);
    StatusCode::SERVICE_UNAVAILABLE
}

#[tokio::test]
async fn stale_session_input_never_reaches_recreated_community_node_host() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let node = TransferNode::new();
    let (runtime, url, _, server, _dir) = dome_runtime_with_routes(node.routes()).await;
    seed_local_community_node_consents(&runtime, &url, 1).await;
    let (context, instance) = create_owner(&runtime).await;
    runtime
        .delete_dome(crate::DeleteDomeRequest {
            spatial_context: context.clone(),
            instance_id: instance.clone(),
            expected_generation: 1,
            operation_id: "replace-input".into(),
        })
        .await
        .unwrap();
    let (_, recreated) = create_owner(&runtime).await;
    assert_eq!(instance, recreated);
    runtime
        .delegate_dome_hosting(delegate_request(&node, &url, &context, &instance))
        .await
        .unwrap();
    for (i, input) in [
        DomeSessionInputKindV1::Join {
            avatar_collider: None,
        },
        DomeSessionInputKindV1::UpsertPersistentProp { prop: prop() },
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            runtime
                .submit_dome_session_input(crate::SubmitDomeSessionInputRequest {
                    expected_generation: Some(1),
                    spatial_context: context.clone(),
                    instance_id: instance.clone(),
                    sequence: i as u64 + 1,
                    input
                })
                .await
                .is_err()
        );
    }
    assert_eq!(
        node.inputs.load(Ordering::SeqCst),
        0,
        "stale inputs must be rejected before HTTP"
    );
    runtime.shutdown().await;
    server.abort();
}
