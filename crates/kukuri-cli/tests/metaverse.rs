use std::sync::Arc;

use kukuri_cli::{
    dispatcher::{DispatchReply, Dispatcher},
    protocol::{PROTOCOL_VERSION, RequestEnvelope},
};
use kukuri_desktop_runtime::{ClientHost, DesktopRuntime};
use serde_json::{Value, json};

async fn invoke(
    dispatcher: &Dispatcher,
    host: &Arc<ClientHost>,
    command: &str,
    payload: Value,
) -> Value {
    let request = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: "dome-input".into(),
        command: command.into(),
        profile: "test".into(),
        payload,
        timeout_ms: None,
        secret_bytes: None,
        accepts_secret_output: false,
    };
    let DispatchReply::Unary(response, secret) =
        dispatcher.dispatch(request, None, "test", Some(host)).await
    else {
        panic!("JSON")
    };
    assert!(secret.is_none());
    assert!(response.ok, "{command}: {:?}", response.error);
    response.data.expect("data")
}

#[tokio::test]
async fn dome_creation_hosting_and_connection_lifecycle_use_shared_runtime() {
    let root = tempfile::tempdir().expect("tempdir");
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .expect("runtime"),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .expect("host");
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-dome";
    let room = invoke(
        &dispatcher,
        &host,
        "create_metaverse_room",
        json!({"topic": topic, "title": "Dome", "description": "テスト"}),
    )
    .await;
    let rooms = invoke(
        &dispatcher,
        &host,
        "list_game_rooms",
        json!({"topic": topic}),
    )
    .await;
    assert_eq!(
        rooms.as_array().expect("rooms").len(),
        1,
        "1入力につきDomeを1個作成"
    );
    let customization = rooms[0]["metaverse"]["dome"]["customization"].clone();
    invoke(&dispatcher, &host, "update_metaverse_room", json!({"topic": topic, "room_id": room, "status": "Running", "customization": customization})).await;
    let context = json!({"kind": "topic", "topic_id": topic});
    let hosting = invoke(
        &dispatcher,
        &host,
        "get_dome_hosting",
        json!({"spatial_context": context, "instance_id": room}),
    )
    .await;
    assert_eq!(hosting["state"]["kind"], "closed");
    let topology = invoke(
        &dispatcher,
        &host,
        "list_dome_connection_topology",
        json!({"spatial_context": context}),
    )
    .await;
    assert!(
        topology["connections"]
            .as_array()
            .expect("connections")
            .is_empty()
    );
    let sync = invoke(&dispatcher, &host, "get_sync_status", json!({})).await;
    let active = invoke(
        &dispatcher,
        &host,
        "start_owner_dome_hosting",
        json!({
            "spatial_context": context, "instance_id": room,
            "endpoint_id": sync["discovery"]["local_endpoint_id"], "lease_duration_millis": 60_000
        }),
    )
    .await;
    assert_eq!(active["state"]["kind"], "owner_hosted");
    assert_eq!(active["state"]["lease_epoch"], 1);
    // 参加者がいると呼び出しの間隔しだいで物理が進み、床にめり込んだ既定の prop が押し戻されて
    // layout が変わる。変更なしの commit は join の前(物理が進まない間)に確かめる(#1511)。
    let layout = invoke(
        &dispatcher,
        &host,
        "commit_dome_layout",
        json!({
            "spatial_context": context, "instance_id": room, "operation_id": "layout-1"
        }),
    )
    .await;
    assert_eq!(layout["outcome"], "no_op");
    let snapshot = invoke(&dispatcher, &host, "submit_dome_session_input", json!({
        "spatial_context": context, "instance_id": room, "sequence": 1, "input": {"type": "join"}
    })).await;
    assert_eq!(snapshot["instance_id"], room);
    assert_eq!(
        snapshot["bodies"]
            .as_array()
            .expect("bodies")
            .iter()
            .filter(|body| body["kind"] == "avatar")
            .count(),
        1
    );
    let snapshots = invoke(
        &dispatcher,
        &host,
        "resync_dome_snapshots",
        json!({
            "spatial_context": context, "instance_id": room, "after_sequence": 0
        }),
    )
    .await;
    assert!(!snapshots.as_array().expect("snapshots").is_empty());
    let closed = invoke(
        &dispatcher,
        &host,
        "close_dome_hosting",
        json!({
            "spatial_context": context, "instance_id": room
        }),
    )
    .await;
    assert_eq!(closed["state"]["kind"], "closed");
    let asset_bytes = b"CLI Dome asset fixture";
    let asset_file = root.path().join("dome-asset.bin");
    std::fs::write(&asset_file, asset_bytes).expect("一時asset");
    let asset = invoke(&dispatcher, &host, "import_metaverse_room_asset", json!({
        "topic": topic, "room_id": room, "kind": "other", "file": {
            "path": asset_file, "hash": blake3::hash(asset_bytes).to_hex().to_string(),
            "byte_size": asset_bytes.len(), "mime": "application/octet-stream", "file_name": "asset.bin"
        }
    })).await;
    assert_eq!(
        asset["blob_hash"],
        blake3::hash(asset_bytes).to_hex().to_string()
    );
    let event = invoke(
        &dispatcher,
        &host,
        "publish_metaverse_room_event",
        json!({
            "topic": topic, "room_id": room, "peer_id": "fixture-peer", "seq": 1,
            "event": {"type": "chat_message", "message": {
                "room_id": room, "message_id": "fixture-message", "author_peer_id": "fixture-peer",
                "display_name": null, "body": "Dome内の本文", "created_at": 1
            }}
        }),
    )
    .await;
    let events = invoke(
        &dispatcher,
        &host,
        "list_metaverse_room_events",
        json!({"topic": topic, "room_id": room}),
    )
    .await;
    assert_eq!(events.as_array().expect("events").len(), 1);
    assert_eq!(events[0]["envelope_id"], event["envelope_id"]);
    let moved = invoke(
        &dispatcher,
        &host,
        "move_dome",
        json!({
            "source_topic": topic, "source_instance_id": room, "move_id": "cli-move",
            "target_context": {"kind": "topic", "topic_id": "kukuri:topic:cli-dome-target"}
        }),
    )
    .await;
    assert_eq!(moved["phase"], "completed");
    let target = invoke(
        &dispatcher,
        &host,
        "list_game_rooms",
        json!({"topic": "kukuri:topic:cli-dome-target"}),
    )
    .await;
    assert_eq!(target.as_array().expect("target rooms").len(), 1);
    assert_eq!(target[0]["room_id"], moved["target_instance_id"]);
    dispatcher.finish_operations().await;
    host.shutdown().await;
}
