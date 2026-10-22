//! #1909 AC-1: CLI で保持する公開 topic を設定すると、daemon の runtime は画面の列なしに、topic の投稿と、設定の後に
//! 作られた公開スレッドの投稿を検証して取り込む。2 つの topic を同時に扱い、設定は host の作り直しで戻り、解除で外れる。
//! AC-3: 保持の期間と容量を CLI で設定する。

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

/// 保持の進み具合で、topic の索引ごとに読み終えた最後の entry の (時刻, object id)。
fn progressed(status: &Value, topic: &str, index: &str) -> Vec<(i64, String)> {
    status["topics"]
        .as_array()
        .expect("topics")
        .iter()
        .filter(|retained| retained["topic"] == topic)
        .flat_map(|retained| retained["progress"].as_array().expect("progress").clone())
        .filter(|progress| progress["index"] == index)
        .map(|progress| {
            (
                progress["created_at"].as_i64().expect("created at"),
                progress["object_id"]
                    .as_str()
                    .expect("object id")
                    .to_string(),
            )
        })
        .collect()
}

/// 読み終えた最後の entry に `id` があるか。
fn reached(status: &Value, topic: &str, index: &str, id: Option<&str>) -> bool {
    progressed(status, topic, index)
        .iter()
        .any(|(_, read)| Some(read.as_str()) == id)
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

    // 保持の期間と容量は既定で 30 日・10 GiB。範囲の外(0 日・366 日・1 GiB 未満)は拒否し、範囲の内は保存して出力に出す。
    let limits = |status: &Value| {
        (
            status["retention_days"].as_u64(),
            status["capacity_bytes"].as_u64(),
        )
    };
    let status = ok(&dispatcher, &host_k, "get_topic_retention", json!({})).await;
    assert_eq!(limits(&status), (Some(30), Some(10 << 30)));
    for (days, bytes) in [(0, 1_u64 << 30), (366, 1 << 30), (30, (1 << 30) - 1)] {
        let rejected = call(
            &dispatcher,
            &host_k,
            "set_topic_retention_limits",
            json!({"retention_days": days, "capacity_bytes": bytes}),
        )
        .await;
        assert_eq!(rejected.error.expect("rejected").code, "validation_failed");
    }
    ok(
        &dispatcher,
        &host_k,
        "set_topic_retention_limits",
        json!({"retention_days": 7, "capacity_bytes": 2_u64 << 30}),
    )
    .await;
    let status = ok(&dispatcher, &host_k, "get_topic_retention", json!({})).await;
    assert_eq!(limits(&status), (Some(7), Some(2 << 30)));

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
    // 公開スレッドの 2 件の投稿の activity の entry。同じ秒の entry は ID の順に読むので、続きの位置は (時刻, ID) の
    // 大きい方まで進む。
    let thread_at_a = ok(
        &dispatcher,
        &host_a,
        "list_timeline",
        json!({"topic": GENERAL, "scope": {"kind": "public_channel", "channel_id": channel_id}}),
    )
    .await;
    let last_activity = thread_at_a["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            let post = item["object_id"].as_str().expect("object id");
            (
                item["created_at"].as_i64().expect("created at"),
                format!("{channel_id}.{post}"),
            )
        })
        .max()
        .expect("thread posts");

    // K の取り込みが、索引の続きの位置を A の記録まで進める(設定し直しは次の回を待たずに起こす)。
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let status = ok(&dispatcher, &host_k, "get_topic_retention", json!({})).await;
            if progressed(&status, GENERAL, "channels/activity/")
                .iter()
                .any(|read| *read >= last_activity)
                && reached(&status, GENERAL, "channels/list/", Some(&channel_id))
                && reached(&status, GENERAL, "indexes/timeline/", general_post.as_str())
                && reached(&status, TEST, "indexes/timeline/", test_post.as_str())
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
