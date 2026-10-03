use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use kukuri_cn_core::{
    ApiError, ApiResult, NewDomeHostingAssignment, StagedDomeBlob,
    activate_dome_hosting_assignment, activate_dome_hosting_blob_pins,
    close_dome_hosting_assignment, get_dome_hosting_assignment,
    list_recoverable_dome_hosting_assignments, release_dome_hosting_blob_pins,
    require_bearer_identity, require_consents, stage_dome_hosting_blobs,
    upsert_pending_dome_hosting_assignment,
};
use kukuri_cn_protocol::{
    DomeHostingActivationRequest, DomeHostingAssignmentRequest, DomeHostingAssignmentResponse,
    DomeHostingLayoutCandidateRequest, DomeHostingLayoutCandidateResponse,
    DomeHostingReleaseRequest, DomeHostingSessionInputRequest, DomeHostingSessionSnapshotResponse,
    DomeHostingSnapshotResyncRequest, DomeHostingSnapshotResyncResponse, DomeHostingStatusResponse,
    DomeTransitionAbortRequest, DomeTransitionCommitRequest, DomeTransitionMutationResponse,
    DomeTransitionPrepareRequest, DomeTransitionPrepareResponse,
};
use kukuri_core::{
    DomeHostTargetV1, DomeHostingRecordV1, DomeHostingStateKindV1, DomeInstanceManifestV1,
    DomePresetManifestV1, DomeSpatialAccessProofV1, DomeTransitionAccessDecisionV1, KukuriKeys,
    SignedDomeHostingAcceptanceV1, SignedDomeHostingActivationV1, SignedDomeHostingLeaseV1,
    accept_dome_hosting_lease, resolve_dome_hosting_state, validate_dome_preset_manifest,
    verify_signed_dome_hosting_lease,
};
use kukuri_metaverse_host::DomeSessionRuntime;
use sqlx::postgres::PgPool;
use tokio::sync::Mutex;

use crate::state::UserApiState;

pub(crate) struct DomeHostingNodeState {
    keys: KukuriKeys,
    lifecycle: Mutex<()>,
    sessions: Arc<Mutex<HashMap<String, DomeSessionRuntime>>>,
    budget: kukuri_core::MetaverseResourceBudgetConfig,
    #[cfg(test)]
    lifecycle_pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

impl DomeHostingNodeState {
    pub(crate) async fn restore(
        pool: PgPool,
        keys: KukuriKeys,
        budget: kukuri_core::MetaverseResourceBudgetConfig,
    ) -> Result<Self> {
        budget.validate()?;
        let state = Self {
            keys,
            lifecycle: Mutex::new(()),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            budget,
            #[cfg(test)]
            lifecycle_pause: Mutex::new(None),
        };
        let now = chrono::Utc::now().timestamp_millis();
        for assignment in list_recoverable_dome_hosting_assignments(&pool, now).await? {
            let lease: SignedDomeHostingLeaseV1 =
                serde_json::from_value(assignment.signed_lease_json)?;
            let instance: DomeInstanceManifestV1 =
                serde_json::from_value(assignment.instance_manifest_json)?;
            let preset: DomePresetManifestV1 =
                serde_json::from_value(assignment.preset_manifest_json)?;
            verify_signed_dome_hosting_lease(&lease, &instance)?;
            let runtime = DomeSessionRuntime::start_with_budget(
                lease,
                state.keys.clone(),
                &instance,
                &preset,
                &assignment.session_id,
                now,
                state.budget.clone(),
            )?;
            state
                .sessions
                .lock()
                .await
                .insert(instance.instance_id, runtime);
        }
        Ok(state)
    }

    fn public_key(&self) -> kukuri_core::Pubkey {
        self.keys.public_key()
    }
}

async fn lock_hosting_lifecycle(
    pool: &PgPool,
    instance_id: &str,
) -> ApiResult<sqlx::Transaction<'static, sqlx::Postgres>> {
    let mut transaction = pool.begin().await.map_err(hosting_internal_error)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 1020))")
        .bind(instance_id)
        .execute(&mut *transaction)
        .await
        .map_err(hosting_internal_error)?;
    Ok(transaction)
}

