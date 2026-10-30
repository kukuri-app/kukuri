//! #1981 AC-1: SDK の SNS（ADR 0070 §5.4、F-SNS-1・F-SNS-2）。ゲーム（SDK）と desktop は同じ runtime の既存の command を
//! 使う。ゲームの profile と desktop の profile の間で、投稿・スクリーンショット・返信・リアクションが同じ ID で行き来し、
//! ゲームは `post_received`・`reaction_received` を受ける。private channel と block は既存の規則どおりで、再起動と後からの
//! 参加でも許された範囲の投稿を読める。
#![allow(clippy::unwrap_used)]

mod support;

use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{
    call, consent, finish, introduce, isolated_environment, open_real, poll_once, request,
    response_for, seen_json, send, send_secret, subscribe, wait_for,
};

const TOPIC: &str = "kukuri:topic:test-game-sdk-social";

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn id() -> String {
    format!("s{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// `command` を読み直し、`ready` が満たされた応答の data を返す（相手の端末から届くまで待つ）。
fn until(
    handle: *mut KukuriHandle,
    what: &str,
    command: &str,
    payload: Value,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let data = call(handle, &id(), command, payload.clone());
        if ready(&data) {
            return data;
        }
        assert!(Instant::now() < deadline, "{what}: {data}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn ids(view: &Value) -> Vec<String> {
    view["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["object_id"].as_str().unwrap().to_owned())
        .collect()
}

fn item<'a>(view: &'a Value, object_id: &str) -> &'a Value {
    view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["object_id"] == object_id)
        .unwrap()
}

fn timeline(scope: Value) -> Value {
    json!({"topic": TOPIC, "scope": scope, "limit": 20})
}

fn thread(root: &str) -> Value {
    json!({"topic": TOPIC, "thread_id": root, "limit": 20})
}

fn post(
    handle: *mut KukuriHandle,
    content: &str,
    reply_to: Option<&str>,
    channel: Value,
) -> String {
    let mut payload = json!({"topic": TOPIC, "content": content, "channel_ref": channel});
    if let Some(reply_to) = reply_to {
        payload["reply_to"] = json!(reply_to);
    }
    call(handle, &id(), "create_post", payload)
        .as_str()
        .unwrap()
        .to_owned()
}

fn react(handle: *mut KukuriHandle, target: &str, emoji: &str) {
    call(
        handle,
        &id(),
        "toggle_reaction",
        json!({"target_topic_id": TOPIC, "target_object_id": target,
            "reaction_key": {"kind": "emoji", "emoji": emoji}, "channel_ref": {"kind": "public"}}),
    );
}

/// ゲームの画面を写した PNG（試験では固定の bytes）を添付して投稿する。
fn post_screenshot(handle: *mut KukuriHandle, dir: &Path) -> String {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..4096).map(|index| (index * 31) as u8));
    let path = dir.join("screenshot.png");
    std::fs::write(&path, &bytes).unwrap();
    call(
        handle,
        &id(),
        "create_post",
        json!({"topic": TOPIC, "content": "test: game screenshot",
            "channel_ref": {"kind": "public"},
            "attachments": [{"path": path, "hash": blake3::hash(&bytes).to_hex().to_string(),
                "byte_size": bytes.len(), "mime": "image/png", "file_name": "screenshot.png",
                "role": "image_original"}]}),
    )
    .as_str()
    .unwrap()
    .to_owned()
}

/// `post_received` で届いた投稿の data。
fn post_received(handle: *mut KukuriHandle, object_id: &str) -> Value {
    wait_for(handle, "post_received", |json, _| {
        (json["type"] == "post_received" && json["post"]["object_id"] == object_id)
            .then(|| json["post"].clone())
    })
}

fn announced(handle: *mut KukuriHandle, object_id: &str) -> bool {
    poll_once(handle, 64).unwrap();
    seen_json(handle).iter().any(|raw| {
        let json: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
        json["type"] == "post_received" && json["post"]["object_id"] == object_id
    })
}

