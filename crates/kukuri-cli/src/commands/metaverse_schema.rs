use super::{
    game_views, media, metaverse_views,
    schema::{array, nullable, object},
};
use serde_json::{Value, json};

pub(super) fn string() -> Value {
    json!({"type": "string"})
}
pub(super) fn integer() -> Value {
    json!({"type": "integer"})
}
pub(super) fn unsigned() -> Value {
    json!({"type": "integer", "minimum": 0})
}
pub(super) fn vector() -> Value {
    json!({"type": "array", "items": integer(), "minItems": 3, "maxItems": 3})
}
pub(super) fn view(properties: Value) -> Value {
    let required = properties
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    object(properties.clone(), &required)
}
pub(super) fn spatial_context() -> Value {
    object(
        json!({"kind": {"enum": ["topic", "channel"]}, "topic_id": string(), "channel_id": string()}),
        &["kind", "topic_id"],
    )
}
pub(super) fn direction() -> Value {
    json!({"enum": ["north", "east", "south", "west"]})
}
pub(super) fn transition() -> Value {
    view(
        json!({"transition_id": string(), "connection_id": string(), "topology_digest": string(),
        "spatial_context": spatial_context(), "source_instance_id": string(), "source_instance_generation": unsigned(),
        "target_instance_id": string(), "target_instance_generation": unsigned(), "participant_pubkey": string(),
        "direction": direction(), "requested_at": integer()}),
    )
}
pub(super) fn ticket() -> Value {
    view(
        json!({"request": transition(), "target_lease_epoch": unsigned(), "target_session_id": string(), "expires_at": integer()}),
    )
}
fn session_input() -> Value {
    let mut schema = object(
        json!({
            "type": {"enum": ["join", "leave", "keep_alive", "move", "grab", "throw", "push", "sit", "prepare_transition", "abort_transition", "complete_transition", "spawn_guest_prop", "upsert_persistent_prop", "delete_persistent_prop"]},
            "avatar_collider": nullable(game_views::collider()), "position": vector(), "rotation": vector(), "animation": string(),
            "prop_id": string(), "impulse": vector(), "transition_id": string(), "direction": direction(),
            "prop": game_views::prop(), "expires_at": integer()
        }),
        &["type"],
    );
    schema["description"] =
        json!("各typeの必須フィールド・許可フィールドは共有DomeSessionInputKindV1で検証する。");
    schema
}
pub(super) fn event() -> Value {
    object(
        json!({"type": {"enum": ["presence_join", "presence_leave", "chat_message"]},
            "presence": view(json!({"room_id": string(), "peer_id": string(), "display_name": nullable(string()),
                "avatar_asset_ref": nullable(game_views::asset()), "joined_at": integer(), "last_seen_at": integer(),
                "microphone_enabled": {"type": "boolean"}})),
            "room_id": string(), "peer_id": string(), "left_at": integer(),
            "message": view(json!({"room_id": string(), "message_id": string(), "author_peer_id": string(),
                "display_name": nullable(string()), "body": string(), "created_at": integer()}))
        }),
        &["type"],
    )
}