pub(crate) async fn assign_dome_hosting(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeHostingAssignmentRequest>,
) -> ApiResult<Json<DomeHostingAssignmentResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    // Keep the assignment, its pins and in-memory session in one lifecycle operation.
    let _lifecycle = hosting.lifecycle.lock().await;
    let _database_lifecycle =
        lock_hosting_lifecycle(&state.pool, &request.signed_lease.lease.instance_id).await?;
    let lease = &request.signed_lease;
    if lease.lease.owner_pubkey.as_str() != identity.pubkey
        || lease.lease.instance_id != request.instance_manifest.instance_id
    {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_OWNER_MISMATCH",
            "bearer identity does not own this Dome Hosting Lease",
        ));
    }
    let node_id = match &lease.lease.host {
        DomeHostTargetV1::CommunityNode { node_id, .. } => node_id,
        DomeHostTargetV1::OwnerDevice { .. } => {
            return Err(hosting_error(
                StatusCode::BAD_REQUEST,
                "DOME_HOSTING_TARGET_INVALID",
                "Community Node assignment requires a Community Node lease target",
            ));
        }
    };
    if node_id != &hosting.public_key() {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_NODE_MISMATCH",
            "Hosting Lease targets a different Community Node",
        ));
    }
    verify_signed_dome_hosting_lease(lease, &request.instance_manifest)
        .map_err(hosting_contract_error)?;
    validate_dome_preset_manifest(&request.preset_manifest).map_err(hosting_contract_error)?;
    kukuri_core::validate_dome_asset_budget(&request.preset_manifest.asset_refs, &hosting.budget)
        .map_err(hosting_contract_error)?;
    if request.instance_manifest.preset_ref.preset_id != request.preset_manifest.preset_id
        || request.instance_manifest.preset_ref.owner_pubkey != request.preset_manifest.owner_pubkey
        || request.instance_manifest.preset_ref.revision != request.preset_manifest.revision
    {
        return Err(hosting_error(
            StatusCode::BAD_REQUEST,
            "DOME_HOSTING_MANIFEST_MISMATCH",
            "Dome Instance and Preset manifest do not match",
        ));
    }
    let now = chrono::Utc::now().timestamp_millis();
    if lease.lease.expires_at <= now {
        return Err(hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_LEASE_EXPIRED",
            "Dome Hosting Lease has expired",
        ));
    }
    let preset_bytes =
        serde_json::to_vec(&request.preset_manifest).map_err(hosting_internal_error)?;
    if blake3::hash(&preset_bytes).to_hex().to_string() != lease.lease.manifest_blob_hash {
        return Err(hosting_error(
            StatusCode::BAD_REQUEST,
            "DOME_HOSTING_MANIFEST_HASH_MISMATCH",
            "Dome Preset manifest bytes do not match the leased content hash",
        ));
    }
    let required_hashes = request
        .preset_manifest
        .asset_refs
        .iter()
        .map(|asset| asset.blob_hash.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let supplied_hashes = request
        .asset_blobs
        .iter()
        .map(|asset| asset.blob_hash.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if required_hashes != supplied_hashes {
        return Err(hosting_error(
            StatusCode::BAD_REQUEST,
            "DOME_HOSTING_ASSET_SET_MISMATCH",
            "Dome assignment must stage exactly the assets referenced by the Preset",
        ));
    }
    for asset in &request.preset_manifest.asset_refs {
        let supplied = request
            .asset_blobs
            .iter()
            .find(|blob| blob.blob_hash == asset.blob_hash)
            .expect("asset hash sets matched above");
        let inspected = kukuri_core::inspect_metaverse_asset(asset.kind.clone(), &supplied.bytes)
            .map_err(hosting_contract_error)?;
        if asset.budget_metadata.as_ref() != Some(&inspected)
            || asset.size_bytes != Some(supplied.bytes.len() as u64)
        {
            return Err(hosting_error(
                StatusCode::BAD_REQUEST,
                "METAVERSE_ASSET_METADATA_MISMATCH",
                "Dome asset bytes do not match inspected resource metadata",
            ));
        }
    }
    let cache_reference = format!(
        "{}:{}",
        lease.lease.instance_id, request.preset_manifest.revision
    );
    let mut staged_blobs = vec![StagedDomeBlob {
        blob_hash: lease.lease.manifest_blob_hash.clone(),
        data: preset_bytes,
    }];
    staged_blobs.extend(request.asset_blobs.iter().map(|asset| StagedDomeBlob {
        blob_hash: asset.blob_hash.clone(),
        data: asset.bytes.clone(),
    }));
    stage_dome_hosting_blobs(
        &state.pool,
        &cache_reference,
        &staged_blobs,
        now,
        hosting.budget.client.cache_capacity_bytes,
    )
    .await
    .map_err(hosting_conflict_error)?;
    let session_id = format!(
        "cn-session-{}-{}-{}",
        lease.lease.instance_id, lease.lease.epoch, now
    );
    let acceptance = accept_dome_hosting_lease(&hosting.keys, lease, &session_id, now)
        .map_err(hosting_contract_error)?;
    let stored = upsert_pending_dome_hosting_assignment(
        &state.pool,
        NewDomeHostingAssignment {
            instance_id: &lease.lease.instance_id,
            owner_pubkey: lease.lease.owner_pubkey.as_str(),
            lease_id: &lease.lease.lease_id,
            lease_epoch: lease.lease.epoch,
            expires_at: lease.lease.expires_at,
            session_id: &session_id,
            signed_lease_json: serde_json::to_value(lease).map_err(hosting_internal_error)?,
            instance_manifest_json: serde_json::to_value(&request.instance_manifest)
                .map_err(hosting_internal_error)?,
            preset_manifest_json: serde_json::to_value(&request.preset_manifest)
                .map_err(hosting_internal_error)?,
            signed_acceptance_json: serde_json::to_value(&acceptance)
                .map_err(hosting_internal_error)?,
        },
    )
    .await
    .map_err(hosting_conflict_error)?;
    let acceptance: SignedDomeHostingAcceptanceV1 =
        serde_json::from_value(stored.signed_acceptance_json).map_err(hosting_internal_error)?;
    Ok(Json(DomeHostingAssignmentResponse {
        signed_acceptance: acceptance,
        state: DomeHostingStateKindV1::Transferring,
        session_id: stored.session_id,
    }))
}

pub(crate) async fn activate_dome_hosting(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeHostingActivationRequest>,
) -> ApiResult<Json<DomeHostingStatusResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    // Keep the assignment, its pins and in-memory session in one lifecycle operation.
    let _lifecycle = hosting.lifecycle.lock().await;
    let _database_lifecycle = lock_hosting_lifecycle(&state.pool, &request.instance_id).await?;
    let assignment = get_dome_hosting_assignment(&state.pool, &request.instance_id)
        .await
        .map_err(hosting_internal_error)?
        .ok_or_else(|| {
            hosting_error(
                StatusCode::NOT_FOUND,
                "DOME_HOSTING_ASSIGNMENT_NOT_FOUND",
                "Dome hosting assignment was not found",
            )
        })?;
    if assignment.owner_pubkey != identity.pubkey {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_OWNER_MISMATCH",
            "only the lease owner can activate Dome hosting",
        ));
    }
    let lease: SignedDomeHostingLeaseV1 =
        serde_json::from_value(assignment.signed_lease_json.clone())
            .map_err(hosting_internal_error)?;
    let acceptance: SignedDomeHostingAcceptanceV1 =
        serde_json::from_value(assignment.signed_acceptance_json.clone())
            .map_err(hosting_internal_error)?;
    let records = vec![
        DomeHostingRecordV1::LeaseIssued(lease.clone()),
        DomeHostingRecordV1::HostAccepted(acceptance),
        DomeHostingRecordV1::LeaseActivated(request.signed_activation.clone()),
    ];
    let instance: DomeInstanceManifestV1 =
        serde_json::from_value(assignment.instance_manifest_json.clone())
            .map_err(hosting_internal_error)?;
    let preset: DomePresetManifestV1 =
        serde_json::from_value(assignment.preset_manifest_json.clone())
            .map_err(hosting_internal_error)?;
    let now = chrono::Utc::now().timestamp_millis();
    let resolved = resolve_dome_hosting_state(&instance, &records, now, Some(now))
        .map_err(hosting_contract_error)?;
    if resolved.kind != DomeHostingStateKindV1::CommunityNodeHosted {
        return Err(hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_ACTIVATION_INVALID",
            "owner activation did not produce an active Community Node lease",
        ));
    }
    let runtime = DomeSessionRuntime::start_with_budget(
        lease,
        hosting.keys.clone(),
        &instance,
        &preset,
        &assignment.session_id,
        now,
        hosting.budget.clone(),
    )
    .map_err(hosting_contract_error)?;
    activate_dome_hosting_assignment(
        &state.pool,
        &request.instance_id,
        request.signed_activation.activation.lease_epoch,
        serde_json::to_value(&request.signed_activation).map_err(hosting_internal_error)?,
    )
    .await
    .map_err(hosting_conflict_error)?;
    activate_dome_hosting_blob_pins(
        &state.pool,
        &request.instance_id,
        &format!("{}:{}", request.instance_id, preset.revision),
        now,
    )
    .await
    .map_err(hosting_internal_error)?;
    hosting
        .sessions
        .lock()
        .await
        .insert(request.instance_id.clone(), runtime);
    Ok(Json(status_from_runtime(
        &request.instance_id,
        hosting.sessions.lock().await.get(&request.instance_id),
        assignment.expires_at,
        DomeHostingStateKindV1::CommunityNodeHosted,
        assignment.lease_epoch,
        &hosting.budget,
    )))
}

