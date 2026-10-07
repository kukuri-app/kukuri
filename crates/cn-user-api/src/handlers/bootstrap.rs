//! bootstrap(nodes / heartbeat)と topic rendezvous heartbeat。

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use kukuri_cn_core::{
    ApiResult, auth_required_error, load_bootstrap_nodes, load_bootstrap_seed_peers,
    refresh_bootstrap_peer_registration, require_bearer_identity, require_consents,
};
use kukuri_cn_protocol::{
    BootstrapHeartbeatRequest, BootstrapHeartbeatResponse, BootstrapNodesResponse,
    TopicRendezvousHeartbeat, TopicRendezvousHeartbeatResponse,
};

use crate::errors::{SupportEndpointError, SupportEndpointOperation, support_endpoint_error};
use crate::state::UserApiState;

pub(crate) async fn bootstrap_nodes(
    State(state): State<UserApiState>,
    headers: HeaderMap,
) -> ApiResult<Json<BootstrapNodesResponse>> {
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    let mut nodes = load_bootstrap_nodes(&state.pool, Some(state.self_node.clone()))
        .await
        .map_err(|source| {
            SupportEndpointError::new(SupportEndpointOperation::LoadBootstrapNodes, source)
        })
        .map_err(support_endpoint_error)?;
    let seed_peers = load_bootstrap_seed_peers(
        &state.pool,
        Some(identity.pubkey.as_str()),
        identity.endpoint_id.as_deref(),
    )
    .await
    .map_err(|source| {
        SupportEndpointError::new(SupportEndpointOperation::LoadBootstrapSeedPeers, source)
    })
    .map_err(support_endpoint_error)?;
    for node in &mut nodes {
        if node.base_url == state.self_node.base_url {
            node.resolved_urls.seed_peers = seed_peers.clone();
            node.resolved_urls.public_blob_search = state.blob_provider_search.is_some();
        }
    }
    Ok(Json(BootstrapNodesResponse { nodes }))
}

pub(crate) async fn bootstrap_heartbeat(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<BootstrapHeartbeatRequest>,
) -> ApiResult<Json<BootstrapHeartbeatResponse>> {
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    if let Some(bound_endpoint_id) = identity.endpoint_id.as_deref()
        && bound_endpoint_id != request.endpoint_id
    {
        return Err(auth_required_error("bearer token endpoint mismatch"));
    }
    let response = refresh_bootstrap_peer_registration(
        &state.pool,
        identity.pubkey.as_str(),
        request.endpoint_id.as_str(),
        request.addr_hint.as_deref(),
    )
    .await
    .map_err(|source| {
        SupportEndpointError::new(SupportEndpointOperation::RefreshBootstrapPeer, source)
    })
    .map_err(support_endpoint_error)?;
    Ok(Json(response))
}

pub(crate) async fn topic_rendezvous_heartbeat(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<TopicRendezvousHeartbeat>,
) -> ApiResult<Json<TopicRendezvousHeartbeatResponse>> {
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    if let Some(bound_endpoint_id) = identity.endpoint_id.as_deref()
        && bound_endpoint_id != request.endpoint_id
    {
        return Err(auth_required_error("bearer token endpoint mismatch"));
    }
    let response = state
        .rendezvous_store
        .heartbeat(
            request,
            state.self_node.resolved_urls.connectivity_urls.as_slice(),
        )
        .await
        .map_err(|source| {
            SupportEndpointError::new(SupportEndpointOperation::RecordRendezvousHeartbeat, source)
        })
        .map_err(support_endpoint_error)?;
    Ok(Json(response))
}
