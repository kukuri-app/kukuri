use std::sync::Arc;

use kukuri_cli::{
    dispatcher::{DispatchReply, Dispatcher},
    protocol::{CommandEffect, PROTOCOL_VERSION, RequestEnvelope, SecretInput},
    registry::CommandRegistry,
};
use kukuri_desktop_runtime::{ClientHost, DesktopRuntime};
use serde_json::{Value, json};

fn request(command: &str, payload: Value, secret: Option<&[u8]>, output: bool) -> RequestEnvelope {
    RequestEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: "private-contract".into(),
        command: command.into(),
        profile: "test".into(),
        payload,
        timeout_ms: None,
        secret_bytes: secret.map(|bytes| bytes.len() as u64),
        accepts_secret_output: output,
    }
}

#[test]
fn private_credentials_use_only_declared_secret_frames() {
    let registry = CommandRegistry::builtin();
    for name in [
        "export_private_channel_invite",
        "export_channel_access_token",
        "export_friend_only_grant",
        "export_friend_plus_share",
    ] {
        let metadata = &registry.get(name).expect("export登録").metadata;
        assert_eq!(metadata.effect, CommandEffect::Write);
        assert!(metadata.secret_output && !metadata.secret_input);
        assert_eq!(metadata.output_schema["additionalProperties"], false);
        assert!(metadata.output_schema["properties"].get("token").is_none());
    }
    for name in [
        "import_private_channel_invite",
        "import_channel_access_token",
        "import_friend_only_grant",
        "import_friend_plus_share",
        "preview_channel_access_token",
    ] {
        let metadata = &registry.get(name).expect("import登録").metadata;
        assert!(metadata.secret_input && !metadata.secret_output);
        assert_eq!(metadata.input_schema["additionalProperties"], false);
        assert_eq!(metadata.input_schema["properties"], json!({}));
        assert_eq!(metadata.output_schema["additionalProperties"], false);
        assert!(
            metadata.output_schema["properties"]
                .get("namespace_secret_hex")
                .is_none()
        );
    }
}

#[tokio::test]
async fn invite_secret_and_namespace_never_reach_json_and_no_operation_history_is_created() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime.clone())
        .await
        .unwrap();
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-private-contract";
    let create = request(
        "create_private_channel",
        json!({"topic": topic, "label": "private contract"}),
        None,
        false,
    );
    let DispatchReply::Unary(created, _) = dispatcher
        .dispatch(create.clone(), None, "test", Some(&host))
        .await
    else {
        panic!("JSON")
    };
    assert!(created.ok, "{:?}", created.error);
    let channel_id = created.data.as_ref().unwrap()["channel_id"].clone();
    let export = request(
        "export_private_channel_invite",
        json!({"topic": topic, "channel_id": channel_id}),
        None,
        true,
    );
    let DispatchReply::Unary(exported, secret) =
        dispatcher.dispatch(export, None, "test", Some(&host)).await
    else {
        panic!("secret response")
    };
    assert!(exported.ok, "{:?}", exported.error);
    let secret = secret.expect("専用frame");
    let token = secret.expose().to_vec();
    let token_json: Value = serde_json::from_slice(&token).unwrap();
    let content: Value =
        serde_json::from_str(token_json["envelope"]["content"].as_str().unwrap()).unwrap();
    let namespace = content["namespace_secret_hex"].as_str().unwrap().as_bytes();
    assert_eq!(exported.data, Some(json!({"kind": "invite"})));

    let preview = request(
        "preview_channel_access_token",
        json!({}),
        Some(&token),
        false,
    );
    let DispatchReply::Unary(previewed, _) = dispatcher
        .dispatch(
            preview,
            Some(SecretInput::new(token.clone())),
            "test",
            Some(&host),
        )
        .await
    else {
        panic!("JSON")
    };
    assert!(previewed.ok, "{:?}", previewed.error);
    // 所有中のchannelへのimportでも返却型のnamespace secretを明示的に除外する。
    let import = request(
        "import_private_channel_invite",
        json!({}),
        Some(&token),
        false,
    );
    let DispatchReply::Unary(imported, _) = dispatcher
        .dispatch(
            import.clone(),
            Some(SecretInput::new(token.clone())),
            "test",
            Some(&host),
        )
        .await
    else {
        panic!("JSON")
    };
    assert!(imported.ok, "{:?}", imported.error);
    let DispatchReply::Unary(repeated, _) = dispatcher
        .dispatch(
            import,
            Some(SecretInput::new(token.clone())),
            "test",
            Some(&host),
        )
        .await
    else {
        panic!("JSON")
    };
    assert!(repeated.ok, "{:?}", repeated.error);
    assert_eq!(
        imported.data, repeated.data,
        "既存domainの同じ招待のimportを維持する"
    );
    for response in [exported, previewed, imported, repeated] {
        let bytes = serde_json::to_vec(&response).unwrap();
        assert!(!bytes.windows(token.len()).any(|part| part == token));
        assert!(!bytes.windows(namespace.len()).any(|part| part == namespace));
    }
    let invalid = b"secret-invalid-token-sentinel";
    let bad_import = request(
        "import_private_channel_invite",
        json!({}),
        Some(invalid),
        false,
    );
    let DispatchReply::Unary(rejected, _) = dispatcher
        .dispatch(
            bad_import,
            Some(SecretInput::new(invalid.to_vec())),
            "test",
            Some(&host),
        )
        .await
    else {
        panic!("JSON")
    };
    assert!(!rejected.ok);
    assert!(
        !serde_json::to_string(&rejected)
            .unwrap()
            .contains("sentinel")
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;
    assert!(
        !runtime
            .db_path()
            .with_file_name("kukuri.idempotency.sqlite3")
            .exists()
    );
}