pub(crate) async fn release_dome_hosting(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeHostingReleaseRequest>,
) -> ApiResult<Json<DomeHostingStatusResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    // Keep the assignment, its pins and in-memory session in one lifecycle operation.
    let _lifecycle = hosting.lifecycle.lock().await;
    let _database_lifecycle = lock_hosting_lifecycle(&state.pool, &request.instance_id).await?;
    let assignment = get_dome_hosting_assignment(&state.pool, &request.instance_id)
        .await
        .map_err(hosting_internal_error)?
        .ok_or_else(|| {
            hosting_error(
                StatusCode::NOT_FOUND,
                "DOME_HOSTING_ASSIGNMENT_NOT_FOUND",
                "Dome hosting assignment was not found",
            )
        })?;
    if assignment.owner_pubkey != identity.pubkey
        || request.signed_close.close.lease_epoch != assignment.lease_epoch
    {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_OWNER_MISMATCH",
            "only the current lease owner can release Dome hosting",
        ));
    }
    let lease: SignedDomeHostingLeaseV1 =
        serde_json::from_value(assignment.signed_lease_json.clone())
            .map_err(hosting_internal_error)?;
    let acceptance: SignedDomeHostingAcceptanceV1 =
        serde_json::from_value(assignment.signed_acceptance_json.clone())
            .map_err(hosting_internal_error)?;
    let mut records = vec![
        DomeHostingRecordV1::LeaseIssued(lease),
        DomeHostingRecordV1::HostAccepted(acceptance),
    ];
    if let Some(activation_json) = assignment.signed_activation_json.clone() {
        let activation: SignedDomeHostingActivationV1 =
            serde_json::from_value(activation_json).map_err(hosting_internal_error)?;
        records.push(DomeHostingRecordV1::LeaseActivated(activation));
    }
    records.push(DomeHostingRecordV1::LeaseClosed(
        request.signed_close.clone(),
    ));
    let instance: DomeInstanceManifestV1 =
        serde_json::from_value(assignment.instance_manifest_json.clone())
            .map_err(hosting_internal_error)?;
    let now = chrono::Utc::now().timestamp_millis();
    let resolved = resolve_dome_hosting_state(&instance, &records, now, None)
        .map_err(hosting_contract_error)?;
    if resolved.kind != DomeHostingStateKindV1::Closed {
        return Err(hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_RELEASE_INVALID",
            "owner close record did not close the active Hosting Lease",
        ));
    }
    close_dome_hosting_assignment(
        &state.pool,
        &request.instance_id,
        assignment.lease_epoch,
        serde_json::to_value(&request.signed_close).map_err(hosting_internal_error)?,
    )
    .await
    .map_err(hosting_conflict_error)?;
    let preset: DomePresetManifestV1 =
        serde_json::from_value(assignment.preset_manifest_json.clone())
            .map_err(hosting_internal_error)?;
    #[cfg(test)]
    if let Some((reached, resume)) = hosting.lifecycle_pause.lock().await.take() {
        reached.notify_one();
        resume.notified().await;
    }
    release_dome_hosting_blob_pins(
        &state.pool,
        &format!("{}:{}", request.instance_id, preset.revision),
        now,
    )
    .await
    .map_err(hosting_internal_error)?;
    hosting.sessions.lock().await.remove(&request.instance_id);
    Ok(Json(DomeHostingStatusResponse {
        instance_id: request.instance_id,
        state: DomeHostingStateKindV1::Closed,
        lease_epoch: assignment.lease_epoch,
        session_id: None,
        participants: 0,
        sleeping: true,
        signed_heartbeat: None,
        expires_at: assignment.expires_at,
        resource_budget: hosting.budget.clone(),
        resource_metrics: Default::default(),
    }))
}

