use super::schema::{array, nullable, object};
use serde_json::{Value, json};

fn string() -> Value {
    json!({"type": "string"})
}
fn strings() -> Value {
    array(string())
}
fn boolean() -> Value {
    json!({"type": "boolean"})
}
fn integer() -> Value {
    json!({"type": "integer"})
}
fn count() -> Value {
    json!({"type": "integer", "minimum": 0})
}
fn path() -> Value {
    json!({"enum": ["direct_p2p", "relay_supported_p2p", "relay_fallback"]})
}
fn mode() -> Value {
    json!({"enum": ["static_peer", "seeded_dht"]})
}
fn connect_mode() -> Value {
    json!({"enum": ["direct_only", "direct_or_relay"]})
}
fn delivery() -> Value {
    json!({"enum": ["Live", "DurableRecovering", "DurableReady", "Offline"]})
}

fn view(properties: Value) -> Value {
    let required = properties
        .as_object()
        .expect("view properties")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

pub(super) fn input(name: &str) -> Value {
    match name {
        "get_sync_status" | "get_discovery_config" | "get_local_peer_ticket" => {
            object(json!({}), &[])
        }
        "list_connectivity_peers" => object(
            json!({
                "kind": {"enum": ["connected", "configured", "missing", "docs_assist", "blob_assist", "manual_ticket", "bootstrap_seed", "configured_seed"]},
                "topic": nullable(string()), "cursor": nullable(string()),
                "limit": nullable(json!({"type": "integer", "minimum": 1, "maximum": 64}))
            }),
            &["kind"],
        ),
        "import_peer_ticket" => object(json!({"ticket": string()}), &["ticket"]),
        "set_discovery_seeds" => object(json!({"seed_entries": strings()}), &["seed_entries"]),
        "unsubscribe_topic" => object(json!({"topic": string()}), &["topic"]),
        "set_topic_gossip_enabled" => object(
            json!({"topic": string(), "enabled": boolean()}),
            &["topic", "enabled"],
        ),
        "set_channel_gossip_enabled" => object(
            json!({"topic": string(), "channel": string(), "enabled": boolean()}),
            &["topic", "channel", "enabled"],
        ),
        _ => unreachable!("network input schema"),
    }
}

pub(super) fn output(name: &str) -> Value {
    match name {
        "get_sync_status" => sync_status(),
        "list_connectivity_peers" => {
            view(json!({"peer_ids": strings(), "next_cursor": nullable(string())}))
        }
        "get_discovery_config" | "set_discovery_seeds" => view(json!({
            "mode": mode(), "connect_mode": connect_mode(), "env_locked": boolean(),
            "seed_peers": array(view(json!({"endpoint_id": string(), "addr_hint": nullable(string())})))
        })),
        "get_local_peer_ticket" => nullable(string()),
        "import_peer_ticket"
        | "unsubscribe_topic"
        | "set_topic_gossip_enabled"
        | "set_channel_gossip_enabled" => json!({"type": "null"}),
        _ => unreachable!("network output schema"),
    }
}

fn sync_status() -> Value {
    view(json!({
        "connected": boolean(), "delivery_state": delivery(), "last_sync_ts": nullable(integer()),
        "peer_count": count(), "pending_events": count(), "status_detail": string(), "last_error": nullable(string()),
        "configured_peer_count": count(), "subscribed_topics": strings(), "active_path": path(), "fallback_peer_count": count(),
        "local_author_pubkey": string(), "gossip_disabled_topics": strings(), "gossip_disabled_channels": strings(),
        "account_sync": view(json!({
            "no_peers": boolean(), "fetch_failed": boolean(), "behind": boolean(),
            "pending_writes": boolean(), "rebuilding": boolean()
        })),
        "topic_diagnostics": array(view(json!({
            "topic": string(), "joined": boolean(), "delivery_state": delivery(), "peer_count": count(),
            "configured_peer_count": count(), "missing_peer_count": count(),
            "active_path": path(), "rendezvous_peer_count": count(), "fallback_peer_count": count(),
            "last_received_at": nullable(integer()), "last_docs_activity_at": nullable(integer()), "status_detail": string(), "last_error": nullable(string())
        }))),
        "discovery": view(json!({
            "mode": mode(), "connect_mode": connect_mode(), "active_path": path(), "env_locked": boolean(),
            "configured_seed_peer_count": count(), "bootstrap_seed_peer_count": count(), "connected_peer_count": count(),
            "docs_assist_peer_count": count(), "blob_assist_peer_count": count(),
            "local_endpoint_id": string(), "last_discovery_error": nullable(string())
        }))
    }))
}
