//! #1909 AC-1: CLI で保持する公開 topic を設定すると、daemon の runtime は画面の列なしに、topic の投稿と、設定の後に
//! 作られた公開スレッドの投稿を検証して取り込む。2 つの topic を同時に扱い、設定は host の作り直しで戻り、解除で外れる。

use std::sync::Arc;
use std::time::Duration;

use kukuri_cli::{
    dispatcher::{DispatchReply, Dispatcher},
    protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope},
};
use kukuri_desktop_runtime::{
    ClientHost, CreatePublicChannelRequest, DesktopRuntime, ImportPeerTicketRequest,
};
use serde_json::{Value, json};

const GENERAL: &str = "kukuri:topic:cli-retention-general";
const TEST: &str = "kukuri:topic:cli-retention-test";

async fn call(
    dispatcher: &Dispatcher,
    host: &Arc<ClientHost>,
    command: &str,
    payload: Value,
) -> ResponseEnvelope {
    let request = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: "retention-contract".into(),
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
        panic!("JSON response")
    };
    assert!(secret.is_none());
    response
}

async fn ok(
    dispatcher: &Dispatcher,
    host: &Arc<ClientHost>,
    command: &str,
    payload: Value,
) -> Value {
    let response = call(dispatcher, host, command, payload).await;
    assert!(response.ok, "{command}: {:?}", response.error);
    response.data.unwrap_or(Value::Null)
}

/// 保持の進み具合で、topic の索引ごとに読み終えた最後の object id。
fn progressed(status: &Value, topic: &str, index: &str) -> Vec<String> {
    status["topics"]
        .as_array()
        .expect("topics")
        .iter()
        .filter(|retained| retained["topic"] == topic)
        .flat_map(|retained| retained["progress"].as_array().expect("progress").clone())
        .filter(|progress| progress["index"] == index)
        .map(|progress| {
            progress["object_id"]
                .as_str()
                .expect("object id")
                .to_string()
        })
        .collect()
}

fn object_ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["object_id"].as_str().expect("object id").to_string())
        .collect()
}

