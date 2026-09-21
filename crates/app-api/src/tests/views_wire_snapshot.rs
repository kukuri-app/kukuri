//! Tauri IPC 契約(views)の wire golden(WP-S4 T2)。
//!
//! 高頻度 8 型グループの serde 出力を、frontend と共有する JSON fixture
//! (apps/desktop/src/lib/api/__fixtures__/views/*.json)と突き合わせて凍結する。
//! 同じ fixture を TS 側(viewsContract.ts の literal + viewsContract.test.ts)が
//! 型検査・値比較するため、Rust 側の変更は
//!   本テスト fail → fixture 更新 → vitest fail → TS literal 更新 → tsc fail →
//!   types.ts 更新
//! の順で frontend まで必ず伝播する(silent break の防止が目的)。
//!
//! fixture の再生成: `UPDATE_FIXTURES=1 cargo test -p kukuri-app-api views_wire`
//! を実行して差分をコミットする(TS 側は apps/desktop の
//! `node scripts/generate-views-contract.mjs` で literal を再生成)。
//! fail した場合はテストではなく変更側の IPC 互換影響を評価すること。

use std::path::PathBuf;

use kukuri_core::{
    ChannelAudienceKind, ChannelId, ChannelSharingState, DomeCustomizationV1, DomeEnvironmentV1,
    DomeInstanceStatusV1, DomePresetRefV1, DomeSurfaceCustomizationV1, GameRoomKind,
    GameRoomStatus, MetaverseAssetKind, MetaverseAssetRef, MetaverseDomeV1,
    MetaverseInteractionKind, MetaversePersistentPropV1, MetaversePrimitive,
    MetaverseRoomChatMessageV1, MetaverseRoomSpawnV1, MetaverseRoomStateV1, SpatialContextV1,
    TopicId,
};
use kukuri_store::{NotificationKind, TimelineCursor};
use kukuri_transport::{ConnectMode, ConnectionPath, DiscoveryMode};

use crate::views::{
    AttachmentView, AuthorSocialView, BlobViewStatus, BookmarkedPostView, ChannelAccessTokenExport,
    ChannelAccessTokenKind, ChannelAccessTokenPreview, ContentObservationView,
    ContentProvenanceView, CustomReactionAssetView, DeliveryState, DirectMessageConversationView,
    DirectMessageMessageView, DirectMessageStatusView, DirectMessageTimelineView, DiscoveryStatus,
    GameRoomView, GameScoreView, JoinedPrivateChannelView, NotificationView, PostView,
    ProfileAssetView, ReactionKeyView, ReactionSummaryView, ReplyPreviewAuthorView,
    ReplyPreviewView, RepostSourceView, SyncStatus, TimelineView, TopicSyncStatus,
};

const PUBKEY_A: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const PUBKEY_B: &str = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/lib/api/__fixtures__/views")
        .join(name)
}

fn assert_fixture<T: serde::Serialize>(name: &str, value: &T) {
    let mut rendered = serde_json::to_string_pretty(value).expect("fixture serializes");
    rendered.push('\n');
    let path = fixture_path(name);
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().expect("fixture dir")).expect("create fixture dir");
        std::fs::write(&path, &rendered).expect("write fixture");
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read {} ({error}); run `UPDATE_FIXTURES=1 cargo test -p \
                 kukuri-app-api views_wire` to (re)generate fixtures",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert_eq!(
        rendered, committed,
        "IPC wire form drifted from the shared fixture {name}; if the Rust-side change is \
         intentional, regenerate fixtures and follow the propagation to viewsContract.ts / \
         types.ts",
    );
}

fn profile_asset() -> ProfileAssetView {
    ProfileAssetView {
        hash: "a".repeat(64),
        mime: "image/png".to_string(),
        bytes: 2048,
        role: "profile_avatar".to_string(),
    }
}

fn attachment() -> AttachmentView {
    AttachmentView {
        hash: "b".repeat(64),
        mime: "image/png".to_string(),
        bytes: 4096,
        role: "image_original".to_string(),
        status: BlobViewStatus::Available,
        provenance: None,
    }
}

fn observed_attachment() -> AttachmentView {
    AttachmentView {
        provenance: Some(ContentProvenanceView {
            canonical_source: "blob".to_string(),
            observed_via: vec![ContentObservationView {
                node_base_url: "https://node.example".to_string(),
                capability: "community_index".to_string(),
                observed_at: 1_700_000_003_000,
            }],
        }),
        ..attachment()
    }
}

