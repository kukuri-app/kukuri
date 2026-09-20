use super::*;
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::{get, post},
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::Utc;
use futures_util::StreamExt;
use image::{
    AnimationDecoder, Delay, DynamicImage, Frame, GenericImageView, ImageDecoder, ImageFormat,
    Rgba, RgbaImage,
};
use iroh::address_lookup::{AddrFilter, AddressLookup};
use iroh_mainline_address_lookup::DhtAddressLookup;
use kukuri_app_api::{GameScoreView, JoinedPrivateChannelView, SyncStatus, TimelineView};
use kukuri_cn_protocol::{
    BootstrapHeartbeatResponse, CommunityNodeConsentStatus, CommunityNodeResolvedUrls,
    CommunityNodeSeedPeer,
};
use kukuri_core::{
    AssetRole, ChannelAudienceKind, ChannelRef, GameRoomStatus, KukuriKeys, TimelineScope,
};
use kukuri_docs_sync::{DocQuery, DocsSync};
use kukuri_transport::{
    ConnectMode, DhtDiscoveryOptions, DiscoveryMode, SeedPeer, TransportNetworkConfig,
};
use n0_mainline::{DhtBuilder, Testnet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep, timeout};

use crate::attachments::{normalize_custom_reaction_gif, normalize_custom_reaction_static};
use crate::community_node::{
    BootstrapNodesResponse, StoredCommunityNodeToken, load_community_node_config_from_file,
    normalize_community_node_config, persist_community_node_token,
    relay_config_from_community_node_config, save_community_node_config,
};
use crate::discovery::resolve_discovery_config_from_env;
use crate::identity::IdentityStorageMode;
use crate::paths::{community_node_config_path, discovery_config_path};
pub(crate) use kukuri_test_support::{TestResource, lock_test_resource};

mod support;
pub(crate) use support::*;

mod accounts_migration;
mod attachments;
mod community_node;
mod device_backup;
mod identity_restart;
mod media_blob_restore;
mod private_channels;
mod reloadable_docs_sync;
mod replication_heuristics;
mod runtime_events;
mod seeded_dht;
mod static_peer;

mod account_logout;
