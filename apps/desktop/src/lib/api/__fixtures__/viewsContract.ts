// 自動生成ファイル — 手で編集しない。
// 再生成: apps/desktop で `node scripts/generate-views-contract.mjs`
// (Rust 側 fixture の再生成は `UPDATE_FIXTURES=1 cargo test -p kukuri-app-api views_wire`)。
//
// IPC 契約の TS 側検出装置(WP-S4 T2):
// - literal の `satisfies` により、Rust が出力するフィールドが types.ts に無い場合
//   (excess property)や、TS 必須フィールドが fixture に無い場合(missing)に tsc が fail する。
// - viewsContract.test.ts が literal と JSON fixture の同一性を検証する
//   (どちらか片方だけの編集を検出)。
// - fail した場合は types.ts / views.rs の IPC 互換影響を評価すること。
import type {
  AuthorSocialView,
  BookmarkedPostView,
  ChannelAccessTokenExport,
  ChannelAccessTokenPreview,
  DirectMessageConversationView,
  DirectMessageMessageView,
  DirectMessageTimelineView,
  GameRoomView,
  JoinedPrivateChannelView,
  NotificationView,
  PostView,
  SyncStatus,
  TimelineView,
} from '../types';

// post_view.full.json
export const postViewFull = {
  "object_id": "post-1",
  "envelope_id": "envelope-post-1",
  "author_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "author_name": "alice",
  "author_display_name": "Alice",
  "author_picture_asset": {
    "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "mime": "image/png",
    "bytes": 2048,
    "role": "profile_avatar"
  },
  "following": true,
  "followed_by": true,
  "mutual": true,
  "friend_of_friend": false,
  "provenance": {
    "canonical_source": "author_docs",
    "observed_via": [
      {
        "node_base_url": "https://node.example",
        "capability": "community_index",
        "observed_at": 1700000003000
      }
    ]
  },
  "withdrawal": null,
  "content": "hello ipc contract",
  "content_status": "Available",
  "attachments": [
    {
      "hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "mime": "image/png",
      "bytes": 4096,
      "role": "image_original",
      "status": "Available",
      "provenance": {
        "canonical_source": "blob",
        "observed_via": [
          {
            "node_base_url": "https://node.example",
            "capability": "community_index",
            "observed_at": 1700000003000
          }
        ]
      }
    }
  ],
  "content_labels": [
    "adult"
  ],
  "created_at": 1700000000,
  "reply_to": "parent-1",
  "reply_preview": {
    "object_id": "parent-1",
    "topic": "kukuri:topic:demo",
    "author": {
      "pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
      "name": "bob",
      "display_name": "Bob",
      "picture_asset": {
        "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "mime": "image/png",
        "bytes": 2048,
        "role": "profile_avatar"
      }
    },
    "content": "parent content",
    "content_status": "Available",
    "attachments": [
      {
        "hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "mime": "image/png",
        "bytes": 4096,
        "role": "image_original",
        "status": "Available",
        "provenance": {
          "canonical_source": "blob",
          "observed_via": [
            {
              "node_base_url": "https://node.example",
              "capability": "community_index",
              "observed_at": 1700000003000
            }
          ]
        }
      }
    ],
    "content_labels": [],
    "root_id": "root-1",
    "reply_to": null
  },
  "root_id": "root-1",
  "object_kind": "comment",
  "published_topic_id": "kukuri:topic:demo",
  "origin_topic_id": "kukuri:topic:demo",
  "repost_of": {
    "source_object_id": "source-1",
    "source_topic_id": "kukuri:topic:source",
    "source_author_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
    "source_author_name": "bob",
    "source_author_display_name": null,
    "source_author_picture_asset": null,
    "source_object_kind": "post",
    "content": "original",
    "attachments": [],
    "content_labels": [],
    "reply_to": null,
    "root_id": null
  },
  "repost_commentary": "great post",
  "is_threadable": true,
  "channel_id": "chan-1",
  "audience_label": "Private channel",
  "reaction_summary": [
    {
      "reaction_key_kind": "emoji",
      "normalized_reaction_key": "emoji:👍",
      "emoji": "👍",
      "custom_asset": null,
      "count": 3
    }
  ],
  "my_reactions": [
    {
      "reaction_key_kind": "custom",
      "normalized_reaction_key": "custom:reaction-asset-1",
      "emoji": null,
      "custom_asset": {
        "asset_id": "reaction-asset-1",
        "owner_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        "blob_hash": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "search_key": "party-parrot",
        "mime": "image/gif",
        "bytes": 12345,
        "width": 64,
        "height": 64
      }
    }
  ]
} satisfies PostView;