/// 秘密の出力（招待の token）を返す command を送り、token を受け取る。
fn secret_output(handle: *mut KukuriHandle, command: &str, payload: Value) -> String {
    let request_id = id();
    let mut envelope: Value =
        serde_json::from_slice(&request(&request_id, command, payload)).unwrap();
    envelope["accepts_secret_output"] = json!(true);
    assert_eq!(send(handle, envelope.to_string().as_bytes()), 0);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let items = poll_once(handle, 64).unwrap();
        if let Some(position) = items
            .iter()
            .position(|item| item.kind == 1 && item.json["request_id"] == request_id.as_str())
        {
            assert_eq!(items[position].json["ok"], true, "{}", items[position].json);
            assert_eq!(
                items[position + 1].kind,
                2,
                "the token follows as a secret item"
            );
            return String::from_utf8(items[position + 1].raw.clone()).unwrap();
        }
        assert!(Instant::now() < deadline, "{command}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn import_invite(handle: *mut KukuriHandle, token: &str) {
    let request_id = id();
    assert_eq!(
        send_secret(
            handle,
            &request(&request_id, "import_private_channel_invite", json!({})),
            token.as_bytes(),
        ),
        0
    );
    let response = response_for(handle, &request_id, Duration::from_secs(60));
    assert_eq!(response["ok"], true, "{}", response["error"]);
}

fn pubkey(handle: *mut KukuriHandle) -> String {
    call(handle, &id(), "get_my_profile", json!({}))["pubkey"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn game_and_desktop_share_posts_replies_and_reactions_within_the_existing_rules() {
    isolated_environment();
    let (game_dir, desktop_dir, late_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let game = open_real(game_dir.path());
    let desktop = open_real(desktop_dir.path());
    for handle in [game, desktop] {
        consent(handle);
        subscribe(handle, TOPIC);
    }
    introduce(game, desktop);
    let public = json!({"kind": "public"});

    // ゲーム → desktop: スクリーンショットつきの投稿が同じ ID で届く。
    let shot = post_screenshot(game, game_dir.path());
    assert_eq!(
        post_received(game, &shot)["topic_id"],
        TOPIC,
        "the game's own post"
    );
    let seen = until(
        desktop,
        "the screenshot post",
        "list_timeline",
        timeline(public.clone()),
        |view| ids(view).contains(&shot),
    );
    let attachments = item(&seen, &shot)["attachments"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["mime"], "image/png");

    // desktop → ゲーム: 返信とリアクションが、ゲームに event で届く。
    let reply = post(
        desktop,
        "test: reply from desktop",
        Some(&shot),
        public.clone(),
    );
    react(desktop, &shot, "👍");
    let received = post_received(game, &reply);
    assert_eq!(received["reply_to"], shot.as_str());
    assert_eq!(received["root_id"], shot.as_str());
    assert_eq!(received["author_pubkey"], pubkey(desktop));
    wait_for(game, "reaction_received", |json, _| {
        (json["type"] == "reaction_received"
            && json["reaction"]["target_object_id"] == shot.as_str())
        .then_some(())
    });

    // ゲーム → desktop: 返信への返信とリアクションが、同じ thread に同じ ID で出る。
    let answer = post(
        game,
        "test: answer from the game",
        Some(&reply),
        public.clone(),
    );
    react(game, &reply, "🎉");
    until(
        desktop,
        "the game's reply and reaction",
        "list_thread",
        thread(&shot),
        |view| {
            ids(view).contains(&answer)
                && item(view, &reply)["reaction_summary"]
                    .as_array()
                    .is_some_and(|summary| summary.iter().any(|entry| entry["count"] == 1))
        },
    );

    // private channel: 招待した desktop だけが読める。
    let channel = call(
        game,
        &id(),
        "create_private_channel",
        json!({"topic": TOPIC, "label": "test squad", "audience_kind": "invite_only"}),
    )["channel_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let invite = secret_output(
        game,
        "export_private_channel_invite",
        json!({"topic": TOPIC, "channel_id": channel}),
    );
    import_invite(desktop, &invite);
    let private_ref = json!({"kind": "private_channel", "channel_id": channel});
    let private_scope = json!({"kind": "channel", "channel_id": channel});
    let secret = post(game, "test: squad only", None, private_ref);
    assert_eq!(post_received(game, &secret)["channel_id"], channel.as_str());
    until(
        desktop,
        "the private post",
        "list_timeline",
        timeline(private_scope.clone()),
        |view| ids(view).contains(&secret),
    );

    // 後から参加した profile: 公開の投稿は読めるが、private channel の投稿は読めない。
    let late = open_real(late_dir.path());
    consent(late);
    subscribe(late, TOPIC);
    introduce(game, late);
    until(
        late,
        "the earlier public post",
        "list_timeline",
        timeline(public.clone()),
        |view| ids(view).contains(&shot),
    );
    assert!(
        !ids(&call(
            late,
            &id(),
            "list_timeline",
            timeline(public.clone())
        ))
        .contains(&secret)
    );
    let request_id = id();
    send(
        late,
        &request(
            &request_id,
            "list_timeline",
            timeline(private_scope.clone()),
        ),
    );
    let refused = response_for(late, &request_id, Duration::from_secs(60));
    assert_eq!(refused["ok"], false, "a non-member cannot read the channel");

    // block: block した作者の投稿は表示せず、知らせも出ない。block していない desktop には届く。
    call(late, &id(), "block_author", json!({"pubkey": pubkey(game)}));
    assert!(
        !ids(&call(
            late,
            &id(),
            "list_timeline",
            timeline(public.clone())
        ))
        .contains(&shot)
    );
    let after_block = post(game, "test: after the block", None, public.clone());
    post_received(desktop, &after_block);
    until(
        desktop,
        "the post after the block",
        "list_timeline",
        timeline(public.clone()),
        |view| ids(view).contains(&after_block),
    );
    std::thread::sleep(Duration::from_secs(5));
    assert!(
        !ids(&call(
            late,
            &id(),
            "list_timeline",
            timeline(public.clone())
        ))
        .contains(&after_block)
    );
    assert!(
        !announced(late, &after_block),
        "a blocked author's post is not announced"
    );

    // 再起動: ゲームの profile を開き直しても、同じ ID の投稿・返信・private channel の投稿を読める。
    finish(game);
    let game = open_real(game_dir.path());
    let restored = call(game, &id(), "list_timeline", timeline(public));
    assert!(ids(&restored).contains(&shot) && ids(&restored).contains(&after_block));
    let restored = call(game, &id(), "list_thread", thread(&shot));
    assert!(ids(&restored).contains(&reply) && ids(&restored).contains(&answer));
    assert!(ids(&call(game, &id(), "list_timeline", timeline(private_scope))).contains(&secret));
    for handle in [game, desktop, late] {
        finish(handle);
    }
}
