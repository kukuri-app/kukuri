#![cfg(target_os = "linux")]

#[path = "support/community_node_mock.rs"]
mod community_node_mock;
#[path = "support/process_client.rs"]
mod process_client;
use process_client::ProcessClient;
use serde_json::json;

// #1262: CLIのconsumerも、表示する対象を明示し、daemon停止で購読を解除する。
fn display_session(client: &ProcessClient, topic: &str, id: &serde_json::Value, kind: &str) {
    client.call(
        "set_session_display",
        json!({"topic":topic, "scope":{"kind":"public"},
        "replica_id":"", "session_id":id, "kind":kind,
        "observer":format!("e2e-{}", id.as_str().expect("session id")), "visible":true}),
    );
}

fn refresh_dm_pair(
    a: &ProcessClient,
    b: &ProcessClient,
    a_pubkey: &serde_json::Value,
    b_pubkey: &serde_json::Value,
) {
    let ticket_a = a.call("get_local_peer_ticket", json!({}));
    let ticket_b = b.call("get_local_peer_ticket", json!({}));
    a.call("import_peer_ticket", json!({"ticket": ticket_b}));
    b.call("import_peer_ticket", json!({"ticket": ticket_a}));
    a.call("open_direct_message", json!({"pubkey": b_pubkey}));
    b.call("open_direct_message", json!({"pubkey": a_pubkey}));
}