/// 129 件を超える参加を `next_cursor` で読み進めると、すべての channel に一度ずつ届く(#1218 AC-4d)。
/// 新しい参加は購読の上限(64)で止まるので、上限を超える参加の行は凍結した旧 registry の移行で作る(ADR 0061 §9)。
#[tokio::test]
async fn joined_channel_list_reaches_every_channel_through_cursor_pages() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("kukuri.db");
    let topic = "kukuri:topic:cli-joined-pages";
    let owner = "11".repeat(32);
    let registry = (0..130)
        .map(|index| {
            json!({
                "topic_id": topic, "channel_id": format!("channel-{index:03}"),
                "label": format!("channel {index}"), "creator_pubkey": owner, "owner_pubkey": owner,
                "joined_via_pubkey": owner, "audience_kind": "invite_only",
                "current_epoch_id": "epoch-86400000-x", "current_epoch_secret_hex": "22".repeat(32),
            })
        })
        .collect::<Vec<_>>();
    let registry_key = blake3::hash(b"registry").to_hex();
    std::fs::write(
        db.with_extension(format!("private-channel-capabilities-{registry_key}")),
        Value::Array(registry).to_string(),
    )
    .unwrap();
    let runtime = Arc::new(DesktopRuntime::new(&db).await.unwrap());
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime.clone())
        .await
        .unwrap();
    let dispatcher = Dispatcher::builtin();
    let mut listed = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let DispatchReply::Unary(reply, _) = dispatcher
            .dispatch(
                request(
                    "list_joined_private_channels",
                    json!({"topic": topic, "cursor": cursor}),
                    None,
                    false,
                ),
                None,
                "test",
                Some(&host),
            )
            .await
        else {
            panic!("JSON")
        };
        assert!(reply.ok, "{:?}", reply.error);
        let page = reply.data.unwrap();
        pages += 1;
        listed.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["channel_id"].as_str().unwrap().to_owned()),
        );
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(pages, 2, "128 件の page の続きを 1 回読む");
    assert_eq!(
        listed,
        (0..130)
            .map(|index| format!("channel-{index:03}"))
            .collect::<Vec<_>>(),
        "すべての channel に重複なく届く"
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;
}
