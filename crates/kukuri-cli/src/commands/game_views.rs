use serde_json::{Value, json};

use super::schema::{array, nullable, object};

fn view(properties: Value) -> Value {
    let required = properties
        .as_object()
        .expect("viewのpropertiesはobjectとして定義する")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    object(properties.clone(), &required)
}

fn string() -> Value {
    json!({"type": "string"})
}
fn integer() -> Value {
    json!({"type": "integer"})
}
fn unsigned() -> Value {
    json!({"type": "integer", "minimum": 0})
}
fn vector() -> Value {
    json!({"type": "array", "items": integer(), "minItems": 3, "maxItems": 3})
}

/// #1980: アセットの形式。`asset_bundle` のときだけ Unity の版と build の対象を持つ(core の検証が確かめる)。
pub(super) fn asset_format() -> Value {
    object(
        json!({"kind": {"enum": ["glb", "png", "jpeg", "asset_bundle"]},
            "unity_version": string(), "build_target": string()}),
        &["kind"],
    )
}

fn asset_release_properties() -> Value {
    json!({"publisher_pubkey": string(), "application_id": string(), "asset_id": string(),
        "revision": unsigned(), "bytes": unsigned(), "format": asset_format(), "published_at": integer(),
        "applied_revision": nullable(unsigned())})
}

pub(super) fn asset_release() -> Value {
    view(asset_release_properties())
}

pub(super) fn asset_file() -> Value {
    let mut properties = asset_release_properties();
    properties["path"] = string();
    view(properties)
}

pub(super) fn score() -> Value {
    view(json!({"participant_id": string(), "label": string(), "score": integer()}))
}

pub(super) fn status() -> Value {
    json!({"enum": ["Waiting", "Running", "Paused", "Ended"]})
}

pub(super) fn game_room() -> Value {
    view(json!({
        "room_id": string(), "host_pubkey": string(), "title": string(), "description": string(),
        "status": status(), "phase_label": nullable(string()), "scores": array(score()),
        "room_kind": {"enum": ["score_game", "metaverse_room", "action_session"]},
        "metaverse": nullable(metaverse_state()), "dome_hosting": nullable(hosting_state()),
        "action": nullable(view(json!({"application_id": string(), "host_endpoint_id": string(),
            "max_players": unsigned()}))),
        "manifest_blob_hash": string(), "updated_at": integer(), "channel_id": nullable(string()),
        "audience_label": string()
    }))
}

pub(super) fn hosting_state() -> Value {
    let host = object(
        json!({"kind": {"enum": ["owner_device", "community_node"]},
        "endpoint_id": string(), "host_pubkey": string(), "node_id": string(), "api_base_url": string()}),
        &["kind"],
    );
    view(
        json!({"kind": {"enum": ["closed", "owner_hosted", "community_node_hosted", "grace_period", "transferring"]},
        "host": nullable(host), "lease_id": nullable(string()), "lease_epoch": nullable(unsigned()),
        "lease_expires_at": nullable(integer()), "session_id": nullable(string()), "reason": nullable(string()),
        "last_heartbeat_at": nullable(integer())}),
    )
}

fn metaverse_state() -> Value {
    view(json!({
        "world_version": unsigned(), "instance_id": string(),
        "spatial_context": object(json!({"kind": {"enum": ["topic", "channel"]},
            "topic_id": string(), "channel_id": string()}), &["kind", "topic_id"]),
        "instance_generation": unsigned(), "instance_status": {"enum": ["staging", "active", "tombstoned"]},
        "relationship_detach": nullable(view(json!({"move_id": string(), "instance_generation": unsigned(), "detached_at": integer()}))),
        "replacement_instance_id": nullable(string()),
        "preset_ref": view(json!({"preset_id": string(), "owner_pubkey": string(), "revision": unsigned(),
            "manifest_blob_hash": string(), "manifest_mime": string(), "manifest_bytes": unsigned()})),
        "session_id": string(), "max_peers": nullable(unsigned()), "dome": dome(),
        "default_spawn": view(json!({"position": vector(), "rotation": vector()})),
        "asset_refs": array(asset()),
        "chat_history": array(view(json!({"room_id": string(), "message_id": string(), "author_peer_id": string(),
            "display_name": nullable(string()), "body": string(), "created_at": integer()})))
    }))
}

