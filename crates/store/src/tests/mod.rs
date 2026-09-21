use super::*;
use sqlx::sqlite::SqlitePoolOptions;

use crate::sqlite::STORE_MIGRATOR;
use kukuri_core::{EnvelopeId, ObjectStatus, Profile, ReactionKeyKind};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kukuri_core::{
    BlobHash, FollowEdgeStatus, PayloadRef, ReplicaId, TopicId, build_follow_edge_envelope,
    build_post_envelope, generate_keys,
};
use tempfile::tempdir;

mod backend_parity;
mod content_observations;
mod direct_messages;
mod migrations;
mod migrations_roundtrip;
mod page_query_plans;
mod pagination;
mod post_withdrawals;
mod row_mapping_edge;
mod row_mapping_enums;
mod row_mapping_roundtrip;
mod row_mapping_roundtrip_dm;
mod row_mapping_roundtrip_live_game;
mod row_mapping_roundtrip_social;
mod sqlite_projection;
mod sqlite_store;