#[tokio::test]
async fn retained_topics_are_taken_in_without_columns_and_restored_after_restart() {
    let root_a = tempfile::tempdir().unwrap();
    let root_k = tempfile::tempdir().unwrap();
    let a = Arc::new(
        DesktopRuntime::new(root_a.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let k = Arc::new(
        DesktopRuntime::new(root_k.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host_a = ClientHost::from_runtime(root_a.path().to_path_buf(), a.clone())
        .await
        .unwrap();
    let host_k = ClientHost::from_runtime(root_k.path().to_path_buf(), k.clone())
        .await
        .unwrap();
    for (source, target) in [(&a, &k), (&k, &a)] {
        source
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: target.local_peer_ticket().await.unwrap().unwrap(),
            })
            .await
            .unwrap();
    }
    let dispatcher = Dispatcher::builtin();

    // K は 2 つの topic を保持する(画面の列も desired の購読も使わない)。公開でない topic は拒否する。
    for topic in [GENERAL, TEST] {
        ok(
            &dispatcher,
            &host_k,
            "set_topic_retention",
            json!({"topic": topic, "retained": true}),
        )
        .await;
    }
    let rejected = call(
        &dispatcher,
        &host_k,
        "set_topic_retention",
        json!({"topic": "private/channel-1", "retained": true}),
    )
    .await;
    assert_eq!(rejected.error.expect("rejected").code, "validation_failed");

    // A は、K の設定の後に公開スレッドを作り(新形式の writer への切替を待つ)、スレッドと 2 つの topic へ投稿する。A は
    // 書き手の fixture なので、公開スレッドは runtime から作る。
    let channel = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match a
                .create_public_channel(CreatePublicChannelRequest {
                    topic: GENERAL.into(),
                    title: "保持するスレッド".into(),
                    first_post: "最初の投稿".into(),
                    attachments: Vec::new(),
                    content_labels: Vec::new(),
                })
                .await
            {
                Ok(channel) => break channel,
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .expect("the public channel is created once the writer is switched");
    let channel_id = channel.channel_id.clone();
    let first_post = channel.first_post_id.clone().expect("first post");
    let channel_post = ok(
        &dispatcher,
        &host_a,
        "create_post",
        json!({"topic": GENERAL, "content": "スレッドの投稿",
            "channel_ref": {"kind": "public_channel", "channel_id": channel_id}}),
    )
    .await;
    let general_post = ok(
        &dispatcher,
        &host_a,
        "create_post",
        json!({"topic": GENERAL, "content": "general の投稿"}),
    )
    .await;
    let test_post = ok(
        &dispatcher,
        &host_a,
        "create_post",
        json!({"topic": TEST, "content": "test の投稿"}),
    )
    .await;
    let channel_activity = format!("{channel_id}.{}", channel_post.as_str().expect("post id"));

    // K の取り込みが、索引の続きの位置を A の記録まで進める(設定し直しは次の回を待たずに起こす)。
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let status = ok(&dispatcher, &host_k, "get_topic_retention", json!({})).await;
            if progressed(&status, GENERAL, "channels/activity/").contains(&channel_activity)
                && progressed(&status, GENERAL, "channels/list/").contains(&channel_id)
                && progressed(&status, GENERAL, "indexes/timeline/")
                    .iter()
                    .any(|id| Some(id.as_str()) == general_post.as_str())
                && progressed(&status, TEST, "indexes/timeline/")
                    .iter()
                    .any(|id| Some(id.as_str()) == test_post.as_str())
            {
                break;
            }
            for topic in [GENERAL, TEST] {
                ok(
                    &dispatcher,
                    &host_k,
                    "set_topic_retention",
                    json!({"topic": topic, "retained": true}),
                )
                .await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("K takes in the topic and public channel posts");

    // A を止め、K の host を作り直す。保存した設定と topic の購読が戻る。
    host_a.shutdown().await;
    host_k.shutdown().await;
    let restarted = Arc::new(
        DesktopRuntime::new(root_k.path().join("kukuri.db"))
            .await
            .unwrap(),
    );
    let host_k = ClientHost::from_runtime(root_k.path().to_path_buf(), restarted)
        .await
        .unwrap();
    let status = ok(&dispatcher, &host_k, "get_topic_retention", json!({})).await;
    let topics = status["topics"]
        .as_array()
        .expect("topics")
        .iter()
        .map(|retained| retained["topic"].as_str().expect("topic").to_string())
        .collect::<Vec<_>>();
    assert_eq!(topics, vec![GENERAL.to_string(), TEST.to_string()]);
    let subscribed = |status: &Value| {
        status["subscribed_topics"]
            .as_array()
            .expect("subscribed topics")
            .iter()
            .map(|topic| topic.as_str().expect("topic").to_string())
            .collect::<Vec<_>>()
    };
    let sync = ok(&dispatcher, &host_k, "get_sync_status", json!({})).await;
    assert!(
        [GENERAL, TEST]
            .iter()
            .all(|topic| subscribed(&sync).contains(&topic.to_string())),
        "{sync:?}"
    );

    // A が止まっていても、K の手元に検証済みの投稿がある。公開スレッドの投稿は topic のタイムラインに出ず、スレッドに出る。
    let general = object_ids(
        &ok(
            &dispatcher,
            &host_k,
            "list_timeline",
            json!({"topic": GENERAL}),
        )
        .await,
    );
    assert!(
        general
            .iter()
            .any(|id| Some(id.as_str()) == general_post.as_str()),
        "{general:?}"
    );
    assert!(
        !general.contains(&first_post)
            && !general
                .iter()
                .any(|id| Some(id.as_str()) == channel_post.as_str()),
        "{general:?}"
    );
    let thread = object_ids(
        &ok(
            &dispatcher,
            &host_k,
            "list_timeline",
            json!({"topic": GENERAL, "scope": {"kind": "public_channel", "channel_id": channel_id}}),
        )
        .await,
    );
    assert!(
        thread.contains(&first_post)
            && thread
                .iter()
                .any(|id| Some(id.as_str()) == channel_post.as_str()),
        "{thread:?}"
    );
    let test = object_ids(
        &ok(
            &dispatcher,
            &host_k,
            "list_timeline",
            json!({"topic": TEST}),
        )
        .await,
    );
    assert!(
        test.iter()
            .any(|id| Some(id.as_str()) == test_post.as_str()),
        "{test:?}"
    );

    // 解除すると、その topic の購読と設定が外れる。
    ok(
        &dispatcher,
        &host_k,
        "set_topic_retention",
        json!({"topic": TEST, "retained": false}),
    )
    .await;
    let status = ok(&dispatcher, &host_k, "get_topic_retention", json!({})).await;
    assert_eq!(status["topics"].as_array().expect("topics").len(), 1);
    let sync = ok(&dispatcher, &host_k, "get_sync_status", json!({})).await;
    assert!(!subscribed(&sync).contains(&TEST.to_string()), "{sync:?}");
    assert!(subscribed(&sync).contains(&GENERAL.to_string()), "{sync:?}");
    dispatcher.finish_operations().await;
    host_k.shutdown().await;
}
