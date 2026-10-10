use super::*;
use kukuri_cn_core::{
    AdmissionMode, JwtConfig, TestDatabase, accept_consents, create_auth_challenge,
    verify_auth_envelope_and_issue_token,
};
use kukuri_core::{
    DomeHostingLeaseV1, DomeInstanceStatusV1, DomePresetRefV1, MetaverseDomeV1,
    MetaverseRoomSpawnV1, SpatialContextV1, TopicId, activate_dome_hosting_lease,
    build_signed_dome_hosting_lease, close_dome_hosting_lease,
};
use tokio::sync::Notify;

fn assignment(
    owner: &KukuriKeys,
    node: &KukuriKeys,
    generation: u64,
) -> DomeHostingAssignmentRequest {
    let now = chrono::Utc::now().timestamp_millis();
    let preset = DomePresetManifestV1 {
        preset_id: format!("preset-{generation}"),
        owner_pubkey: owner.public_key(),
        revision: 1,
        dome: MetaverseDomeV1::default(),
        asset_refs: vec![],
        updated_at: now,
    };
    let bytes = serde_json::to_vec(&preset).unwrap();
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new("kukuri:topic:delete-race"),
    };
    let instance = DomeInstanceManifestV1 {
        instance_id: "same-instance".into(),
        spatial_context: context.clone(),
        owner_pubkey: owner.public_key(),
        preset_ref: DomePresetRefV1 {
            preset_id: preset.preset_id.clone(),
            owner_pubkey: owner.public_key(),
            revision: 1,
            manifest_blob_hash: hash.clone(),
            manifest_mime: kukuri_core::DOME_PRESET_MANIFEST_MIME.into(),
            manifest_bytes: bytes.len() as u64,
        },
        title: "race".into(),
        description: String::new(),
        max_peers: Some(8),
        default_spawn: MetaverseRoomSpawnV1 {
            position: [0, 0, 260],
            rotation: [0, 180, 0],
        },
        generation,
        status: DomeInstanceStatusV1::Active,
        relationship_detach: None,
        replacement_instance_id: None,
        chat_history: vec![],
        updated_at: now,
    };
    let lease = build_signed_dome_hosting_lease(
        owner,
        DomeHostingLeaseV1 {
            lease_id: format!("lease-{generation}"),
            spatial_context: context,
            instance_id: instance.instance_id.clone(),
            instance_generation: generation,
            owner_pubkey: owner.public_key(),
            host: DomeHostTargetV1::CommunityNode {
                node_id: node.public_key(),
                api_base_url: "http://127.0.0.1:13330".into(),
            },
            manifest_blob_hash: hash,
            manifest_version: 1,
            epoch: generation,
            issued_at: now,
            expires_at: now + 600_000,
        },
    )
    .unwrap();
    DomeHostingAssignmentRequest {
        signed_lease: lease,
        instance_manifest: instance,
        preset_manifest: preset,
        asset_blobs: vec![],
    }
}