fn custom_reaction_asset() -> CustomReactionAssetView {
    CustomReactionAssetView {
        asset_id: "reaction-asset-1".to_string(),
        owner_pubkey: PUBKEY_A.to_string(),
        blob_hash: "c".repeat(64),
        search_key: "party-parrot".to_string(),
        mime: "image/gif".to_string(),
        bytes: 12345,
        width: 64,
        height: 64,
    }
}

fn post_view_full() -> PostView {
    PostView {
        object_id: "post-1".to_string(),
        envelope_id: "envelope-post-1".to_string(),
        author_pubkey: PUBKEY_A.to_string(),
        author_name: Some("alice".to_string()),
        author_display_name: Some("Alice".to_string()),
        author_picture_asset: Some(profile_asset()),
        following: true,
        followed_by: true,
        mutual: true,
        friend_of_friend: false,
        provenance: Some(ContentProvenanceView {
            canonical_source: "author_docs".to_string(),
            observed_via: vec![ContentObservationView {
                node_base_url: "https://node.example".to_string(),
                capability: "community_index".to_string(),
                observed_at: 1_700_000_003_000,
            }],
        }),
        withdrawal: None,
        content: "hello ipc contract".to_string(),
        content_status: BlobViewStatus::Available,
        attachments: vec![observed_attachment()],
        content_labels: vec!["adult".to_string()],
        created_at: 1_700_000_000,
        reply_to: Some("parent-1".to_string()),
        reply_preview: Some(ReplyPreviewView {
            object_id: "parent-1".to_string(),
            topic: "kukuri:topic:demo".to_string(),
            author: ReplyPreviewAuthorView {
                pubkey: PUBKEY_B.to_string(),
                name: Some("bob".to_string()),
                display_name: Some("Bob".to_string()),
                picture_asset: Some(profile_asset()),
            },
            content: "parent content".to_string(),
            attachments: vec![observed_attachment()],
            content_labels: Vec::new(),
            root_id: Some("root-1".to_string()),
            reply_to: None,
        }),
        root_id: Some("root-1".to_string()),
        object_kind: "comment".to_string(),
        published_topic_id: Some("kukuri:topic:demo".to_string()),
        origin_topic_id: Some("kukuri:topic:demo".to_string()),
        repost_of: Some(RepostSourceView {
            source_object_id: "source-1".to_string(),
            source_topic_id: "kukuri:topic:source".to_string(),
            source_author_pubkey: PUBKEY_B.to_string(),
            source_author_name: Some("bob".to_string()),
            source_author_display_name: None,
            source_author_picture_asset: None,
            source_object_kind: "post".to_string(),
            content: "original".to_string(),
            attachments: vec![],
            content_labels: Vec::new(),
            reply_to: None,
            root_id: None,
        }),
        repost_commentary: Some("great post".to_string()),
        is_threadable: true,
        channel_id: Some("chan-1".to_string()),
        audience_label: "Private channel".to_string(),
        reaction_summary: vec![ReactionSummaryView {
            reaction_key_kind: "emoji".to_string(),
            normalized_reaction_key: "emoji:👍".to_string(),
            emoji: Some("👍".to_string()),
            custom_asset: None,
            count: 3,
        }],
        my_reactions: vec![ReactionKeyView {
            reaction_key_kind: "custom".to_string(),
            normalized_reaction_key: "custom:reaction-asset-1".to_string(),
            emoji: None,
            custom_asset: Some(custom_reaction_asset()),
        }],
    }
}

fn post_view_minimal() -> PostView {
    PostView {
        object_id: "post-2".to_string(),
        envelope_id: "envelope-post-2".to_string(),
        author_pubkey: PUBKEY_B.to_string(),
        author_name: None,
        author_display_name: None,
        author_picture_asset: None,
        following: false,
        followed_by: false,
        mutual: false,
        friend_of_friend: false,
        provenance: None,
        withdrawal: None,
        content: "minimal".to_string(),
        content_status: BlobViewStatus::Missing,
        attachments: vec![],
        content_labels: vec![],
        created_at: 1_700_000_001,
        reply_to: None,
        reply_preview: None,
        root_id: None,
        object_kind: "post".to_string(),
        published_topic_id: None,
        origin_topic_id: None,
        repost_of: None,
        repost_commentary: None,
        is_threadable: true,
        channel_id: None,
        audience_label: "Public".to_string(),
        reaction_summary: vec![],
        my_reactions: vec![],
    }
}

#[test]
fn views_wire_post_view_full() {
    assert_fixture("post_view.full.json", &post_view_full());
}

#[test]
fn views_wire_post_view_minimal() {
    assert_fixture("post_view.minimal.json", &post_view_minimal());
}