pub(crate) async fn dome_hosting_status(
    State(state): State<UserApiState>,
    Path(instance_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<DomeHostingStatusResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    let _lifecycle = hosting.lifecycle.lock().await;
    let _database_lifecycle = lock_hosting_lifecycle(&state.pool, &instance_id).await?;
    let assignment = get_dome_hosting_assignment(&state.pool, &instance_id)
        .await
        .map_err(hosting_internal_error)?
        .ok_or_else(|| {
            hosting_error(
                StatusCode::NOT_FOUND,
                "DOME_HOSTING_ASSIGNMENT_NOT_FOUND",
                "Dome hosting assignment was not found",
            )
        })?;
    #[cfg(test)]
    if let Some((reached, resume)) = hosting.lifecycle_pause.lock().await.take() {
        reached.notify_one();
        resume.notified().await;
    }
    let now = chrono::Utc::now().timestamp_millis();
    let mut sessions = hosting.sessions.lock().await;
    if assignment.expires_at <= now {
        sessions.remove(&instance_id);
    }
    let mut runtime = sessions.get_mut(&instance_id);
    if let Some(runtime) = runtime.as_deref_mut() {
        runtime.advance_to(now).map_err(hosting_internal_error)?;
    }
    let state_kind = if assignment.expires_at <= now {
        DomeHostingStateKindV1::Closed
    } else if assignment.status == "active" {
        DomeHostingStateKindV1::CommunityNodeHosted
    } else if assignment.status == "pending" {
        DomeHostingStateKindV1::Transferring
    } else {
        DomeHostingStateKindV1::Closed
    };
    Ok(Json(status_from_runtime(
        &instance_id,
        runtime.as_deref(),
        assignment.expires_at,
        state_kind,
        assignment.lease_epoch,
        &hosting.budget,
    )))
}

pub(crate) async fn submit_dome_hosting_input(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeHostingSessionInputRequest>,
) -> ApiResult<Json<DomeHostingSessionSnapshotResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    if request.signed_input.input.participant_pubkey.as_str() != identity.pubkey {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_PARTICIPANT_MISMATCH",
            "bearer identity does not match Dome session input signer",
        ));
    }
    let instance_id = request.signed_input.input.instance_id.clone();
    let mut sessions = hosting.sessions.lock().await;
    let now = chrono::Utc::now().timestamp_millis();
    if sessions
        .get(&instance_id)
        .is_some_and(|runtime| runtime.lease().expires_at <= now)
    {
        sessions.remove(&instance_id);
        return Err(hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_LEASE_EXPIRED",
            "Dome Hosting Lease has expired",
        ));
    }
    let runtime = sessions.get_mut(&instance_id).ok_or_else(|| {
        hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_SESSION_INACTIVE",
            "Dome session is not active on this Community Node",
        )
    })?;
    if matches!(
        &request.signed_input.input.input,
        kukuri_core::DomeSessionInputKindV1::Join { .. }
            | kukuri_core::DomeSessionInputKindV1::KeepAlive
    ) {
        let proof_valid = dome_entry_access_proof_is_valid(
            request.access_proof.as_ref(),
            &request.signed_input.input.participant_pubkey,
            &runtime.lease().spatial_context,
            &runtime.lease().owner_pubkey,
            now,
        );
        if !proof_valid {
            return Err(hosting_error(
                StatusCode::FORBIDDEN,
                "DOME_ENTRY_ACCESS_DENIED",
                "Dome entry access proof is missing or invalid",
            ));
        }
    }
    let admission = matches!(
        &request.signed_input.input.input,
        kukuri_core::DomeSessionInputKindV1::Join { .. }
    );
    runtime
        .apply_signed_input_at(&request.signed_input, now)
        .map_err(hosting_contract_error)?;
    let snapshot = if admission {
        runtime.signed_admission_snapshot(now)
    } else {
        runtime.signed_snapshot(now)
    }
    .map_err(hosting_contract_error)?;
    Ok(Json(DomeHostingSessionSnapshotResponse {
        signed_snapshot: snapshot,
    }))
}