async fn activate_generation(
    state: UserApiState,
    headers: HeaderMap,
    owner: &KukuriKeys,
    request: DomeHostingAssignmentRequest,
) {
    let accepted =
        assign_dome_hosting(State(state.clone()), headers.clone(), Json(request.clone()))
            .await
            .unwrap()
            .0;
    let activation = activate_dome_hosting_lease(
        owner,
        &request.signed_lease,
        &accepted.signed_acceptance,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap();
    let activated = activate_dome_hosting(
        State(state),
        headers,
        Json(DomeHostingActivationRequest {
            instance_id: "same-instance".into(),
            signed_activation: activation,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        activated.0.state,
        DomeHostingStateKindV1::CommunityNodeHosted
    );
}

#[tokio::test]
async fn old_release_cannot_remove_recreated_generation_runtime_or_pins() -> Result<()> {
    release_race(false, false).await
}

#[tokio::test]
async fn release_lock_protects_pins_across_server_instances() -> Result<()> {
    release_race(true, false).await
}

#[tokio::test]
async fn expired_status_cannot_remove_recreated_generation() -> Result<()> {
    release_race(false, true).await
}

async fn release_race(separate_node_state: bool, expired_status: bool) -> Result<()> {
    if std::env::var("KUKURI_CN_RUN_INTEGRATION_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping CN deletion race test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    }
    let admin = std::env::var("COMMUNITY_NODE_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://cn:cn_password@127.0.0.1:15432/cn".into());
    let database = TestDatabase::create(&admin, "cn_delete_race").await?;
    let jwt = JwtConfig::new("delete-race", "test-delete-race-key", 3600);
    let config = crate::UserApiConfig {
        bind_addr: "127.0.0.1:0".parse()?,
        database_url: database.database_url.clone(),
        rendezvous_redis_url: std::env::var("COMMUNITY_NODE_RENDEZVOUS_REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:16379/".into()),
        rendezvous_key_prefix: "cn:delete:race".into(),
        base_url: "http://127.0.0.1:13330".into(),
        public_base_url: "http://127.0.0.1:13330".into(),
        connectivity_urls: vec![],
        jwt_config: jwt.clone(),
        operator_config_path: None,
        channel_secret_key: None,
        legal_data_key: None,
        index_query_enabled: false,
        indexer_data_dir: Default::default(),
        trust_read_enabled: false,
        relation_distance_optout_min_proximity: None,
        deployment_revision: "test".into(),
        readiness_activation_max_age_secs: 3600,
        expected_issuer_node_id: None,
    };
    let mut state = crate::build_state(&config).await?;
    let node = KukuriKeys::generate();
    state.dome_hosting = Some(Arc::new(
        DomeHostingNodeState::restore(state.pool.clone(), node.clone(), Default::default()).await?,
    ));
    let owner = KukuriKeys::generate();
    let challenge = create_auth_challenge(&state.pool, &owner.public_key_hex()).await?;
    let envelope = kukuri_cn_protocol::build_auth_envelope_json(
        &owner,
        &challenge.challenge,
        &config.public_base_url,
    )?;
    let token = verify_auth_envelope_and_issue_token(
        &state.pool,
        &jwt,
        &config.public_base_url,
        &envelope,
        None,
        AdmissionMode::Open,
        None,
    )
    .await?;
    let policies = kukuri_cn_core::list_policies(&state.pool).await?;
    let slugs = policies
        .iter()
        .map(|p| p.policy_slug.clone())
        .collect::<Vec<_>>();
    let snapshot = policies
        .first()
        .and_then(|p| p.policy_snapshot_revision.as_deref());
    accept_consents(&state.pool, &owner.public_key_hex(), &slugs, snapshot).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {}", token.access_token).parse()?,
    );
    let original = assignment(&owner, &node, 1);
    activate_generation(state.clone(), headers.clone(), &owner, original.clone()).await;
    let reached = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    *state
        .dome_hosting
        .as_ref()
        .unwrap()
        .lifecycle_pause
        .lock()
        .await = Some((reached.clone(), resume.clone()));
    let close = close_dome_hosting_lease(
        &owner,
        &original.signed_lease,
        chrono::Utc::now().timestamp_millis(),
    )?;
    if expired_status {
        sqlx::query("UPDATE cn_metaverse.dome_hosting_assignments SET expires_at=0 WHERE instance_id='same-instance'")
            .execute(&state.pool).await?;
    }
    let old_state = state.clone();
    let old_headers = headers.clone();
    let old_release = tokio::spawn(async move {
        if expired_status {
            dome_hosting_status(State(old_state), Path("same-instance".into()), old_headers).await
        } else {
            release_dome_hosting(
                State(old_state),
                old_headers,
                Json(DomeHostingReleaseRequest {
                    instance_id: "same-instance".into(),
                    signed_close: close,
                }),
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), reached.notified()).await?;
    let next = assignment(&owner, &node, 2);
    let next_hash = next.signed_lease.lease.manifest_blob_hash.clone();
    let mut next_state = state.clone();
    if separate_node_state {
        next_state.dome_hosting = Some(Arc::new(
            DomeHostingNodeState::restore(state.pool.clone(), node.clone(), Default::default())
                .await?,
        ));
    }
    let next_hosting = next_state.dome_hosting.clone().unwrap();
    let next_owner = owner.clone();
    let mut new_host =
        tokio::spawn(
            async move { activate_generation(next_state, headers, &next_owner, next).await },
        );
    // Let the new request race while the old release is between DB close and pin/runtime cleanup.
    let completed_early =
        tokio::time::timeout(std::time::Duration::from_millis(300), &mut new_host)
            .await
            .is_ok();
    resume.notify_one();
    assert_eq!(
        old_release.await?.unwrap().0.state,
        DomeHostingStateKindV1::Closed
    );
    if !completed_early {
        new_host.await?;
    }
    let hosting = &next_hosting;
    assert_eq!(
        hosting
            .sessions
            .lock()
            .await
            .get("same-instance")
            .map(|r| r.lease().epoch),
        Some(2)
    );
    let pins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cn_metaverse.dome_blob_pins WHERE blob_hash=$1 AND reason='active_lease'")
        .bind(next_hash).fetch_one(&state.pool).await?;
    assert_eq!(pins, 1);
    drop(state);
    database.cleanup().await?;
    Ok(())
}