#[test]
fn views_wire_timeline_view() {
    assert_fixture(
        "timeline_view.json",
        &TimelineView {
            items: vec![post_view_minimal()],
            next_cursor: Some(TimelineCursor {
                created_at: 1_700_000_001,
                object_id: kukuri_core::EnvelopeId("post-2".to_string()),
            }),
            unavailable_count: 2,
        },
    );
}

#[test]
fn views_wire_bookmarked_post_view() {
    assert_fixture(
        "bookmarked_post_view.json",
        &BookmarkedPostView {
            bookmarked_at: 1_700_000_050,
            post: post_view_minimal(),
        },
    );
}

fn sync_status_full() -> SyncStatus {
    SyncStatus {
        connected: true,
        delivery_state: DeliveryState::DurableRecovering,
        last_sync_ts: Some(1_700_000_100),
        peer_count: 2,
        pending_events: 0,
        status_detail: "durable recovering".to_string(),
        last_error: Some("previous error".to_string()),
        configured_peers: vec!["peer-a".to_string()],
        subscribed_topics: vec!["kukuri:topic:demo".to_string()],
        active_path: ConnectionPath::RelaySupportedP2p,
        fallback_peer_ids: vec![],
        topic_diagnostics: vec![TopicSyncStatus {
            topic: "kukuri:topic:demo".to_string(),
            joined: true,
            delivery_state: DeliveryState::Live,
            peer_count: 2,
            connected_peers: vec!["peer-a".to_string(), "peer-b".to_string()],
            docs_assist_peer_ids: vec!["peer-b".to_string()],
            configured_peer_ids: vec!["peer-a".to_string()],
            missing_peer_ids: vec![],
            active_path: ConnectionPath::RelayFallback,
            rendezvous_peer_ids: vec![],
            fallback_peer_ids: vec!["peer-a".to_string(), "peer-b".to_string()],
            last_received_at: Some(1_700_000_090),
            last_docs_activity_at: Some(1_700_000_095),
            status_detail: "live".to_string(),
            last_error: None,
        }],
        local_author_pubkey: PUBKEY_A.to_string(),
        discovery: DiscoveryStatus {
            mode: DiscoveryMode::SeededDht,
            connect_mode: ConnectMode::DirectOrRelay,
            active_path: ConnectionPath::RelaySupportedP2p,
            fallback_peer_ids: vec![],
            env_locked: false,
            configured_seed_peer_ids: vec!["seed-1".to_string()],
            bootstrap_seed_peer_ids: vec!["seed-2".to_string()],
            manual_ticket_peer_ids: vec!["manual-1".to_string()],
            connected_peer_ids: vec!["peer-a".to_string()],
            docs_assist_peer_ids: vec!["peer-b".to_string()],
            blob_assist_peer_ids: vec![],
            local_endpoint_id: "endpoint-1".to_string(),
            last_discovery_error: Some("dht timeout".to_string()),
        },
        gossip_disabled_topics: vec!["kukuri:topic:quiet".to_string()],
        gossip_disabled_channels: vec!["chan-quiet".to_string()],
    }
}

#[test]
fn views_wire_sync_status_full() {
    assert_fixture("sync_status.full.json", &sync_status_full());
}

#[test]
fn views_wire_sync_status_minimal() {
    assert_fixture(
        "sync_status.minimal.json",
        &SyncStatus {
            connected: false,
            delivery_state: DeliveryState::Offline,
            last_sync_ts: None,
            peer_count: 0,
            pending_events: 0,
            status_detail: "offline".to_string(),
            last_error: None,
            configured_peers: vec![],
            subscribed_topics: vec![],
            active_path: ConnectionPath::DirectP2p,
            fallback_peer_ids: vec![],
            topic_diagnostics: vec![],
            local_author_pubkey: PUBKEY_A.to_string(),
            discovery: DiscoveryStatus::default(),
            gossip_disabled_topics: vec![],
            gossip_disabled_channels: vec![],
        },
    );
}

#[test]
fn views_wire_notification_view() {
    assert_fixture(
        "notification_view.json",
        &NotificationView {
            notification_id: "notification-1".to_string(),
            kind: NotificationKind::QuoteRepost,
            actor_pubkey: PUBKEY_B.to_string(),
            actor_name: Some("bob".to_string()),
            actor_display_name: Some("Bob".to_string()),
            actor_picture_asset: Some(profile_asset()),
            source_envelope_id: Some("envelope-1".to_string()),
            source_replica_id: Some("topic::demo".to_string()),
            topic_id: Some("kukuri:topic:demo".to_string()),
            channel_id: Some("chan-1".to_string()),
            object_id: Some("post-1".to_string()),
            thread_root_object_id: Some("root-1".to_string()),
            dm_id: None,
            message_id: None,
            preview_text: Some("quoted you".to_string()),
            content_labels: Some(vec![kukuri_core::ADULT_CONTENT_LABEL.to_string()]),
            created_at: 1_700_000_200,
            received_at: 1_700_000_201,
            read_at: Some(1_700_000_210),
        },
    );
}