// post_view.minimal.json
export const postViewMinimal = {
  "object_id": "post-2",
  "envelope_id": "envelope-post-2",
  "author_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
  "author_name": null,
  "author_display_name": null,
  "author_picture_asset": null,
  "following": false,
  "followed_by": false,
  "mutual": false,
  "friend_of_friend": false,
  "provenance": null,
  "withdrawal": null,
  "content": "minimal",
  "content_status": "Missing",
  "attachments": [],
  "content_labels": [],
  "created_at": 1700000001,
  "reply_to": null,
  "reply_preview": null,
  "root_id": null,
  "object_kind": "post",
  "published_topic_id": null,
  "origin_topic_id": null,
  "repost_of": null,
  "repost_commentary": null,
  "is_threadable": true,
  "channel_id": null,
  "audience_label": "Public",
  "reaction_summary": [],
  "my_reactions": []
} satisfies PostView;

// timeline_view.json
export const timelineView = {
  "items": [
    {
      "object_id": "post-2",
      "envelope_id": "envelope-post-2",
      "author_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
      "author_name": null,
      "author_display_name": null,
      "author_picture_asset": null,
      "following": false,
      "followed_by": false,
      "mutual": false,
      "friend_of_friend": false,
      "provenance": null,
      "withdrawal": null,
      "content": "minimal",
      "content_status": "Missing",
      "attachments": [],
      "content_labels": [],
      "created_at": 1700000001,
      "reply_to": null,
      "reply_preview": null,
      "root_id": null,
      "object_kind": "post",
      "published_topic_id": null,
      "origin_topic_id": null,
      "repost_of": null,
      "repost_commentary": null,
      "is_threadable": true,
      "channel_id": null,
      "audience_label": "Public",
      "reaction_summary": [],
      "my_reactions": []
    }
  ],
  "next_cursor": {
    "created_at": 1700000001,
    "object_id": "post-2"
  },
  "unavailable_count": 2
} satisfies TimelineView;

// bookmarked_post_view.json
export const bookmarkedPostView = {
  "bookmarked_at": 1700000050,
  "post": {
    "object_id": "post-2",
    "envelope_id": "envelope-post-2",
    "author_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
    "author_name": null,
    "author_display_name": null,
    "author_picture_asset": null,
    "following": false,
    "followed_by": false,
    "mutual": false,
    "friend_of_friend": false,
    "provenance": null,
    "withdrawal": null,
    "content": "minimal",
    "content_status": "Missing",
    "attachments": [],
    "content_labels": [],
    "created_at": 1700000001,
    "reply_to": null,
    "reply_preview": null,
    "root_id": null,
    "object_kind": "post",
    "published_topic_id": null,
    "origin_topic_id": null,
    "repost_of": null,
    "repost_commentary": null,
    "is_threadable": true,
    "channel_id": null,
    "audience_label": "Public",
    "reaction_summary": [],
    "my_reactions": []
  }
} satisfies BookmarkedPostView;

// sync_status.full.json
export const syncStatusFull = {
  "connected": true,
  "delivery_state": "DurableRecovering",
  "last_sync_ts": 1700000100,
  "peer_count": 2,
  "pending_events": 0,
  "status_detail": "durable recovering",
  "last_error": "previous error",
  "configured_peers": [
    "peer-a"
  ],
  "subscribed_topics": [
    "kukuri:topic:demo"
  ],
  "active_path": "relay_supported_p2p",
  "fallback_peer_ids": [],
  "topic_diagnostics": [
    {
      "topic": "kukuri:topic:demo",
      "joined": true,
      "delivery_state": "Live",
      "peer_count": 2,
      "connected_peers": [
        "peer-a",
        "peer-b"
      ],
      "docs_assist_peer_ids": [
        "peer-b"
      ],
      "configured_peer_ids": [
        "peer-a"
      ],
      "missing_peer_ids": [],
      "active_path": "relay_fallback",
      "rendezvous_peer_ids": [],
      "fallback_peer_ids": [
        "peer-a",
        "peer-b"
      ],
      "last_received_at": 1700000090,
      "last_docs_activity_at": 1700000095,
      "status_detail": "live",
      "last_error": null
    }
  ],
  "local_author_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "discovery": {
    "mode": "seeded_dht",
    "connect_mode": "direct_or_relay",
    "active_path": "relay_supported_p2p",
    "fallback_peer_ids": [],
    "env_locked": false,
    "configured_seed_peer_ids": [
      "seed-1"
    ],
    "bootstrap_seed_peer_ids": [
      "seed-2"
    ],
    "manual_ticket_peer_ids": [
      "manual-1"
    ],
    "connected_peer_ids": [
      "peer-a"
    ],
    "docs_assist_peer_ids": [
      "peer-b"
    ],
    "blob_assist_peer_ids": [],
    "local_endpoint_id": "endpoint-1",
    "last_discovery_error": "dht timeout"
  },
  "gossip_disabled_topics": [
    "kukuri:topic:quiet"
  ],
  "gossip_disabled_channels": [
    "chan-quiet"
  ]
} satisfies SyncStatus;

