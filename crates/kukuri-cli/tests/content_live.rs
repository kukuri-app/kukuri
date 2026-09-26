use std::sync::Arc;

use kukuri_cli::{
    dispatcher::{DispatchReply, Dispatcher},
    protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope},
};
use kukuri_desktop_runtime::{ClientHost, DesktopRuntime};
use serde_json::{Value, json};

async fn call(
    dispatcher: &Dispatcher,
    host: &Arc<ClientHost>,
    command: &str,
    payload: Value,
) -> ResponseEnvelope {
    let request = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: "content-contract".into(),
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
    response
}

#[tokio::test]
async fn public_inputs_create_distinct_posts_once_each_and_reply_uses_domain_runtime() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .unwrap();
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-post-contract";
    let payload = json!({"topic": topic, "content": "public post"});
    let created = call(&dispatcher, &host, "create_post", payload.clone()).await;
    assert!(created.ok, "{:?}", created.error);
    // 既存署名契約のIDは秒単位の時刻と本文から決まる。CLIの入力間排除の
    // 検査をdomainの同一envelope判定と混同しないよう、次の入力は別秒に置く。
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = call(&dispatcher, &host, "create_post", payload).await;
    assert!(second.ok, "{:?}", second.error);
    assert_ne!(created.data, second.data);
    let second_id = second.data.unwrap();
    let post_id = created.data.unwrap();
    let replied = call(
        &dispatcher,
        &host,
        "create_post",
        json!({"topic": topic, "content": "reply", "reply_to": post_id}),
    )
    .await;
    assert!(replied.ok, "{:?}", replied.error);
    let listed = call(&dispatcher, &host, "list_timeline", json!({"topic": topic})).await;
    assert!(listed.ok, "{:?}", listed.error);
    let items = listed.data.unwrap()["items"].as_array().unwrap().clone();
    assert_eq!(
        items
            .iter()
            .filter(|item| item["object_id"] == post_id)
            .count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["object_id"] == second_id)
            .count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["content"] == "public post")
            .count(),
        2
    );
    let thread = call(
        &dispatcher,
        &host,
        "list_thread",
        json!({"topic": topic, "thread_id": post_id}),
    )
    .await;
    assert!(thread.ok, "{:?}", thread.error);
    assert!(
        thread.data.unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| Some(&item["object_id"]) == replied.data.as_ref())
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;
}

#[tokio::test]
async fn live_lifecycle_executes_once_and_preserves_shared_view_fields() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .unwrap();
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-live-contract";
    let payload = json!({"topic": topic, "title": "live", "description": "CLI contract"});
    let created = call(&dispatcher, &host, "create_live_session", payload.clone()).await;
    assert!(created.ok, "{:?}", created.error);
    let session_id = created.data.unwrap();
    let candidates = call(
        &dispatcher,
        &host,
        "list_session_candidates",
        json!({"topic": topic}),
    )
    .await;
    assert!(candidates.ok, "{:?}", candidates.error);
    for visible in [true, false] {
        let displayed = call(
            &dispatcher,
            &host,
            "set_session_display",
            json!({
                "topic":topic, "scope":{"kind":"public"}, "replica_id":"", "session_id":session_id,
                "kind":"live", "observer":"cli-contract", "visible":visible,
            }),
        )
        .await;
        assert!(displayed.ok, "{:?}", displayed.error);
    }
    for command in [
        "join_live_session",
        "leave_live_session",
        "end_live_session",
    ] {
        let result = call(
            &dispatcher,
            &host,
            command,
            json!({"topic": topic, "session_id": session_id}),
        )
        .await;
        assert!(result.ok, "{command}: {:?}", result.error);
    }
    let listed = call(
        &dispatcher,
        &host,
        "list_live_sessions",
        json!({"topic": topic}),
    )
    .await;
    assert!(listed.ok, "{:?}", listed.error);
    let sessions = listed.data.unwrap();
    let sessions = sessions.as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], session_id);
    assert_eq!(sessions[0]["status"], "Ended");
    assert_eq!(sessions[0]["joined_by_me"], false);
    assert!(sessions[0]["ended_at"].is_number());
    dispatcher.finish_operations().await;
    host.shutdown().await;
}

#[tokio::test]
async fn game_lifecycle_rejects_invalid_roster_without_mutation() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .unwrap();
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-game-contract";
    let payload = json!({"topic": topic, "title": "game", "description": "CLI contract",
        "participants": ["Alice", "Bob"]});
    let created = call(&dispatcher, &host, "create_game_room", payload.clone()).await;
    assert!(created.ok, "{:?}", created.error);
    let room_id = created.data.unwrap();
    let listed = call(
        &dispatcher,
        &host,
        "list_game_rooms",
        json!({"topic": topic}),
    )
    .await;
    assert!(listed.ok, "{:?}", listed.error);
    let rooms = listed.data.unwrap();
    assert_eq!(rooms.as_array().unwrap().len(), 1);
    assert_eq!(rooms[0]["room_kind"], "score_game");
    let mut scores = rooms[0]["scores"].clone();
    scores[0]["score"] = json!(7);
    let payload = json!({"topic": topic, "room_id": room_id, "status": "Running",
        "phase_label": "round 1", "scores": scores});
    let updated = call(&dispatcher, &host, "update_game_room", payload.clone()).await;
    assert!(updated.ok, "{:?}", updated.error);
    let before = call(
        &dispatcher,
        &host,
        "list_game_rooms",
        json!({"topic": topic}),
    )
    .await;
    assert!(before.ok, "{:?}", before.error);
    assert_eq!(before.data.as_ref().unwrap()[0]["scores"][0]["score"], 7);
    let rejected = call(
        &dispatcher,
        &host,
        "update_game_room",
        json!({"topic": topic, "room_id": room_id, "status": "Ended", "scores": []}),
    )
    .await;
    assert!(!rejected.ok);
    let after = call(
        &dispatcher,
        &host,
        "list_game_rooms",
        json!({"topic": topic}),
    )
    .await;
    assert!(after.ok, "{:?}", after.error);
    assert_eq!(
        before.data, after.data,
        "拒否された更新でmanifestや投影を変更しない"
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;
}