#[test]
fn views_wire_joined_private_channel_view() {
    assert_fixture(
        "joined_private_channel_view.json",
        &JoinedPrivateChannelView {
            topic_id: "kukuri:topic:demo".to_string(),
            channel_id: "chan-1".to_string(),
            label: "secret room".to_string(),
            creator_pubkey: PUBKEY_A.to_string(),
            owner_pubkey: PUBKEY_A.to_string(),
            joined_via_pubkey: Some(PUBKEY_B.to_string()),
            audience_kind: ChannelAudienceKind::FriendPlus,
            is_owner: false,
            current_epoch_id: "epoch-2".to_string(),
            archived_epoch_ids: vec!["epoch-1".to_string()],
            sharing_state: ChannelSharingState::Frozen,
            rotation_required: true,
            participant_count: 4,
            stale_participant_count: 1,
            entry_dome_instance_id: Some("dome-entry".to_string()),
        },
    );
}

fn dm_status() -> DirectMessageStatusView {
    DirectMessageStatusView {
        peer_pubkey: PUBKEY_B.to_string(),
        dm_id: "dm-1".to_string(),
        mutual: true,
        send_enabled: true,
        peer_count: 1,
        pending_outbox_count: 2,
    }
}

fn dm_message() -> DirectMessageMessageView {
    DirectMessageMessageView {
        dm_id: "dm-1".to_string(),
        message_id: "msg-1".to_string(),
        sender_pubkey: PUBKEY_A.to_string(),
        recipient_pubkey: PUBKEY_B.to_string(),
        created_at: 1_700_000_300,
        text: "hello dm".to_string(),
        reply_to_message_id: Some("msg-0".to_string()),
        attachments: vec![attachment()],
        outgoing: true,
        delivered: false,
    }
}

#[test]
fn views_wire_dm_conversation_view() {
    assert_fixture(
        "dm_conversation_view.json",
        &DirectMessageConversationView {
            dm_id: "dm-1".to_string(),
            peer_pubkey: PUBKEY_B.to_string(),
            peer_name: Some("bob".to_string()),
            peer_display_name: Some("Bob".to_string()),
            peer_picture_asset: Some(profile_asset()),
            updated_at: 1_700_000_310,
            last_message_at: Some(1_700_000_300),
            last_message_id: Some("msg-1".to_string()),
            last_message_preview: Some("hello dm".to_string()),
            status: dm_status(),
        },
    );
}

#[test]
fn views_wire_dm_message_and_timeline_view() {
    assert_fixture("dm_message_view.json", &dm_message());
    assert_fixture(
        "dm_timeline_view.json",
        &DirectMessageTimelineView {
            items: vec![dm_message()],
            next_cursor: Some(TimelineCursor {
                created_at: 1_700_000_300,
                object_id: kukuri_core::EnvelopeId("msg-1".to_string()),
            }),
        },
    );
}