pub(super) fn input(name: &str) -> Value {
    let common = json!({"spatial_context": spatial_context(), "instance_id": string()});
    match name {
        "create_metaverse_room" => object(
            json!({"topic": string(), "channel_ref": super::schema::channel_ref(), "title": string(), "description": string(), "max_peers": nullable(unsigned())}),
            &["topic", "title", "description"],
        ),
        "update_metaverse_room" => view(
            json!({"topic": string(), "room_id": string(), "status": game_views::status(), "customization": game_views::customization()}),
        ),
        "get_dome_hosting" => view(common),
        "close_dome_hosting" => object(
            json!({"spatial_context": spatial_context(), "instance_id": string(), "expected_generation": nullable(unsigned())}),
            &["spatial_context", "instance_id"],
        ),
        "list_pending_dome_deletions" => view(json!({"spatial_context": spatial_context()})),
        "delete_dome" => view(
            json!({"spatial_context": spatial_context(), "instance_id": string(), "expected_generation": unsigned(), "operation_id": string()}),
        ),
        "start_owner_dome_hosting" => object(
            json!({"spatial_context": spatial_context(), "instance_id": string(), "endpoint_id": string(), "lease_duration_millis": integer(), "expected_generation": nullable(unsigned())}),
            &[
                "spatial_context",
                "instance_id",
                "endpoint_id",
                "lease_duration_millis",
            ],
        ),
        "delegate_dome_hosting" => object(
            json!({"spatial_context": spatial_context(), "instance_id": string(), "node_id": string(), "base_url": string(), "lease_duration_millis": integer(), "expected_generation": nullable(unsigned())}),
            &[
                "spatial_context",
                "instance_id",
                "node_id",
                "base_url",
                "lease_duration_millis",
            ],
        ),
        "submit_dome_session_input" => object(
            json!({"spatial_context": spatial_context(), "instance_id": string(), "sequence": unsigned(), "input": session_input(), "expected_generation": nullable(unsigned())}),
            &["spatial_context", "instance_id", "sequence", "input"],
        ),
        "prepare_dome_transition" | "preview_dome_transition_access" => {
            view(json!({"request": transition()}))
        }
        "commit_dome_transition" => {
            view(json!({"ticket": ticket(), "position": vector(), "rotation": vector()}))
        }
        "abort_dome_transition" => view(json!({"ticket": ticket()})),
        "commit_dome_layout" => view(
            json!({"spatial_context": spatial_context(), "instance_id": string(), "operation_id": string()}),
        ),
        "resync_dome_snapshots" => view(
            json!({"spatial_context": spatial_context(), "instance_id": string(), "after_sequence": unsigned()}),
        ),
        "move_dome" => view(
            json!({"source_topic": string(), "move_id": string(), "source_instance_id": string(), "target_context": spatial_context()}),
        ),
        "list_dome_connection_topology" => view(json!({"spatial_context": spatial_context()})),
        "create_dome_connection_proposal" => view(
            json!({"proposal_id": string(), "spatial_context": spatial_context(), "proposer_instance_id": string(), "receiver_instance_id": string(), "proposer_direction": direction()}),
        ),
        "accept_dome_connection_proposal" | "withdraw_dome_connection_proposal" => {
            view(json!({"spatial_context": spatial_context(), "proposal_id": string()}))
        }
        "revoke_dome_connection" => {
            view(json!({"spatial_context": spatial_context(), "connection_id": string()}))
        }
        "publish_metaverse_room_event" => view(
            json!({"topic": string(), "room_id": string(), "peer_id": string(), "seq": unsigned(), "event": event()}),
        ),
        "list_metaverse_room_events" => object(
            json!({"topic": string(), "room_id": string(), "after_envelope_id": nullable(string()), "limit": nullable(unsigned())}),
            &["topic", "room_id"],
        ),
        "import_metaverse_room_asset" => view(
            json!({"topic": string(), "room_id": string(), "kind": {"enum": ["vrm", "glb", "texture", "other"]}, "file": media::input_schema()}),
        ),
        _ => unreachable!("登録済みMetaverse command"),
    }
}

pub(super) fn output(name: &str) -> Value {
    match name {
        "create_metaverse_room" => string(),
        "list_pending_dome_deletions" => array(view(
            json!({"request": input("delete_dome"), "title": string(), "deleted": {"type":"boolean"}}),
        )),
        "delete_dome" => view(
            json!({"instance_id": string(), "generation": unsigned(), "deleted": {"type":"boolean"}, "cleanup_pending": {"type":"boolean"}}),
        ),
        "update_metaverse_room" | "commit_dome_transition" | "abort_dome_transition" => {
            json!({"type": "null"})
        }
        "get_dome_hosting"
        | "start_owner_dome_hosting"
        | "delegate_dome_hosting"
        | "close_dome_hosting" => metaverse_views::hosting(),
        "submit_dome_session_input" => metaverse_views::snapshot(),
        "prepare_dome_transition" => ticket(),
        "preview_dome_transition_access" => object(
            json!({"status": {"enum": ["allowed", "denied"]},
            "reason": {"enum": ["host_unavailable", "access_denied", "owners_blocked", "visitor_blocked", "capacity_full", "assets_unavailable", "stale_topology", "stale_session", "invalid_ticket"]}}),
            &["status"],
        ),
        "commit_dome_layout" => view(
            json!({"outcome": {"enum": ["no_op", "committed"]}, "operation_id": string(), "revision": unsigned(), "manifest_blob_hash": string(), "signed_commit_json": nullable(string()), "hosting": metaverse_views::hosting()}),
        ),
        "resync_dome_snapshots" => array(metaverse_views::snapshot()),
        "move_dome" => metaverse_views::movement(),
        "list_dome_connection_topology" => super::dome_connection_views::topology(),
        "create_dome_connection_proposal" | "withdraw_dome_connection_proposal" => {
            super::dome_connection_views::proposal()
        }
        "accept_dome_connection_proposal" | "revoke_dome_connection" => {
            super::dome_connection_views::connection()
        }
        "publish_metaverse_room_event" => metaverse_views::room_event(),
        "list_metaverse_room_events" => array(metaverse_views::room_event()),
        "import_metaverse_room_asset" => game_views::asset(),
        _ => unreachable!("登録済みMetaverse command"),
    }
}