fn dome_entry_access_proof_is_valid(
    proof: Option<&DomeSpatialAccessProofV1>,
    participant_pubkey: &kukuri_core::Pubkey,
    spatial_context: &kukuri_core::SpatialContextV1,
    target_owner_pubkey: &kukuri_core::Pubkey,
    now: i64,
) -> bool {
    proof.is_some_and(|proof| {
        proof
            .verify_for(
                participant_pubkey,
                spatial_context,
                target_owner_pubkey,
                now,
            )
            .is_ok()
    })
}

pub(crate) async fn prepare_dome_transition(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeTransitionPrepareRequest>,
) -> ApiResult<Json<DomeTransitionPrepareResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    if request.request.participant_pubkey.as_str() != identity.pubkey {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_PARTICIPANT_MISMATCH",
            "bearer identity does not match Dome transition participant",
        ));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let assignment = get_dome_hosting_assignment(&state.pool, &request.request.target_instance_id)
        .await
        .map_err(hosting_internal_error)?
        .ok_or_else(|| {
            hosting_error(
                StatusCode::NOT_FOUND,
                "DOME_HOSTING_ASSIGNMENT_NOT_FOUND",
                "destination Dome hosting assignment was not found",
            )
        })?;
    let mut sessions = hosting.sessions.lock().await;
    let runtime = sessions
        .get_mut(&request.request.target_instance_id)
        .ok_or_else(|| {
            hosting_error(
                StatusCode::CONFLICT,
                "DOME_HOSTING_SESSION_INACTIVE",
                "destination Dome session is not active on this Community Node",
            )
        })?;
    if runtime.lease().expires_at <= now {
        return Err(hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_LEASE_EXPIRED",
            "Dome Hosting Lease has expired",
        ));
    }
    let proof_valid = request.access_proof.statement.participant_pubkey
        == request.request.participant_pubkey
        && request.access_proof.statement.spatial_context == request.request.spatial_context
        && request.access_proof.statement.target_owner_pubkey.as_str() == assignment.owner_pubkey
        && request.access_proof.verify_at(now).is_ok();
    let access = if proof_valid {
        DomeTransitionAccessDecisionV1::Allowed
    } else {
        DomeTransitionAccessDecisionV1::Denied {
            reason: kukuri_core::DomeTransitionDenialReasonV1::AccessDenied,
        }
    };
    let ticket = runtime
        .prepare_transition_admission(request.request, access, now)
        .map_err(hosting_contract_error)?;
    Ok(Json(DomeTransitionPrepareResponse { ticket }))
}