// sync_status.minimal.json
export const syncStatusMinimal = {
  "connected": false,
  "delivery_state": "Offline",
  "last_sync_ts": null,
  "peer_count": 0,
  "pending_events": 0,
  "status_detail": "offline",
  "last_error": null,
  "configured_peers": [],
  "subscribed_topics": [],
  "active_path": "direct_p2p",
  "fallback_peer_ids": [],
  "topic_diagnostics": [],
  "local_author_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "discovery": {
    "mode": "static_peer",
    "connect_mode": "direct_only",
    "active_path": "direct_p2p",
    "fallback_peer_ids": [],
    "env_locked": false,
    "configured_seed_peer_ids": [],
    "bootstrap_seed_peer_ids": [],
    "manual_ticket_peer_ids": [],
    "connected_peer_ids": [],
    "docs_assist_peer_ids": [],
    "blob_assist_peer_ids": [],
    "local_endpoint_id": "",
    "last_discovery_error": null
  },
  "gossip_disabled_topics": [],
  "gossip_disabled_channels": []
} satisfies SyncStatus;

// notification_view.json
export const notificationView = {
  "notification_id": "notification-1",
  "kind": "quote_repost",
  "actor_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
  "actor_name": "bob",
  "actor_display_name": "Bob",
  "actor_picture_asset": {
    "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "mime": "image/png",
    "bytes": 2048,
    "role": "profile_avatar"
  },
  "source_envelope_id": "envelope-1",
  "source_replica_id": "topic::demo",
  "topic_id": "kukuri:topic:demo",
  "channel_id": "chan-1",
  "object_id": "post-1",
  "thread_root_object_id": "root-1",
  "dm_id": null,
  "message_id": null,
  "preview_text": "quoted you",
  "content_labels": [
    "adult"
  ],
  "created_at": 1700000200,
  "received_at": 1700000201,
  "read_at": 1700000210
} satisfies NotificationView;

// joined_private_channel_view.json
export const joinedPrivateChannelView = {
  "topic_id": "kukuri:topic:demo",
  "channel_id": "chan-1",
  "label": "secret room",
  "creator_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "owner_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "joined_via_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
  "audience_kind": "friend_plus",
  "is_owner": false,
  "current_epoch_id": "epoch-2",
  "archived_epoch_ids": [
    "epoch-1"
  ],
  "sharing_state": "frozen",
  "rotation_required": true,
  "participant_count": 4,
  "stale_participant_count": 1,
  "entry_dome_instance_id": "dome-entry"
} satisfies JoinedPrivateChannelView;

// dm_conversation_view.json
export const dmConversationView = {
  "dm_id": "dm-1",
  "peer_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
  "peer_name": "bob",
  "peer_display_name": "Bob",
  "peer_picture_asset": {
    "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "mime": "image/png",
    "bytes": 2048,
    "role": "profile_avatar"
  },
  "updated_at": 1700000310,
  "last_message_at": 1700000300,
  "last_message_id": "msg-1",
  "last_message_preview": "hello dm",
  "status": {
    "peer_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
    "dm_id": "dm-1",
    "mutual": true,
    "send_enabled": true,
    "pending_outbox_count": 2,
    "pending_outbox_has_more": false
  }
} satisfies DirectMessageConversationView;

// dm_message_view.json
export const dmMessageView = {
  "dm_id": "dm-1",
  "message_id": "msg-1",
  "sender_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "recipient_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
  "created_at": 1700000300,
  "text": "hello dm",
  "reply_to_message_id": "msg-0",
  "attachments": [
    {
      "hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "mime": "image/png",
      "bytes": 4096,
      "role": "image_original",
      "status": "Available"
    }
  ],
  "outgoing": true,
  "delivered": false
} satisfies DirectMessageMessageView;

// dm_timeline_view.json
export const dmTimelineView = {
  "items": [
    {
      "dm_id": "dm-1",
      "message_id": "msg-1",
      "sender_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
      "recipient_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
      "created_at": 1700000300,
      "text": "hello dm",
      "reply_to_message_id": "msg-0",
      "attachments": [
        {
          "hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          "mime": "image/png",
          "bytes": 4096,
          "role": "image_original",
          "status": "Available"
        }
      ],
      "outgoing": true,
      "delivered": false
    }
  ],
  "next_cursor": {
    "created_at": 1700000300,
    "object_id": "msg-1"
  }
} satisfies DirectMessageTimelineView;