pub(super) fn asset() -> Value {
    view(
        json!({"kind": {"enum": ["vrm", "glb", "texture", "other"]}, "blob_hash": string(),
        "mime_type": nullable(string()), "size_bytes": nullable(unsigned()), "name": nullable(string()),
        "budget_metadata": nullable(view(json!({"stored_bytes": unsigned(), "texture_width": nullable(unsigned()),
            "texture_height": nullable(unsigned()), "decoded_texture_bytes": unsigned(),
            "model_triangles": unsigned(), "model_primitives": unsigned()})))}),
    )
}

fn dome() -> Value {
    view(json!({"spec_id": string(), "customization": customization()}))
}

pub(super) fn collider() -> Value {
    object(
        json!({"shape": {"enum": ["capsule", "cuboid"]}, "center": vector(),
        "radius": integer(), "half_height": integer(), "half_extents": vector()}),
        &["shape", "center"],
    )
}

pub(super) fn prop() -> Value {
    view(json!({"prop_id": string(), "asset_ref": nullable(asset()),
        "primitive_fallback": {"enum": ["cube", "sphere"]}, "position": vector(), "rotation": vector(), "scale": vector(),
        "visual_only": {"type": "boolean"}, "interactions": array(json!({"enum": ["grab", "throw", "push", "sit"]})),
        "collider": nullable(collider())}))
}

pub(super) fn customization() -> Value {
    let material = json!({"enum": ["concrete", "stone", "metal", "wood"]});
    view(json!({
        "surface": view(json!({"wall_material": material, "floor_material": material,
            "wall_texture": nullable(asset()), "floor_texture": nullable(asset())})),
        "environment": view(json!({"key_light_milli": unsigned(), "ambient_light_milli": unsigned(),
            "fog_density_micros": unsigned(), "gravity_milli": unsigned()})),
        "persistent_props": array(prop())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dispatcher::{DispatchReply, Dispatcher},
        protocol::{CommandEffect, PROTOCOL_VERSION, ProtocolError, RequestEnvelope, SecretInput},
        registry::{CommandHandler, CommandOutput, CommandRegistry, HandlerContext},
    };
    use std::sync::Arc;

    struct Fixture(Value);

    #[async_trait::async_trait]
    impl CommandHandler for Fixture {
        async fn execute(
            &self,
            _: HandlerContext<'_>,
            _: Value,
            _: Option<&SecretInput>,
        ) -> Result<CommandOutput, ProtocolError> {
            Ok(CommandOutput::Unary(self.0.clone()))
        }
    }

    #[tokio::test]
    async fn game_schema_preserves_existing_score_and_metaverse_wire_fixtures() {
        for fixture in [
            include_str!(
                "../../../../apps/desktop/src/lib/api/__fixtures__/views/game_room_view.score.json"
            ),
            include_str!(
                "../../../../apps/desktop/src/lib/api/__fixtures__/views/game_room_view.metaverse.json"
            ),
        ] {
            let value: Value = serde_json::from_str(fixture).unwrap();
            let registration = super::super::command(
                "fixture",
                CommandEffect::Read,
                false,
                false,
                vec![],
                (object(json!({}), &[]), game_room()),
                Arc::new(Fixture(value)),
            );
            let dispatcher = Dispatcher::new(CommandRegistry::new(vec![registration]).unwrap());
            let request = RequestEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id: "fixture".into(),
                command: "fixture".into(),
                profile: "fixture".into(),
                payload: json!({}),
                timeout_ms: None,
                secret_bytes: None,
                accepts_secret_output: false,
            };
            let DispatchReply::Unary(response, _) =
                dispatcher.dispatch(request, None, "fixture", None).await
            else {
                panic!("JSON")
            };
            assert!(response.ok, "{:?}", response.error);
        }
    }
}