pub(crate) async fn commit_dome_transition(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeTransitionCommitRequest>,
) -> ApiResult<Json<DomeTransitionMutationResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    if request.ticket.request.participant_pubkey.as_str() != identity.pubkey {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_PARTICIPANT_MISMATCH",
            "bearer identity does not match Dome transition participant",
        ));
    }
    hosting
        .sessions
        .lock()
        .await
        .get_mut(&request.ticket.request.target_instance_id)
        .ok_or_else(|| {
            hosting_error(
                StatusCode::CONFLICT,
                "DOME_HOSTING_SESSION_INACTIVE",
                "destination Dome session is not active on this Community Node",
            )
        })?
        .commit_transition_admission(
            &request.ticket,
            request.position,
            request.rotation,
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(hosting_contract_error)?;
    Ok(Json(DomeTransitionMutationResponse {
        transition_id: request.ticket.request.transition_id,
        state: "committed".into(),
    }))
}

pub(crate) async fn abort_dome_transition(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeTransitionAbortRequest>,
) -> ApiResult<Json<DomeTransitionMutationResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    if request.ticket.request.participant_pubkey.as_str() != identity.pubkey {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_HOSTING_PARTICIPANT_MISMATCH",
            "bearer identity does not match Dome transition participant",
        ));
    }
    hosting
        .sessions
        .lock()
        .await
        .get_mut(&request.ticket.request.target_instance_id)
        .ok_or_else(|| {
            hosting_error(
                StatusCode::CONFLICT,
                "DOME_HOSTING_SESSION_INACTIVE",
                "destination Dome session is not active on this Community Node",
            )
        })?
        .abort_transition_admission(
            &request.ticket.request.transition_id,
            &request.ticket.request.participant_pubkey,
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(hosting_contract_error)?;
    Ok(Json(DomeTransitionMutationResponse {
        transition_id: request.ticket.request.transition_id,
        state: "aborted".into(),
    }))
}

