//! 所有者の端末で稼働中の Dome への P2P の session 経路(ADR 0038、#1527)。participant 側の要求と、
//! 所有者の端末の受け口の登録を持つ。

use std::sync::Weak;

use kukuri_core::{DomeSessionRequestV1, DomeSessionResponseV1, SignedDomePhysicsSnapshotV1};
use kukuri_iroh_node::{DomeHostUnreachable, DomeSessionHandler};

use super::*;
use crate::DomeHostingRequestError;

/// 所有者の端末の受け口。要求は app-api が Community Node host と同じ確認点で処理する。
/// runtime を所有しない(止まった後の要求は拒否する)。
pub(crate) fn dome_session_handler(app: Weak<AppService>) -> DomeSessionHandler {
    Arc::new(move |request| {
        let app = app.clone();
        Box::pin(async move {
            match app.upgrade() {
                Some(app) => app.serve_dome_session_request(&request).await,
                None => serde_json::to_vec(&DomeSessionResponseV1::Rejected {
                    message: "the Dome host is stopping".into(),
                    resource_rejection: None,
                })
                .unwrap_or_default(),
            }
        })
    })
}

impl DesktopRuntime {
    pub async fn resync_dome_snapshots(
        &self,
        request: ResyncDomeSnapshotsRequest,
    ) -> Result<Vec<kukuri_core::DomePhysicsSnapshotV1>> {
        let hosting = self
            .get_dome_hosting(GetDomeHostingRequest {
                spatial_context: request.spatial_context.clone(),
                instance_id: request.instance_id.clone(),
            })
            .await?;
        let lease = hosting.lease.context("Dome is not currently hosted")?;
        let session_id = hosting
            .state
            .session_id
            .context("Dome session is not active")?;
        let signed = match &lease.host {
            DomeHostTargetV1::OwnerDevice {
                endpoint_id,
                host_pubkey,
            } if host_pubkey != &self.author_keys.public_key() => {
                let access_proof = self
                    .app_service
                    .build_dome_access_proof(request.spatial_context, lease.owner_pubkey.clone())
                    .await?;
                let request = DomeSessionRequestV1::ResyncSnapshots {
                    instance_id: request.instance_id,
                    after_sequence: request.after_sequence,
                    access_proof,
                };
                match self.request_owner_device_host(endpoint_id, request).await? {
                    DomeSessionResponseV1::Snapshots { snapshots } => snapshots,
                    _ => bail!("unexpected Dome host response"),
                }
            }
            DomeHostTargetV1::OwnerDevice { .. } => {
                self.app_service
                    .resync_dome_snapshots(ResyncDomeSnapshotsInput {
                        spatial_context: request.spatial_context,
                        instance_id: request.instance_id,
                        after_sequence: request.after_sequence,
                    })
                    .await?
            }
            DomeHostTargetV1::CommunityNode { api_base_url, .. } => {
                self.resync_dome_snapshots_from_community_node(
                    api_base_url,
                    &DomeHostingSnapshotResyncRequest {
                        instance_id: request.instance_id,
                        after_sequence: request.after_sequence,
                    },
                )
                .await?
                .snapshots
            }
        };
        signed
            .into_iter()
            .map(|snapshot| {
                verify_signed_dome_physics_snapshot(&snapshot, &lease, &session_id)?;
                Ok(snapshot.snapshot)
            })
            .collect()
    }

    pub(crate) async fn submit_owner_device_input(
        &self,
        endpoint_id: &str,
        signed_input: kukuri_core::SignedDomeSessionInputV1,
    ) -> Result<SignedDomePhysicsSnapshotV1> {
        let request = DomeSessionRequestV1::Input { signed_input };
        match self.request_owner_device_host(endpoint_id, request).await? {
            DomeSessionResponseV1::Snapshot { signed_snapshot } => Ok(*signed_snapshot),
            _ => bail!("unexpected Dome host response"),
        }
    }

    /// lease が指す所有者の端末へ要求を 1 件送る。接続できなければ code `DOME_HOST_UNREACHABLE`、host の拒否は
    /// process 内の host と同じ形(resource budget の拒否は型のまま)の失敗にする。
    async fn request_owner_device_host(
        &self,
        endpoint_id: &str,
        request: DomeSessionRequestV1,
    ) -> Result<DomeSessionResponseV1> {
        let response = self
            .iroh_stack
            .dome_session_request(
                endpoint_id,
                &serde_json::to_vec(&request)?,
                request.response_limit(),
            )
            .await
            .map_err(|error| match error.downcast_ref::<DomeHostUnreachable>() {
                Some(unreachable) => DomeHostingRequestError {
                    code: "DOME_HOST_UNREACHABLE".into(),
                    message: unreachable.to_string(),
                    status: 503,
                }
                .into(),
                None => error,
            })?;
        match serde_json::from_slice(&response).context("invalid Dome host response")? {
            DomeSessionResponseV1::Rejected {
                resource_rejection: Some(rejection),
                ..
            } => Err(rejection.into()),
            DomeSessionResponseV1::Rejected { message, .. } => Err(anyhow!(message)),
            response => Ok(response),
        }
    }
}