#[test]
fn views_wire_game_room_view() {
    assert_fixture(
        "game_room_view.score.json",
        &GameRoomView {
            room_id: "room-1".to_string(),
            host_pubkey: PUBKEY_A.to_string(),
            title: "score battle".to_string(),
            description: "first to 10".to_string(),
            status: GameRoomStatus::Running,
            phase_label: Some("round 2".to_string()),
            scores: vec![GameScoreView {
                participant_id: "participant-1".to_string(),
                label: "alice".to_string(),
                score: 7,
            }],
            room_kind: GameRoomKind::ScoreGame,
            metaverse: None,
            dome_hosting: None,
            manifest_blob_hash: "d".repeat(64),
            updated_at: 1_700_000_400,
            channel_id: None,
            audience_label: "Public".to_string(),
        },
    );
    assert_fixture(
        "game_room_view.metaverse.json",
        &GameRoomView {
            room_id: "room-2".to_string(),
            host_pubkey: PUBKEY_A.to_string(),
            title: "metaverse room".to_string(),
            description: "hangout".to_string(),
            status: GameRoomStatus::Waiting,
            phase_label: None,
            scores: vec![],
            room_kind: GameRoomKind::MetaverseRoom,
            metaverse: Some(MetaverseRoomStateV1 {
                world_version: kukuri_core::METAVERSE_WORLD_VERSION,
                instance_id: "room-2".to_string(),
                spatial_context: SpatialContextV1::Channel {
                    topic_id: TopicId::new("kukuri:topic:wire"),
                    channel_id: ChannelId::new("chan-1"),
                },
                instance_generation: 1,
                instance_status: DomeInstanceStatusV1::Active,
                relationship_detach: None,
                replacement_instance_id: None,
                preset_ref: DomePresetRefV1 {
                    preset_id: "preset-2".to_string(),
                    owner_pubkey: PUBKEY_A.into(),
                    revision: 1,
                    manifest_blob_hash: "a".repeat(64),
                    manifest_mime: "application/vnd.kukuri.dome-preset+json".to_string(),
                    manifest_bytes: 2048,
                },
                session_id: "room-2".to_string(),
                max_peers: Some(8),
                dome: MetaverseDomeV1 {
                    spec_id: "fixed_dome_v1".to_string(),
                    customization: DomeCustomizationV1 {
                        surface: DomeSurfaceCustomizationV1::default(),
                        environment: DomeEnvironmentV1::default(),
                        persistent_props: vec![MetaversePersistentPropV1 {
                            prop_id: "shared-1".to_string(),
                            asset_ref: Some(MetaverseAssetRef {
                                kind: MetaverseAssetKind::Glb,
                                blob_hash: "e".repeat(64),
                                mime_type: Some("model/gltf-binary".to_string()),
                                size_bytes: Some(65536),
                                name: Some("statue".to_string()),
                                budget_metadata: Some(
                                    kukuri_core::MetaverseAssetBudgetMetadataV1 {
                                        stored_bytes: 65_536,
                                        model_triangles: 120,
                                        model_primitives: 1,
                                        ..Default::default()
                                    },
                                ),
                            }),
                            primitive_fallback: MetaversePrimitive::Cube,
                            position: [1, 2, 3],
                            rotation: [0, 90, 0],
                            scale: [100, 100, 100],
                            visual_only: false,
                            interactions: vec![
                                MetaverseInteractionKind::Grab,
                                MetaverseInteractionKind::Push,
                            ],
                            collider: None,
                        }],
                    },
                },
                default_spawn: MetaverseRoomSpawnV1 {
                    position: [0, 0, 5],
                    rotation: [0, 180, 0],
                },
                asset_refs: vec![],
                chat_history: vec![MetaverseRoomChatMessageV1 {
                    room_id: "room-2".to_string(),
                    message_id: "chat-1".to_string(),
                    author_peer_id: "peer-a".to_string(),
                    display_name: Some("Alice".to_string()),
                    body: "welcome".to_string(),
                    created_at: 1_700_000_420,
                }],
            }),
            dome_hosting: None,
            manifest_blob_hash: "f".repeat(64),
            updated_at: 1_700_000_430,
            channel_id: Some("chan-1".to_string()),
            audience_label: "Private channel".to_string(),
        },
    );
}

#[test]
fn views_wire_author_social_view() {
    assert_fixture(
        "author_social_view.json",
        &AuthorSocialView {
            author_pubkey: PUBKEY_B.to_string(),
            name: Some("bob".to_string()),
            display_name: Some("Bob".to_string()),
            about: Some("hi".to_string()),
            picture_asset: Some(profile_asset()),
            updated_at: Some(1_700_000_500),
            following: true,
            followed_by: false,
            mutual: false,
            friend_of_friend: true,
            friend_of_friend_via_pubkeys: vec![PUBKEY_A.to_string()],
            provenance: Some(ContentProvenanceView {
                canonical_source: "author_docs".to_string(),
                observed_via: vec![ContentObservationView {
                    node_base_url: "https://node.example".to_string(),
                    capability: "community_index".to_string(),
                    observed_at: 1_700_000_003_000,
                }],
            }),
            muted: true,
            blocking: true,
            blocked_by: false,
        },
    );
}

#[test]
fn views_wire_channel_access_tokens() {
    assert_fixture(
        "channel_access_token_export.json",
        &ChannelAccessTokenExport {
            kind: ChannelAccessTokenKind::Share,
            token: "token-payload".to_string(),
        },
    );
    assert_fixture(
        "channel_access_token_preview.json",
        &ChannelAccessTokenPreview {
            kind: ChannelAccessTokenKind::Grant,
            topic_id: "kukuri:topic:demo".to_string(),
            channel_id: "chan-1".to_string(),
            channel_label: "secret room".to_string(),
            owner_pubkey: PUBKEY_A.to_string(),
            inviter_pubkey: Some(PUBKEY_B.to_string()),
            sponsor_pubkey: None,
            epoch_id: "epoch-2".to_string(),
        },
    );
}