pub(crate) async fn capture_dome_layout_candidate(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeHostingLayoutCandidateRequest>,
) -> ApiResult<Json<DomeHostingLayoutCandidateResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    let assignment = get_dome_hosting_assignment(&state.pool, &request.instance_id)
        .await
        .map_err(hosting_internal_error)?
        .ok_or_else(|| {
            hosting_error(
                StatusCode::NOT_FOUND,
                "DOME_HOSTING_ASSIGNMENT_NOT_FOUND",
                "Dome hosting assignment was not found",
            )
        })?;
    if assignment.owner_pubkey != identity.pubkey {
        return Err(hosting_error(
            StatusCode::FORBIDDEN,
            "DOME_LAYOUT_OWNER_REQUIRED",
            "only the Dome owner can capture a durable layout candidate",
        ));
    }
    let mut sessions = hosting.sessions.lock().await;
    let runtime = sessions.get_mut(&request.instance_id).ok_or_else(|| {
        hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_SESSION_INACTIVE",
            "Dome session is not active on this Community Node",
        )
    })?;
    let candidate = runtime
        .signed_layout_candidate(&request.operation_id, chrono::Utc::now().timestamp_millis())
        .map_err(hosting_contract_error)?;
    Ok(Json(DomeHostingLayoutCandidateResponse {
        signed_candidate: candidate,
    }))
}

pub(crate) async fn resync_dome_hosting_snapshots(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<DomeHostingSnapshotResyncRequest>,
) -> ApiResult<Json<DomeHostingSnapshotResyncResponse>> {
    let hosting = require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    let sessions = hosting.sessions.lock().await;
    let runtime = sessions.get(&request.instance_id).ok_or_else(|| {
        hosting_error(
            StatusCode::CONFLICT,
            "DOME_HOSTING_SESSION_INACTIVE",
            "Dome session is not active on this Community Node",
        )
    })?;
    Ok(Json(DomeHostingSnapshotResyncResponse {
        snapshots: runtime.snapshots_after(request.after_sequence),
    }))
}

pub(crate) async fn dome_hosting_session_ws(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    require_hosting(&state)?;
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    Ok(upgrade.on_upgrade(move |socket| dome_hosting_socket(state, headers, socket)))
}

async fn dome_hosting_socket(state: UserApiState, headers: HeaderMap, mut socket: WebSocket) {
    while let Some(Ok(message)) = socket.recv().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let response = match serde_json::from_str::<DomeHostingSessionInputRequest>(&text) {
            Ok(request) => match submit_dome_hosting_input(
                State(state.clone()),
                headers.clone(),
                Json(request),
            )
            .await
            {
                Ok(Json(snapshot)) => serde_json::to_string(&snapshot),
                Err(_) => serde_json::to_string(&serde_json::json!({
                    "code": "DOME_HOSTING_INPUT_REJECTED",
                    "message": "signed Dome session input was rejected"
                })),
            },
            Err(_) => serde_json::to_string(&serde_json::json!({
                "code": "DOME_HOSTING_INPUT_INVALID",
                "message": "invalid Dome session input frame"
            })),
        };
        let Ok(response) = response else {
            break;
        };
        if socket.send(Message::Text(response.into())).await.is_err() {
            break;
        }
    }
}

