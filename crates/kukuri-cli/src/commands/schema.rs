use serde_json::{Value, json};

pub(super) fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required,
        "additionalProperties": false})
}

pub(super) fn array(items: Value) -> Value {
    json!({"type": "array", "items": items})
}

pub(super) fn channel_ref() -> Value {
    let mut schema = object(
        json!({"kind": {"enum": ["public", "private_channel"]},
        "channel_id": {"type": "string"}}),
        &["kind"],
    );
    schema["description"] =
        json!("private_channelはchannel_id必須。共有ChannelRef DTOで条件を検証する。");
    schema
}

pub(super) fn timeline_scope() -> Value {
    let mut schema = object(
        json!({"kind": {"enum": ["public", "channel"]},
        "channel_id": {"type": "string"}}),
        &["kind"],
    );
    schema["description"] =
        json!("channelはchannel_id必須。共有TimelineScope DTOで条件を検証する。");
    schema
}

// v1のsubsetには型のunionがない。nullableな共有DTOフィールドは型名を
// annotationで明示し、object/arrayの構造制約を残す。値の生成は共有Rust DTOが担う。
pub(super) fn nullable(mut schema: Value) -> Value {
    let kind = schema
        .as_object_mut()
        .expect("nullable schemaはobjectとして定義する")
        .remove("type")
        .expect("nullable schemaには元のtypeを定義する");
    schema["description"] = json!(format!("null または {}。共有DTOのOptionフィールド。", kind));
    schema
}

pub(super) fn cursor() -> Value {
    object(
        json!({"created_at": {"type": "integer"}, "object_id": {"type": "string"}}),
        &["created_at", "object_id"],
    )
}

pub(super) fn attachment() -> Value {
    object(
        json!({
            "hash": {"type": "string"}, "mime": {"type": "string"},
            "bytes": {"type": "integer", "minimum": 0}, "role": {"type": "string"},
            "status": {"enum": ["Missing", "Available", "Pinned"]},
            "provenance": object(json!({
                "canonical_source": {"type": "string"},
                "observed_via": array(object(json!({
                    "node_base_url": {"type": "string"}, "capability": {"type": "string"},
                    "observed_at": {"type": "integer"}
                }), &["node_base_url", "capability", "observed_at"]))
            }), &["canonical_source", "observed_via"])
        }),
        &["hash", "mime", "bytes", "role", "status"],
    )
}

pub(super) fn profile_asset() -> Value {
    object(
        json!({"hash": {"type": "string"}, "mime": {"type": "string"},
        "bytes": {"type": "integer", "minimum": 0}, "role": {"const": "profile_avatar"}}),
        &["hash", "mime", "bytes", "role"],
    )
}