// game_room_view.score.json
export const gameRoomViewScore = {
  "room_id": "room-1",
  "host_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "title": "score battle",
  "description": "first to 10",
  "status": "Running",
  "phase_label": "round 2",
  "scores": [
    {
      "participant_id": "participant-1",
      "label": "alice",
      "score": 7
    }
  ],
  "room_kind": "score_game",
  "metaverse": null,
  "dome_hosting": null,
  "manifest_blob_hash": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
  "updated_at": 1700000400,
  "channel_id": null,
  "audience_label": "Public"
} satisfies GameRoomView;

// game_room_view.metaverse.json
export const gameRoomViewMetaverse = {
  "room_id": "room-2",
  "host_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "title": "metaverse room",
  "description": "hangout",
  "status": "Waiting",
  "phase_label": null,
  "scores": [],
  "room_kind": "metaverse_room",
  "metaverse": {
    "world_version": 7,
    "instance_id": "room-2",
    "spatial_context": {
      "kind": "channel",
      "topic_id": "kukuri:topic:wire",
      "channel_id": "chan-1"
    },
    "instance_generation": 1,
    "instance_status": "active",
    "relationship_detach": null,
    "replacement_instance_id": null,
    "preset_ref": {
      "preset_id": "preset-2",
      "owner_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
      "revision": 1,
      "manifest_blob_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "manifest_mime": "application/vnd.kukuri.dome-preset+json",
      "manifest_bytes": 2048
    },
    "session_id": "room-2",
    "max_peers": 8,
    "dome": {
      "spec_id": "fixed_dome_v1",
      "customization": {
        "surface": {
          "wall_material": "concrete",
          "floor_material": "stone",
          "wall_texture": null,
          "floor_texture": null
        },
        "environment": {
          "key_light_milli": 2400,
          "ambient_light_milli": 400,
          "fog_density_micros": 8000,
          "gravity_milli": 9800
        },
        "persistent_props": [
          {
            "prop_id": "shared-1",
            "asset_ref": {
              "kind": "glb",
              "blob_hash": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
              "mime_type": "model/gltf-binary",
              "size_bytes": 65536,
              "name": "statue",
              "budget_metadata": {
                "stored_bytes": 65536,
                "texture_width": null,
                "texture_height": null,
                "decoded_texture_bytes": 0,
                "model_triangles": 120,
                "model_primitives": 1
              }
            },
            "primitive_fallback": "cube",
            "position": [
              1,
              2,
              3
            ],
            "rotation": [
              0,
              90,
              0
            ],
            "scale": [
              100,
              100,
              100
            ],
            "visual_only": false,
            "interactions": [
              "grab",
              "push"
            ],
            "collider": null
          }
        ]
      }
    },
    "default_spawn": {
      "position": [
        0,
        0,
        5
      ],
      "rotation": [
        0,
        180,
        0
      ]
    },
    "asset_refs": [],
    "chat_history": [
      {
        "room_id": "room-2",
        "message_id": "chat-1",
        "author_peer_id": "peer-a",
        "display_name": "Alice",
        "body": "welcome",
        "created_at": 1700000420
      }
    ]
  },
  "dome_hosting": null,
  "manifest_blob_hash": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
  "updated_at": 1700000430,
  "channel_id": "chan-1",
  "audience_label": "Private channel"
} satisfies GameRoomView;

// author_social_view.json
export const authorSocialView = {
  "author_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
  "name": "bob",
  "display_name": "Bob",
  "about": "hi",
  "picture_asset": {
    "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "mime": "image/png",
    "bytes": 2048,
    "role": "profile_avatar"
  },
  "updated_at": 1700000500,
  "following": true,
  "followed_by": false,
  "mutual": false,
  "friend_of_friend": true,
  "friend_of_friend_via_pubkeys": [
    "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
  ],
  "provenance": {
    "canonical_source": "author_docs",
    "observed_via": [
      {
        "node_base_url": "https://node.example",
        "capability": "community_index",
        "observed_at": 1700000003000
      }
    ]
  },
  "muted": true,
  "blocking": true,
  "blocked_by": false
} satisfies AuthorSocialView;

// channel_access_token_export.json
export const channelAccessTokenExport = {
  "kind": "share",
  "token": "token-payload"
} satisfies ChannelAccessTokenExport;

// channel_access_token_preview.json
export const channelAccessTokenPreview = {
  "kind": "grant",
  "topic_id": "kukuri:topic:demo",
  "channel_id": "chan-1",
  "channel_label": "secret room",
  "owner_pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "inviter_pubkey": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
  "sponsor_pubkey": null,
  "epoch_id": "epoch-2"
} satisfies ChannelAccessTokenPreview;