fn require_hosting(state: &UserApiState) -> ApiResult<Arc<DomeHostingNodeState>> {
    state.dome_hosting.clone().ok_or_else(|| {
        hosting_error(
            StatusCode::NOT_FOUND,
            "DOME_HOSTING_NOT_CONFIGURED",
            "this Community Node does not provide Dome hosting",
        )
    })
}

fn status_from_runtime(
    instance_id: &str,
    runtime: Option<&DomeSessionRuntime>,
    expires_at: i64,
    state: DomeHostingStateKindV1,
    lease_epoch: u64,
    configured_budget: &kukuri_core::MetaverseResourceBudgetConfig,
) -> DomeHostingStatusResponse {
    DomeHostingStatusResponse {
        instance_id: instance_id.to_string(),
        state,
        lease_epoch,
        session_id: runtime.map(|runtime| runtime.session_id().to_string()),
        participants: runtime
            .map(|runtime| runtime.participant_count().try_into().unwrap_or(u32::MAX))
            .unwrap_or(0),
        sleeping: runtime.is_none_or(DomeSessionRuntime::is_sleeping),
        signed_heartbeat: runtime.and_then(|runtime| {
            runtime
                .signed_heartbeat(chrono::Utc::now().timestamp_millis())
                .ok()
        }),
        expires_at,
        resource_budget: runtime
            .map(|runtime| runtime.budget().clone())
            .unwrap_or_else(|| configured_budget.clone()),
        resource_metrics: runtime
            .map(DomeSessionRuntime::resource_metrics)
            .unwrap_or_default(),
    }
}

fn hosting_error(status: StatusCode, code: &'static str, message: &'static str) -> ApiError {
    ApiError::new(status, code, message)
}

fn hosting_contract_error(error: anyhow::Error) -> ApiError {
    if let Some(rejection) = error.downcast_ref::<kukuri_core::MetaverseResourceRejection>() {
        let status =
            if rejection.reason == kukuri_core::MetaverseResourceRejectionReason::RateExceeded {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
        return ApiError::new(
            status,
            "METAVERSE_RESOURCE_BUDGET_REJECTED",
            serde_json::to_string(rejection).unwrap_or_else(|_| rejection.to_string()),
        );
    }
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "DOME_HOSTING_CONTRACT_INVALID",
        format!("{error:#}"),
    )
}

fn hosting_conflict_error(error: anyhow::Error) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "DOME_HOSTING_CONFLICT",
        format!("{error:#}"),
    )
}

fn hosting_internal_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "DOME_HOSTING_INTERNAL",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use kukuri_core::{SpatialContextV1, TopicId, build_dome_spatial_access_proof, generate_keys};

    use super::dome_entry_access_proof_is_valid;

    #[test]
    fn entry_access_proof_is_required_bound_and_current() {
        let participant = generate_keys();
        let owner = generate_keys().public_key();
        let other_owner = generate_keys().public_key();
        let context = SpatialContextV1::Topic {
            topic_id: TopicId::new("kukuri:topic:cn-entry-proof"),
        };
        let proof = build_dome_spatial_access_proof(
            &participant,
            context.clone(),
            owner.clone(),
            1_000,
            None,
            None,
        )
        .expect("access proof");

        assert!(dome_entry_access_proof_is_valid(
            Some(&proof),
            &participant.public_key(),
            &context,
            &owner,
            1_001,
        ));
        assert!(!dome_entry_access_proof_is_valid(
            None,
            &participant.public_key(),
            &context,
            &owner,
            1_001,
        ));
        assert!(!dome_entry_access_proof_is_valid(
            Some(&proof),
            &participant.public_key(),
            &context,
            &other_owner,
            1_001,
        ));
        assert!(!dome_entry_access_proof_is_valid(
            Some(&proof),
            &participant.public_key(),
            &context,
            &owner,
            11_000,
        ));
    }
}

#[cfg(test)]
#[path = "dome_hosting_delete_tests.rs"]
mod deletion_tests;