fn wait_for_dm(
    a: &ProcessClient,
    b: &ProcessClient,
    a_pubkey: &serde_json::Value,
    b_pubkey: &serde_json::Value,
    message_id: &serde_json::Value,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut next_refresh = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        // 既存harnessのpair refreshと同じく、配送を担う送信側の状態も更新する。
        // send_direct_messageは呼び直さず、元のmessage_idの配送だけを待つ。
        let sender_status = a.call("get_direct_message_status", json!({"pubkey": b_pubkey}));
        let data = b.call("list_direct_message_messages", json!({"pubkey": a_pubkey}));
        if data["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["message_id"] == *message_id))
        {
            return data;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "DM配送期限: sender={sender_status}"
        );
        if std::time::Instant::now() >= next_refresh {
            refresh_dm_pair(a, b, a_pubkey, b_pubkey);
            next_refresh = std::time::Instant::now() + std::time::Duration::from_secs(5);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn connect_dm_pair(
    a: &ProcessClient,
    b: &ProcessClient,
    a_pubkey: &serde_json::Value,
    b_pubkey: &serde_json::Value,
) {
    refresh_dm_pair(a, b, a_pubkey, b_pubkey);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut next_refresh = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let left = a.call("get_direct_message_status", json!({"pubkey": b_pubkey}));
        let right = b.call("get_direct_message_status", json!({"pubkey": a_pubkey}));
        if left["send_enabled"] == true && right["send_enabled"] == true {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "DM接続未完了: A={left}, B={right}"
        );
        if std::time::Instant::now() >= next_refresh {
            refresh_dm_pair(a, b, a_pubkey, b_pubkey);
            next_refresh = std::time::Instant::now() + std::time::Duration::from_secs(5);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[test]
fn real_cli_backup_cancel_bypasses_the_mutation_queue_and_restores_runtime() {
    let mut client = ProcessClient::start();
    client.consent();
    let topic = "kukuri:topic:cli-backup-cancel";
    let bytes = vec![42_u8; 16 * 1024 * 1024];
    let source = client.path("large-attachment.bin");
    std::fs::write(&source, &bytes).expect("一時添付fixture");
    let post = client.call("create_post", json!({"topic": topic, "content": "取消後も残す投稿", "attachments": [{
        "path": source, "hash": blake3::hash(&bytes).to_hex().to_string(), "byte_size": bytes.len(), "mime": "application/octet-stream"
    }]}));
    let destination = client.path("cancelled.kukuri");
    let passphrase = client.secret_file(b"backup-cancel-passphrase-sentinel");
    let operation = client.begin_call(
        "create_device_backup_command",
        json!({"path": destination}),
        Some(&passphrase),
        None,
    );
    client.wait_for("get_desktop_startup_status", json!({}), |status| {
        status["status"] == "initializing"
    });
    client.call("cancel_device_backup", json!({}));
    let result = operation.finish();
    assert_eq!(result["ok"], false, "取消は実行中backupへ到達する");
    assert!(!destination.exists(), "部分archiveを残さない");
    assert_eq!(
        client.call("get_desktop_startup_status", json!({}))["status"],
        "ready"
    );
    assert_eq!(
        client.call("list_timeline", json!({"topic": topic}))["items"][0]["object_id"],
        post
    );
    client.stop();
}

#[test]
fn real_cli_node_consent_is_per_node_and_survives_restart_without_auto_acceptance() {
    use community_node_mock::{MockNode, TOKEN, mock_node};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let runtime = tokio::runtime::Runtime::new().expect("HTTP runtime");
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("node A");
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("未同意node B");
    silent.set_nonblocking(true).expect("非blocking");
    let silent_url = format!("http://{}", silent.local_addr().expect("node B address"));
    let node = Arc::new(MockNode {
        base_url: format!("http://{}", listener.local_addr().expect("node A address")),
        policy_version: AtomicUsize::new(1),
        reject_index_once: AtomicBool::new(true),
        index_hits: AtomicUsize::new(0),
        verify_hits: AtomicUsize::new(0),
    });
    let router = axum::Router::new()
        .fallback(mock_node)
        .with_state(node.clone());
    let server = runtime.spawn(async move {
        axum::serve(listener, router).await.expect("node HTTP");
    });
    let mut client = ProcessClient::start();
    client.consent();
    client.call(
        "set_community_node_config",
        json!({"nodes": [{"base_url": node.base_url}, {"base_url": silent_url}]}),
    );
    client.call(
        "fetch_community_node_policies",
        json!({"base_url": node.base_url, "language": "ja"}),
    );
    let accepted = client.call("accept_community_node_consents", json!({"base_url": node.base_url, "language": "ja", "documents": [{"policy_slug": "builder-preview", "policy_version": 1}]}));
    assert_eq!(accepted["auth_state"]["authenticated"], true);
    assert!(!accepted.to_string().contains(TOKEN));
    let statuses = client.call("get_community_node_statuses", json!({}));
    let other = statuses
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["base_url"] == silent_url)
        .expect("node B");
    assert_eq!(other["auth_state"]["authenticated"], false);
    assert!(
        other["local_consent"]["records"]
            .as_array()
            .expect("records")
            .is_empty()
    );
    client.call(
        "search_community_node_index",
        json!({"base_url": node.base_url, "query": "検索本文"}),
    );
    assert_eq!(
        node.index_hits.load(Ordering::SeqCst),
        2,
        "domain内部の401再認証だけを実行"
    );
    node.policy_version.store(2, Ordering::SeqCst);
    let verifies = node.verify_hits.load(Ordering::SeqCst);
    client.restart();
    let rejected = client.raw_call(
        "search_community_node_index",
        json!({"base_url": node.base_url, "query": "失効後の本文"}),
        None,
        None,
    );
    assert_eq!(rejected["error"]["code"], "consent_required");
    assert_eq!(node.index_hits.load(Ordering::SeqCst), 2);
    assert_eq!(node.verify_hits.load(Ordering::SeqCst), verifies);
    assert!(
        matches!(silent.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "未同意nodeのHTTPは0件"
    );
    client.stop();
    server.abort();
    let _ = runtime.block_on(server);
}

#[test]
fn three_real_daemons_exchange_content_and_preserve_private_boundaries() {
    let mut a = ProcessClient::start();
    let mut b = ProcessClient::start();
    let mut outsider = ProcessClient::start();
    for client in [&a, &b, &outsider] {
        client.consent();
    }
    let clients = [&a, &b, &outsider];
    let tickets = clients
        .iter()
        .map(|client| client.call("get_local_peer_ticket", json!({})))
        .collect::<Vec<_>>();
    for (index, client) in clients.iter().enumerate() {
        for (target, ticket) in tickets.iter().enumerate() {
            if index != target {
                client.call("import_peer_ticket", json!({"ticket": ticket}));
            }
        }
    }
    let topic = "kukuri:topic:cli-three-processes";
    for client in clients {
        client.call(
            "set_topic_gossip_enabled",
            json!({"topic": topic, "enabled": true}),
        );
    }
    let post = a.call(
        "create_post",
        json!({"topic": topic, "content": "公開の本文"}),
    );
    // 既存の秒単位署名ID契約を変えず、別秒の同内容入力を検証する。
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let other = a.call(
        "create_post",
        json!({"topic": topic, "content": "公開の本文"}),
    );
    assert_ne!(post, other, "明示した2入力は別投稿");
    b.wait_for("list_timeline", json!({"topic": topic}), |data| {
        data["items"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
    });
    let reply = b.call(
        "create_post",
        json!({"topic": topic, "content": "返信の本文", "reply_to": post}),
    );
    a.wait_for("list_timeline", json!({"topic": topic}), |data| {
        data["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["object_id"] == reply))
    });

    let a_pubkey = a.call("get_sync_status", json!({}))["local_author_pubkey"].clone();
    let b_pubkey = b.call("get_sync_status", json!({}))["local_author_pubkey"].clone();
    a.call("follow_author", json!({"pubkey": b_pubkey}));
    b.call("follow_author", json!({"pubkey": a_pubkey}));
    a.wait_for(
        "get_author_social_view",
        json!({"pubkey": b_pubkey}),
        |data| data["mutual"] == true,
    );
    b.wait_for(
        "get_author_social_view",
        json!({"pubkey": a_pubkey}),
        |data| data["mutual"] == true,
    );
    connect_dm_pair(&a, &b, &a_pubkey, &b_pubkey);
    let dm = a.call(
        "send_direct_message",
        json!({"pubkey": b_pubkey, "text": "参加者だけのDM本文"}),
    );
    let messages = wait_for_dm(&a, &b, &a_pubkey, &b_pubkey, &dm);
    assert_eq!(messages["items"][0]["text"], "参加者だけのDM本文");
    assert!(
        !outsider
            .call("list_timeline", json!({"topic": topic}))
            .to_string()
            .contains("DM本文")
    );
    let denied_dm = outsider.raw_call(
        "send_direct_message",
        json!({"pubkey": a_pubkey, "text": "未承認"}),
        None,
        None,
    );
    assert_eq!(denied_dm["error"]["code"], "authorization_failed");

    let channel = a.call(
        "create_private_channel",
        json!({"topic": topic, "label": "秘密channel"}),
    )["channel_id"]
        .clone();
    let invite_file = a.path("channel.invite");
    assert_eq!(
        a.raw_call(
            "export_private_channel_invite",
            json!({"topic": topic, "channel_id": channel}),
            None,
            Some(&invite_file)
        )["ok"],
        true
    );
    // remote replicaは非同期に配布される。明確な同期未完了に限り、テストの
    // 呼出元が別の明示入力を行う。CLI／daemon内には要求の再送処理を置かない。
    let import_deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let imported = loop {
        let response = b.raw_call(
            "import_private_channel_invite",
            json!({}),
            Some(&invite_file),
            None,
        );
        if response["ok"] == true || response["error"]["code"] != "network_unavailable" {
            break response;
        }
        assert!(
            std::time::Instant::now() < import_deadline,
            "private replica同期未完了: {}",
            response["error"]
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert_eq!(imported["ok"], true, "{}", imported["error"]);
    assert!(imported["data"].get("namespace_secret_hex").is_none());
    let private_post = a.call("create_post", json!({"topic": topic, "content": "招待者だけの投稿", "channel_ref": {"kind": "private_channel", "channel_id": channel}}));
    b.wait_for(
        "list_timeline",
        json!({"topic": topic, "scope": {"kind": "channel", "channel_id": channel}}),
        |data| {
            data["items"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["object_id"] == private_post))
        },
    );
    assert!(
        outsider
            .call("list_joined_private_channels", json!({"topic": topic}))
            .as_array()
            .expect("channels")
            .is_empty()
    );
    assert!(
        !outsider
            .call("list_timeline", json!({"topic": topic}))
            .to_string()
            .contains("招待者だけ")
    );
    let denied = outsider.raw_call(
        "list_timeline",
        json!({"topic": topic, "scope": {"kind": "channel", "channel_id": channel}}),
        None,
        None,
    );
    assert_ne!(
        denied["ok"], true,
        "未招待clientはprivate replicaを読めない"
    );
    let rotated = a.call(
        "rotate_private_channel",
        json!({"topic": topic, "channel_id": channel}),
    );
    // 既存のepoch handoffは参加中の相手へ新しい世代を配布する。
    b.wait_for(
        "list_joined_private_channels",
        json!({"topic": topic}),
        |data| data[0]["current_epoch_id"] == rotated["current_epoch_id"],
    );
    let renewed_post = a.call("create_post", json!({"topic": topic, "content": "鍵更新後の投稿", "channel_ref": {"kind": "private_channel", "channel_id": channel}}));
    b.wait_for(
        "list_timeline",
        json!({"topic": topic, "scope": {"kind": "channel", "channel_id": channel}}),
        |data| {
            data["items"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["object_id"] == renewed_post))
        },
    );
    b.call(
        "leave_private_channel",
        json!({"topic": topic, "channel_id": channel}),
    );
    assert!(
        b.call("list_joined_private_channels", json!({"topic": topic}))
            .as_array()
            .expect("退出後channels")
            .is_empty()
    );

    let live = a.call(
        "create_live_session",
        json!({"topic": topic, "title": "配信", "description": "E2E"}),
    );
    display_session(&b, topic, &live, "live");
    b.wait_for("list_live_sessions", json!({"topic": topic}), |data| {
        data.as_array()
            .is_some_and(|items| items.iter().any(|item| item["session_id"] == live))
    });
    b.call(
        "join_live_session",
        json!({"topic": topic, "session_id": live}),
    );
    b.call(
        "leave_live_session",
        json!({"topic": topic, "session_id": live}),
    );
    a.call(
        "end_live_session",
        json!({"topic": topic, "session_id": live}),
    );
    let game = a.call(
        "create_game_room",
        json!({"topic": topic, "title": "Game", "description": "E2E", "participants": ["A", "B"]}),
    );
    display_session(&b, topic, &game, "game");
    let rooms = b.wait_for("list_game_rooms", json!({"topic": topic}), |data| {
        data.as_array()
            .is_some_and(|items| items.iter().any(|item| item["room_id"] == game))
    });
    let scores = rooms
        .as_array()
        .expect("rooms")
        .iter()
        .find(|room| room["room_id"] == game)
        .expect("game")["scores"]
        .clone();
    assert_eq!(
        b.raw_call(
            "update_game_room",
            json!({"topic": topic, "room_id": game, "status": "Ended", "scores": scores}),
            None,
            None
        )["error"]["code"],
        "authorization_failed"
    );
    a.call(
        "update_game_room",
        json!({"topic": topic, "room_id": game, "status": "Ended", "scores": scores}),
    );

    let dome = a.call(
        "create_metaverse_room",
        json!({"topic": topic, "title": "Dome", "description": "E2E"}),
    );
    display_session(&b, topic, &dome, "game");
    b.wait_for("list_game_rooms", json!({"topic": topic}), |data| {
        data.as_array()
            .is_some_and(|items| items.iter().any(|item| item["room_id"] == dome))
    });
    let context = json!({"kind": "topic", "topic_id": topic});
    let receiver_dome = b.call(
        "create_metaverse_room",
        json!({"topic": topic, "title": "受信側Dome", "description": "接続検証"}),
    );
    display_session(&a, topic, &receiver_dome, "game");
    a.wait_for("list_game_rooms", json!({"topic": topic}), |data| {
        data.as_array()
            .is_some_and(|items| items.iter().any(|item| item["room_id"] == receiver_dome))
    });
    let proposal = a.call("create_dome_connection_proposal", json!({"proposal_id": "cli-connection", "spatial_context": context, "proposer_instance_id": dome, "receiver_instance_id": receiver_dome, "proposer_direction": "east"}));
    assert_eq!(proposal["status"], "proposed");
    b.wait_for(
        "list_dome_connection_topology",
        json!({"spatial_context": context}),
        |data| {
            data["proposals"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["proposal"]["proposal_id"] == "cli-connection")
            })
        },
    );
    let connection = b.call(
        "accept_dome_connection_proposal",
        json!({"spatial_context": context, "proposal_id": "cli-connection"}),
    );
    assert_eq!(connection["record"]["status"], "active");
    let connection_id = connection["record"]["agreement"]["connection_id"].clone();
    a.wait_for(
        "list_dome_connection_topology",
        json!({"spatial_context": context}),
        |data| {
            data["resolution"]["topology"]["active_connection_ids"]
                .as_array()
                .is_some_and(|ids| ids.contains(&connection_id))
        },
    );
    let endpoint = a.call("get_sync_status", json!({}))["discovery"]["local_endpoint_id"].clone();
    assert_eq!(a.call("start_owner_dome_hosting", json!({"spatial_context": context, "instance_id": dome, "endpoint_id": endpoint, "lease_duration_millis": 60_000}))["state"]["kind"], "owner_hosted");
    a.call("submit_dome_session_input", json!({"spatial_context": context, "instance_id": dome, "sequence": 1, "input": {"type": "join"}}));
    a.call(
        "close_dome_hosting",
        json!({"spatial_context": context, "instance_id": dome}),
    );
    let revoked = b.call(
        "revoke_dome_connection",
        json!({"spatial_context": context, "connection_id": connection_id}),
    );
    assert_eq!(revoked["record"]["status"], "revoked");
    a.wait_for(
        "list_dome_connection_topology",
        json!({"spatial_context": context}),
        |data| {
            data["resolution"]["topology"]["active_connection_ids"]
                .as_array()
                .is_some_and(|ids| !ids.contains(&connection_id))
        },
    );

    b.restart();
    assert_eq!(
        b.call("get_sync_status", json!({}))["local_author_pubkey"],
        b_pubkey
    );
    assert!(
        b.call("get_sync_status", json!({}))["subscribed_topics"]
            .as_array()
            .expect("topics")
            .contains(&json!(topic))
    );
    assert_eq!(
        b.call("list_direct_message_messages", json!({"pubkey": a_pubkey}))["items"][0]["message_id"],
        dm
    );
    b.stop();
    let offline_dm = a.call(
        "send_direct_message",
        json!({"pubkey": b_pubkey, "text": "停止中の相手へのDM"}),
    );
    b.restart();
    connect_dm_pair(&a, &b, &a_pubkey, &b_pubkey);
    wait_for_dm(&a, &b, &a_pubkey, &b_pubkey, &offline_dm);
    b.call(
        "delete_direct_message_message",
        json!({"pubkey": a_pubkey, "message_id": dm}),
    );
    assert!(
        !b.call("list_direct_message_messages", json!({"pubkey": a_pubkey}))["items"]
            .as_array()
            .expect("削除後DM")
            .iter()
            .any(|item| item["message_id"] == dm)
    );
    assert!(
        a.call("list_direct_message_messages", json!({"pubkey": b_pubkey}))["items"]
            .as_array()
            .expect("送信側DM")
            .iter()
            .any(|item| item["message_id"] == dm),
        "相手のローカル削除で送信側を消さない"
    );
    a.stop();
    b.stop();
    outsider.stop();
}

#[test]
fn real_cli_consent_key_backup_restore_and_restart() {
    let mut client = ProcessClient::start();
    assert_eq!(
        client.call("get_desktop_startup_status", json!({}))["status"],
        "consent_required"
    );
    let denied = client.raw_call(
        "create_post",
        json!({"topic": "test", "content": "同意前"}),
        None,
        None,
    );
    assert_eq!(denied["error"]["code"], "consent_required");
    client.consent();
    let accounts = client.call("list_accounts", json!({}));
    let passphrase = client.secret_file(b"cli-backup-passphrase-sentinel");
    let exported = client.path("account.export");
    let key = client.raw_call(
        "export_account_key",
        json!({}),
        Some(&passphrase),
        Some(&exported),
    );
    assert_eq!(key["ok"], true, "{}", key["error"]);
    assert!(!key.to_string().contains("sentinel"));
    let preview = client.raw_call(
        "preview_account_key_import",
        json!({}),
        Some(&exported),
        None,
    );
    assert_eq!(preview["ok"], true, "{}", preview["error"]);
    assert_eq!(preview["data"]["already_registered"], true);
    let topic = "kukuri:topic:cli-process-backup";
    let first = client.call(
        "create_post",
        json!({"topic": topic, "content": "バックアップ前"}),
    );
    let backup = client.path("device.kukuri");
    let created = client.raw_call(
        "create_device_backup_command",
        json!({"path": backup}),
        Some(&passphrase),
        None,
    );
    assert_eq!(created["ok"], true, "{}", created["error"]);
    let preview = client.raw_call(
        "preview_device_backup_command",
        json!({"path": backup}),
        Some(&passphrase),
        None,
    );
    assert_eq!(preview["ok"], true, "{}", preview["error"]);
    let after_backup = client.call(
        "create_post",
        json!({"topic": topic, "content": "バックアップ後"}),
    );
    assert_ne!(first, after_backup);
    let wrong_passphrase = client.secret_file(b"wrong-backup-passphrase-sentinel");
    let rejected = client.raw_call(
        "restore_device_backup_command",
        json!({"path": backup, "replace_existing": true}),
        Some(&wrong_passphrase),
        None,
    );
    assert_eq!(rejected["ok"], false);
    assert!(!rejected.to_string().contains("sentinel"));
    assert_eq!(
        client.call("get_desktop_startup_status", json!({}))["status"],
        "ready"
    );
    assert_eq!(
        client.call("list_timeline", json!({"topic": topic}))["items"]
            .as_array()
            .expect("items")
            .len(),
        2,
        "復号失敗では以前のruntimeと投稿を維持"
    );
    let restored = client.raw_call(
        "restore_device_backup_command",
        json!({"path": backup, "replace_existing": true}),
        Some(&passphrase),
        None,
    );
    assert_eq!(restored["ok"], true, "{}", restored["error"]);
    assert_eq!(
        client.call("get_desktop_startup_status", json!({}))["status"],
        "consent_required"
    );
    assert_eq!(
        client.raw_call("list_timeline", json!({"topic": topic}), None, None)["error"]["code"],
        "consent_required"
    );
    client.consent();
    let timeline = client.call("list_timeline", json!({"topic": topic}));
    assert_eq!(
        timeline["items"].as_array().expect("items").len(),
        1,
        "バックアップ時点を復元"
    );
    assert_eq!(timeline["items"][0]["object_id"], first);
    let new = client.call(
        "create_post",
        json!({"topic": topic, "content": "待機なしで新規入力"}),
    );
    assert_ne!(new, first);
    client.restart();
    assert_eq!(
        client.call("list_accounts", json!({}))["active_account_id"],
        accounts["active_account_id"]
    );
    assert_eq!(
        client.call("list_timeline", json!({"topic": topic}))["items"]
            .as_array()
            .expect("items")
            .len(),
        2
    );
    client.stop();
}

#[test]
fn real_cli_import_switch_and_failed_switch_preserve_the_current_account() {
    let mut client = ProcessClient::start();
    let mut donor = ProcessClient::start();
    client.consent();
    donor.consent();
    let initial = client.call("list_accounts", json!({}))["active_account_id"].clone();
    let donor_pubkey = donor.call("get_sync_status", json!({}))["local_author_pubkey"].clone();
    let passphrase_text = "account-import-passphrase-sentinel";
    let passphrase = donor.secret_file(passphrase_text.as_bytes());
    let exported = donor.path("identity.export");
    assert_eq!(
        donor.raw_call(
            "export_account_key",
            json!({}),
            Some(&passphrase),
            Some(&exported)
        )["ok"],
        true
    );
    let secret = client.secret_file(&serde_json::to_vec(&json!({"export": std::fs::read_to_string(exported).expect("暗号化export"), "passphrase": passphrase_text})).expect("Secret JSON"));
    let imported = client.raw_call(
        "import_account_key",
        json!({"label": "追加account"}),
        Some(&secret),
        None,
    );
    assert_eq!(imported["ok"], true, "{}", imported["error"]);
    assert!(!imported.to_string().contains("sentinel"));
    let id = imported["data"]["id"].clone();
    client.call("switch_account", json!({"account_id": id}));
    assert_eq!(
        client.call("get_sync_status", json!({}))["local_author_pubkey"],
        donor_pubkey
    );
    client.call("switch_account", json!({"account_id": initial}));
    let before = client.call("get_sync_status", json!({}))["local_author_pubkey"].clone();
    let record: kukuri_desktop_runtime::AccountRecord =
        serde_json::from_value(imported["data"].clone()).expect("account record");
    let db = kukuri_desktop_runtime::account_db_path(&client.app_data_dir(), &record.id);
    std::fs::write(&db, b"invalid-sqlite-fixture").expect("一時accountの破損fixture");
    assert_eq!(
        client.raw_call("switch_account", json!({"account_id": id}), None, None)["ok"],
        false
    );
    assert_eq!(
        client.call("list_accounts", json!({}))["active_account_id"],
        initial
    );
    assert_eq!(
        client.call("get_sync_status", json!({}))["local_author_pubkey"],
        before
    );
    client.stop();
    donor.stop();
}