#[tokio::test]
async fn game_update_by_non_owner_is_rejected_without_changing_replicated_state() {
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();
    let a = Arc::new(
        DesktopRuntime::new(root_a.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let b = Arc::new(
        DesktopRuntime::new(root_b.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host_a = ClientHost::from_runtime(root_a.path().to_path_buf(), a.clone())
        .await
        .unwrap();
    let host_b = ClientHost::from_runtime(root_b.path().to_path_buf(), b.clone())
        .await
        .unwrap();
    for (source, target) in [(&a, &b), (&b, &a)] {
        source
            .import_peer_ticket(kukuri_desktop_runtime::ImportPeerTicketRequest {
                ticket: target.local_peer_ticket().await.unwrap().unwrap(),
            })
            .await
            .unwrap();
    }
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-game-owner-contract";
    // CLI の購読は desired(set_topic_gossip_enabled)で始める。読み込みは購読を始めない(#1221 R2-C)。
    for host in [&host_a, &host_b] {
        let subscribed = call(
            &dispatcher,
            host,
            "set_topic_gossip_enabled",
            json!({"topic": topic, "enabled": true}),
        )
        .await;
        assert!(subscribed.ok, "{:?}", subscribed.error);
    }
    let created = call(&dispatcher, &host_a, "create_game_room",
        json!({"topic": topic, "title": "owner room", "description": "owner contract", "participants": ["Alice", "Bob"]})).await;
    assert!(created.ok, "{:?}", created.error);
    let room_id = created.data.unwrap();
    let before = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let displayed = call(
            &dispatcher,
            &host_b,
            "set_session_display",
            json!({
                "topic":topic, "scope":{"kind":"public"}, "replica_id":"", "session_id":room_id,
                "kind":"game", "observer":"cli-owner-contract", "visible":true,
            }),
        )
        .await;
        assert!(displayed.ok, "{:?}", displayed.error);
        loop {
            let listed = call(
                &dispatcher,
                &host_b,
                "list_game_rooms",
                json!({"topic": topic}),
            )
            .await;
            assert!(listed.ok, "{:?}", listed.error);
            let rooms = listed.data.unwrap();
            if let Some(room) = rooms
                .as_array()
                .unwrap()
                .iter()
                .find(|room| room["room_id"] == room_id)
            {
                break room.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("公開roomが別clientへ伝播する");
    let payload = json!({"topic": topic, "room_id": room_id, "status": "Running", "scores": before["scores"]});
    // 別入力も共有domainが個別に認可判定する。
    for expected in ["authorization_failed"; 2] {
        let rejected = call(&dispatcher, &host_b, "update_game_room", payload.clone()).await;
        assert_eq!(rejected.error.unwrap().code, expected);
    }
    for host in [&host_a, &host_b] {
        let listed = call(
            &dispatcher,
            host,
            "list_game_rooms",
            json!({"topic": topic}),
        )
        .await;
        assert!(listed.ok, "{:?}", listed.error);
        assert_eq!(
            listed.data.unwrap(),
            json!([before]),
            "2回のowner拒否で署名済みstateを変更しない"
        );
    }
    dispatcher.finish_operations().await;
    host_a.shutdown().await;
    host_b.shutdown().await;
}

#[tokio::test]
async fn media_round_trip_uses_explicit_files_without_base64_json_or_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .unwrap();
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-media-contract";
    let bytes = b"cli-media-contract-bytes";
    let input_path = root.path().join("input.png");
    std::fs::write(&input_path, bytes).unwrap();
    let hash = blake3::hash(bytes).to_hex().to_string();
    let payload = json!({"topic": topic, "content": "attachment", "attachments": [{
        "path": input_path, "hash": hash, "byte_size": bytes.len(), "mime": "image/png"
    }]});
    let created = call(&dispatcher, &host, "create_post", payload.clone()).await;
    assert!(created.ok, "{:?}", created.error);
    std::fs::write(&input_path, vec![b'x'; bytes.len()]).unwrap();
    let rejected = call(&dispatcher, &host, "create_post", payload).await;
    assert!(!rejected.ok, "内容が変わったファイルを新たに投稿しない");
    for command in ["get_blob_preview_url", "get_blob_media_payload"] {
        let output_path = root.path().join(command);
        let payload = json!({"hash": hash, "mime": "image/png", "output_path": output_path});
        let result = call(&dispatcher, &host, command, payload.clone()).await;
        assert!(result.ok, "{command}: {:?}", result.error);
        let data = result.data.unwrap();
        assert_eq!(data["hash"], hash);
        assert_eq!(data["byte_size"], bytes.len());
        assert!(data.get("bytes_base64").is_none());
        assert!(data.get("data_base64").is_none());
        assert_eq!(std::fs::read(&output_path).unwrap(), bytes);
        let repeated = call(&dispatcher, &host, command, payload).await;
        assert_eq!(repeated.error.unwrap().code, "conflict");
        assert_eq!(std::fs::read(&output_path).unwrap(), bytes);
    }
    dispatcher.finish_operations().await;
    host.shutdown().await;
}
