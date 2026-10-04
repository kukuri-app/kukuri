//! 所有者の端末の Dome host への P2P の session 経路の要求と応答(ADR 0038、#1527)。

use serde::{Deserialize, Serialize};

use crate::{
    DomeSpatialAccessProofV1, MetaverseResourceRejection, SignedDomePhysicsSnapshotV1,
    SignedDomeSessionInputV1,
};

/// 要求 1 件の上限。署名済みの input と access proof が収まる。
pub const DOME_SESSION_REQUEST_MAX_BYTES: usize = 256 * 1024;
/// 応答 1 件の上限。rigid body の安全上限の snapshot が収まる。
pub const DOME_SESSION_RESPONSE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// 再同期の応答の上限。既定の budget の ring 100 件が収まる。超える分は新しい側だけを返す。
pub const DOME_SESSION_RESYNC_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DomeSessionRequestV1 {
    Input {
        signed_input: SignedDomeSessionInputV1,
    },
    ResyncSnapshots {
        instance_id: String,
        after_sequence: u64,
        access_proof: DomeSpatialAccessProofV1,
    },
}

impl DomeSessionRequestV1 {
    /// この要求への応答に許す大きさ。
    pub fn response_limit(&self) -> usize {
        match self {
            Self::ResyncSnapshots { .. } => DOME_SESSION_RESYNC_MAX_BYTES,
            Self::Input { .. } => DOME_SESSION_RESPONSE_MAX_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DomeSessionResponseV1 {
    Snapshot {
        signed_snapshot: Box<SignedDomePhysicsSnapshotV1>,
    },
    Snapshots {
        snapshots: Vec<SignedDomePhysicsSnapshotV1>,
    },
    /// host が拒否した理由。resource budget の拒否は型のまま返す(ADR 0041)。
    Rejected {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_rejection: Option<MetaverseResourceRejection>,
    },
}
