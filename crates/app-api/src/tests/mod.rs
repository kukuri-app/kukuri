use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use futures_util::StreamExt;
#[cfg(feature = "iroh-integration-tests")]
use iroh::address_lookup::{AddrFilter, AddressLookup};
#[cfg(feature = "iroh-integration-tests")]
use iroh_mainline_address_lookup::DhtAddressLookup;
#[cfg(feature = "iroh-integration-tests")]
use kukuri_blob_service::IrohBlobService;
use kukuri_core::{build_block_edge_envelope, build_follow_edge_envelope};
#[cfg(feature = "iroh-integration-tests")]
use kukuri_docs_sync::IrohDocsSync;
#[cfg(feature = "iroh-integration-tests")]
use kukuri_iroh_node::IrohDocsNode;
use kukuri_store::{
    BookmarkedCustomReactionRow, ContentObservationRow, ContentObservationStore,
    DirectMessageStore, LiveGameProjectionStore, MemoryStore, ReactionBookmarkStore,
    SocialProjectionStore, SqliteStore,
};
#[cfg(feature = "iroh-integration-tests")]
use kukuri_transport::{DhtDiscoveryOptions, IrohGossipTransport, TransportRelayConfig};
use kukuri_transport::{
    DiscoveryMode, FakeNetwork, FakeTransport, HintEnvelope, HintStream, SeedPeer,
};
#[cfg(feature = "iroh-integration-tests")]
use n0_mainline::{DhtBuilder, Testnet};
use std::sync::OnceLock;
use tempfile::tempdir;
use tokio::sync::{Mutex as TokioMutex, broadcast};
use tokio::time::{Duration, sleep, timeout};
use tokio_stream::wrappers::BroadcastStream;

fn minimal_metaverse_glb_bytes() -> Vec<u8> {
    let json = br#"{"asset":{"version":"2.0"},"accessors":[{"count":3}],"meshes":[{"primitives":[{"indices":0}]}]}"#;
    let padded_len = (json.len() + 3) & !3;
    let total_len = 12 + 8 + padded_len;
    let mut glb = Vec::with_capacity(total_len);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2_u32.to_le_bytes());
    glb.extend_from_slice(&(total_len as u32).to_le_bytes());
    glb.extend_from_slice(&(padded_len as u32).to_le_bytes());
    glb.extend_from_slice(&0x4e4f534a_u32.to_le_bytes());
    glb.extend_from_slice(json);
    glb.resize(total_len, b' ');
    glb
}

fn minimal_metaverse_png_bytes() -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0,
        0, 1, 0, 0, 0, 1,
    ]
}

mod capability_registry_snapshot;
mod direct_messages;
mod dome_connections;
mod dome_delete;
mod dome_hosting;
mod dome_listing;
mod dome_move;
mod game;
mod game_projection_freshness;
mod live;
mod media;
mod media_adult_gating;
mod notifications;
mod private_channels;
mod reactions;
mod shared_replica_keys;
mod social;
mod sync;
mod timeline;
mod topic_normalization;
mod views_wire_snapshot;

mod support;
use support::*;
