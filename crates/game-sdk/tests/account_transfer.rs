//! 既存の kukuri アカウントを移行で受けて始める入口（ADR 0070 §4.1。#1996 AC-3）。desktop の runtime を移行元にし、
//! SDK は「貼り付け」の経路（リンクを秘密の入力で渡す）で受ける。逆に、ゲームのアカウントを同じ鍵のまま desktop へ
//! 移して会話を続ける（§4.2「kukuriで続ける」。#1998 AC-2）。開始済みのゲームのアカウントを、受けた既存のアカウントへ
//! まとめる（§4.2「既存のアカウントへまとめる」。#1998 AC-3）。keyring を使わず loopback だけで動かす。期限切れは
//! 拒否・取消と同じ失敗の経路で、`kukuri-iroh-node` の移行の試験が確かめている。
#![allow(clippy::unwrap_used)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kukuri_desktop_runtime::{DecideAccountTransferRequest, DesktopRuntime};
use kukuri_game_sdk::KukuriHandle;
use serde_json::{Value, json};
use support::{
    call, consent, error_code, finish, introduce, isolated_environment, open_real, request,
    response_for, secret_output, send, send_secret, subscribe, wait_for, wait_for_startup,
};

const GAME: &str = "com.example.cyber-disc-arena";
const TOPIC: &str = "kukuri:topic:test-game-sdk-handoff";

fn id(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn status(handle: *mut KukuriHandle) -> Value {
    call(
        handle,
        &id("status"),
        "get_account_transfer_status",
        json!({}),
    )
}

fn wait_state(handle: *mut KukuriHandle, state: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let current = status(handle);
        if current["state"] == state {
            return current;
        }
        assert!(Instant::now() < deadline, "{state}: {current}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn paste(handle: *mut KukuriHandle, link: &str) {
    let id = id("open");
    assert_eq!(
        send_secret(
            handle,
            &request(&id, "open_account_transfer", json!({})),
            link.as_bytes()
        ),
        0
    );
    let response = response_for(handle, &id, Duration::from_secs(60));
    assert_eq!(response["ok"], true, "{response}");
}

fn decide(handle: *mut KukuriHandle, accept: bool) {
    call(
        handle,
        &id("decide"),
        "decide_account_transfer",
        json!({"accept": accept}),
    );
}

fn accounts(handle: *mut KukuriHandle) -> Value {
    call(handle, &id("accounts"), "list_accounts", json!({}))
}

fn public_key(handle: *mut KukuriHandle) -> Value {
    call(handle, &id("key"), "get_sync_status", json!({}))["local_author_pubkey"].clone()
}

fn reopen(dir: &std::path::Path) -> *mut KukuriHandle {
    let handle = open_real(dir);
    wait_for_startup(handle);
    let deadline = Instant::now() + Duration::from_secs(30);
    while call(
        handle,
        &id("ready"),
        "get_desktop_startup_status",
        json!({}),
    )["status"]
        != "ready"
    {
        assert!(Instant::now() < deadline, "the profile did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    handle
}

/// 移行元（desktop の runtime）。招待のリンクを出し、確認コードの承認を返す。
struct Source {
    tokio: tokio::runtime::Runtime,
    runtime: DesktopRuntime,
}

impl Source {
    fn start(dir: &std::path::Path) -> Self {
        let tokio = tokio::runtime::Runtime::new().unwrap();
        let runtime = tokio
            .block_on(DesktopRuntime::new(dir.join("source.db")))
            .unwrap();
        Self { tokio, runtime }
    }

    fn link(&self) -> String {
        self.tokio
            .block_on(self.runtime.create_account_transfer_invite())
            .unwrap()
            .link
    }

    fn state(&self) -> Value {
        serde_json::to_value(
            self.tokio
                .block_on(self.runtime.account_transfer_status())
                .unwrap(),
        )
        .unwrap()
    }

    fn wait_state(&self, state: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let current = self.state();
            if current["state"] == state {
                return current;
            }
            assert!(Instant::now() < deadline, "source {state}: {current}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn decide(&self, accept: bool) {
        self.tokio
            .block_on(
                self.runtime
                    .decide_account_transfer(DecideAccountTransferRequest { accept }),
            )
            .unwrap();
    }
}

/// 貼り付け → 両方に出る確認コードが一致 → 両方で承認 → 完了。受けたアカウントの ID を返す。
fn receive(source: &Source, handle: *mut KukuriHandle) -> Value {
    paste(handle, &source.link());
    let target = wait_state(handle, "confirming");
    assert_eq!(target["role"], "target");
    assert_eq!(source.wait_state("confirming")["code"], target["code"]);
    source.decide(true);
    decide(handle, true);
    let completed = wait_state(handle, "completed");
    source.wait_state("completed");
    assert_eq!(completed["history"], Value::Null);
    completed["account_id"].clone()
}

#[test]
fn an_existing_account_is_received_by_paste_and_kept_across_restarts() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let source = Source::start(dir.path());
    let existing = source.runtime.local_author_pubkey();
    let handle = open_real(&dir.path().join("game"));
    wait_for_startup(handle);
    consent(handle);
    let game = json!({"application_id": GAME});
    assert_eq!(
        call(handle, "none", "get_game_player", game.clone()),
        Value::Null
    );

    // 開始の前の移行は、受けたアカウントを足すだけで active を変えない。ゲームが切り替えてから始める。
    let created = public_key(handle);
    let received = receive(&source, handle);
    assert_eq!(public_key(handle), created);
    call(
        handle,
        "switch",
        "switch_account",
        json!({"account_id": received}),
    );
    assert_eq!(public_key(handle), existing);
    let player = call(handle, "start", "start_game_player", game.clone());
    assert_eq!(player["public_key"], existing);
    // 受けたアカウントは Steam Cloud に保存できない（ADR 0070 §4.3）。
    let cloud = request(
        "cloud",
        "set_cloud_account",
        json!({"application_id": GAME, "enabled": true}),
    );
    assert_eq!(send(handle, &cloud), 0);
    let response = response_for(handle, "cloud", Duration::from_secs(60));
    assert_eq!(
        response["error"]["details"]["reason"], "not_game_account",
        "{response}"
    );
    finish(handle);

    let handle = reopen(&dir.path().join("game"));
    assert_eq!(call(handle, "resume", "start_game_player", game), player);
    assert_eq!(public_key(handle), existing);
    assert_eq!(accounts(handle)["active_account_id"], received);
    finish(handle);
}

#[test]
fn refused_or_cancelled_transfers_keep_the_started_player() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let source = Source::start(dir.path());
    let handle = open_real(&dir.path().join("game"));
    wait_for_startup(handle);
    consent(handle);
    let game = json!({"application_id": GAME});
    let player = call(handle, "start", "start_game_player", game.clone());
    let before = accounts(handle);

    // 「一致しない」で拒否する。
    paste(handle, &source.link());
    wait_state(handle, "confirming");
    decide(handle, false);
    assert_eq!(wait_state(handle, "failed")["reason"], "rejected");
    assert_eq!(accounts(handle), before);

    // 確認コードを出している間に取り消す（移行先の移行は消え、何も保存しない）。
    paste(handle, &source.link());
    wait_state(handle, "confirming");
    call(handle, "cancel", "cancel_account_transfer", json!({}));
    wait_state(handle, "idle");
    assert_eq!(accounts(handle), before);

    // 開始済みで別のアカウントを受けると、profile には加わるが切り替えない（統合が要る）。
    let received = receive(&source, handle);
    assert_eq!(
        accounts(handle)["accounts"].as_array().unwrap().len(),
        before["accounts"].as_array().unwrap().len() + 1
    );
    let switch = id("switch");
    assert_eq!(
        send(
            handle,
            &request(&switch, "switch_account", json!({"account_id": received}))
        ),
        0
    );
    let refused = response_for(handle, &switch, Duration::from_secs(60));
    assert_eq!(error_code(&refused), "conflict", "{refused}");
    assert_eq!(
        refused["error"]["details"]["reason"],
        "integration_required"
    );
    assert_eq!(
        accounts(handle)["active_account_id"],
        before["active_account_id"]
    );
    assert_eq!(public_key(handle), player["public_key"]);
    assert_eq!(call(handle, "resume", "start_game_player", game), player);
    finish(handle);
}

/// 「kukuriで続ける」: ゲームが招待を出し、desktop（別の profile の同じ runtime）が受けて両方で承認する。desktop の
/// 移行先の完了の状態を返す。
fn hand_off(game: *mut KukuriHandle, desktop: *mut KukuriHandle) -> Value {
    let link = secret_output(
        game,
        &id("invite"),
        "create_account_transfer_invite",
        json!({}),
    );
    assert!(wait_state(game, "waiting")["expires_at_ms"].is_i64());
    paste(desktop, &link);
    let code = wait_state(desktop, "confirming")["code"].clone();
    assert_eq!(wait_state(game, "confirming")["code"], code);
    decide(game, true);
    decide(desktop, true);
    let completed = wait_state(desktop, "completed");
    assert_eq!(wait_state(game, "completed")["role"], "source");
    completed
}

#[test]
fn the_game_account_moves_to_desktop_with_the_same_key_and_the_conversation_continues() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let game = open_real(&dir.path().join("game"));
    let desktop = open_real(&dir.path().join("desktop"));
    for handle in [game, desktop] {
        wait_for_startup(handle);
        consent(handle);
    }
    let player = call(
        game,
        "start",
        "start_game_player",
        json!({"application_id": GAME}),
    );
    let before = accounts(game);
    subscribe(game, TOPIC);
    let post = call(
        game,
        "post",
        "create_post",
        json!({"topic": TOPIC, "content": "test: from the game", "channel_ref": {"kind": "public"}}),
    )
    .as_str()
    .unwrap()
    .to_owned();

    let received = hand_off(game, desktop)["account_id"].clone();
    call(
        desktop,
        "switch",
        "switch_account",
        json!({"account_id": received}),
    );
    assert_eq!(public_key(desktop), player["public_key"]);

    // desktop は履歴を移さずに、ゲームの端末から topic の timeline で同じ投稿を読み、返信する。
    subscribe(desktop, TOPIC);
    introduce(game, desktop);
    let deadline = Instant::now() + Duration::from_secs(120);
    while !call(
        desktop,
        &id("timeline"),
        "list_timeline",
        json!({"topic": TOPIC, "scope": {"kind": "public"}, "limit": 20}),
    )["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["object_id"] == post.as_str() && item["author_pubkey"] == player["public_key"]
        })
    {
        assert!(Instant::now() < deadline, "the game's post on desktop");
        std::thread::sleep(Duration::from_millis(500));
    }
    let reply = call(
        desktop,
        "reply",
        "create_post",
        json!({"topic": TOPIC, "content": "test: reply from desktop", "reply_to": post,
            "channel_ref": {"kind": "public"}}),
    );
    let seen = wait_for(game, "the reply", |json, _| {
        (json["type"] == "post_received" && json["post"]["object_id"] == reply)
            .then(|| json["post"].clone())
    });
    assert_eq!(seen["reply_to"], post.as_str());
    assert_eq!(seen["author_pubkey"], player["public_key"]);

    // ゲームは同じ game player とアカウントのまま続く。
    assert_eq!(accounts(game), before);
    assert_eq!(
        call(
            game,
            "resume",
            "start_game_player",
            json!({"application_id": GAME})
        ),
        player
    );
    finish(desktop);
    finish(game);
}

#[test]
fn a_refused_or_cancelled_hand_off_keeps_the_game_account() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let game = open_real(&dir.path().join("game"));
    let desktop = open_real(&dir.path().join("desktop"));
    for handle in [game, desktop] {
        wait_for_startup(handle);
        consent(handle);
    }
    let player = call(
        game,
        "start",
        "start_game_player",
        json!({"application_id": GAME}),
    );
    let before = accounts(game);

    // ゲームで「一致しない」を選ぶ。
    let link = secret_output(
        game,
        &id("invite"),
        "create_account_transfer_invite",
        json!({}),
    );
    paste(desktop, &link);
    wait_state(game, "confirming");
    decide(game, false);
    assert_eq!(wait_state(game, "failed")["reason"], "rejected");

    // 招待を出した後、相手が来る前に取り消す。
    secret_output(
        game,
        &id("invite"),
        "create_account_transfer_invite",
        json!({}),
    );
    call(game, "cancel", "cancel_account_transfer", json!({}));
    wait_state(game, "idle");

    assert_eq!(accounts(game), before);
    assert_eq!(public_key(game), player["public_key"]);
    assert_eq!(
        call(
            game,
            "resume",
            "start_game_player",
            json!({"application_id": GAME})
        ),
        player
    );
    finish(desktop);
    finish(game);
}

/// 曲線上の公開鍵（secp256k1 の生成点の 1〜4 倍の x 座標）。
const PEERS: [&str; 4] = [
    "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
    "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
    "e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13",
];

fn social(handle: *mut KukuriHandle, pubkey: &str) -> Value {
    call(
        handle,
        &id("social"),
        "get_author_social_view",
        json!({"pubkey": pubkey}),
    )
}

fn joined(handle: *mut KukuriHandle) -> Vec<String> {
    call(
        handle,
        &id("joined"),
        "list_joined_private_channels",
        json!({"topic": TOPIC}),
    )["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["channel_id"].as_str().unwrap().to_owned())
        .collect()
}

/// 招待制の private channel を作り、その招待を返す。
fn channel_with_invite(owner: *mut KukuriHandle) -> (String, String) {
    let channel = call(
        owner,
        &id("channel"),
        "create_private_channel",
        json!({"topic": TOPIC, "label": "test squad", "audience_kind": "invite_only"}),
    )["channel_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let invite = secret_output(
        owner,
        &id("invite"),
        "export_private_channel_invite",
        json!({"topic": TOPIC, "channel_id": channel}),
    );
    (channel, invite)
}

fn join(handle: *mut KukuriHandle, invite: &str) {
    let request_id = id("join");
    assert_eq!(
        send_secret(
            handle,
            &request(&request_id, "import_private_channel_invite", json!({})),
            invite.as_bytes(),
        ),
        0
    );
    let response = response_for(handle, &request_id, Duration::from_secs(60));
    assert_eq!(response["ok"], true, "{response}");
}

/// `handle` の timeline に投稿が届くまで待ち、その著者を返す。
fn author(handle: *mut KukuriHandle, object_id: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let timeline = call(
            handle,
            &id("timeline"),
            "list_timeline",
            json!({"topic": TOPIC, "scope": {"kind": "public"}, "limit": 20}),
        );
        if let Some(item) = timeline["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["object_id"] == *object_id)
        {
            return item["author_pubkey"].clone();
        }
        assert!(Instant::now() < deadline, "{object_id} did not arrive");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 持ち主から見た、private channel の今の epoch の参加者の数（持ち主を含む）。
fn participants(owner: *mut KukuriHandle, channel: &str) -> u64 {
    call(
        owner,
        &id("joined"),
        "list_joined_private_channels",
        json!({"topic": TOPIC}),
    )["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["channel_id"] == channel)
        .and_then(|item| item["participant_count"].as_u64())
        .unwrap_or(0)
}

fn merge(handle: *mut KukuriHandle, payload: Value) -> Value {
    let request_id = id("merge");
    assert_eq!(
        send(handle, &request(&request_id, "merge_game_player", payload)),
        0
    );
    response_for(handle, &request_id, Duration::from_secs(60))
}

/// 付け直しが終わるまで待つ。
fn copied(handle: *mut KukuriHandle) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let player = call(
            handle,
            &id("player"),
            "get_game_player",
            json!({"application_id": GAME}),
        );
        if player["copying"] == false {
            return player;
        }
        assert!(Instant::now() < deadline, "the copy did not finish");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn refused_reason(handle: *mut KukuriHandle, command: &str, payload: Value) -> Value {
    let request_id = id("refused");
    assert_eq!(send(handle, &request(&request_id, command, payload)), 0);
    let response = response_for(handle, &request_id, Duration::from_secs(60));
    assert_eq!(error_code(&response), "conflict", "{response}");
    response["error"]["details"]["reason"].clone()
}

#[test]
fn the_game_account_is_merged_into_an_existing_account_and_its_relations_follow() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let source = Source::start(dir.path());
    let existing = source.runtime.local_author_pubkey();
    let owned = source
        .tokio
        .block_on(
            source.runtime.create_private_channel(
                serde_json::from_value(
                    json!({"topic": TOPIC, "label": "test own", "audience_kind": "invite_only"}),
                )
                .unwrap(),
            ),
        )
        .unwrap()
        .channel_id;
    let other = open_real(&dir.path().join("other"));
    let game = open_real(&dir.path().join("game"));
    for handle in [other, game] {
        wait_for_startup(handle);
        consent(handle);
    }
    let start = json!({"application_id": GAME});
    let player = call(game, "start", "start_game_player", start.clone());
    let game_account = accounts(game)["active_account_id"].clone();
    for handle in [other, game] {
        subscribe(handle, TOPIC);
    }
    introduce(other, game);
    let post = call(
        game,
        "post",
        "create_post",
        json!({"topic": TOPIC, "content": "test: before the merge", "channel_ref": {"kind": "public"}}),
    );
    assert_eq!(author(other, &post), player["public_key"]);
    let [followed, unfollowed, blocked, muted] = PEERS.map(str::to_owned);
    for target in [&followed, &unfollowed, &existing] {
        call(
            game,
            &id("follow"),
            "follow_author",
            json!({"pubkey": target}),
        );
    }
    call(
        game,
        &id("unfollow"),
        "unfollow_author",
        json!({"pubkey": unfollowed}),
    );
    call(
        game,
        &id("block"),
        "block_author",
        json!({"pubkey": blocked}),
    );
    for target in [&muted, &existing] {
        call(game, &id("mute"), "mute_author", json!({"pubkey": target}));
    }
    let (others, invite) = channel_with_invite(other);
    join(game, &invite);

    // B を受けても、残す進行の選択で取り消せば何も変わらない（B は profile に残り、切り替えない）。
    let received = receive(&source, game);
    finish(game);
    let game = reopen(&dir.path().join("game"));
    assert_eq!(
        call(game, "resume", "start_game_player", start.clone()),
        player
    );

    let merged = merge(
        game,
        json!({"application_id": GAME, "account_id": received}),
    );
    assert_eq!(merged["ok"], true, "{merged}");
    assert_eq!(merged["data"]["game_player_id"], player["game_player_id"]);
    assert_eq!(merged["data"]["public_key"], existing);
    assert_eq!(merged["data"]["copying"], true);
    assert_eq!(copied(game)["public_key"], existing);
    assert_eq!(accounts(game)["active_account_id"], received);
    assert_eq!(public_key(game), existing);

    // A のフォロー・ブロック・ミュートを B で付け直す（解除したもの、B 自身は付けない）。
    assert_eq!(social(game, &followed)["following"], true);
    assert_eq!(social(game, &unfollowed)["following"], false);
    assert_eq!(social(game, &blocked)["blocking"], true);
    assert_eq!(social(game, &muted)["muted"], true);
    let following = call(
        game,
        &id("following"),
        "list_social_connections",
        json!({"kind": "following"}),
    );
    assert!(
        !following
            .as_array()
            .unwrap()
            .iter()
            .any(|view| view["author_pubkey"] == existing.as_str()),
        "{following}"
    );
    assert_eq!(social(game, &existing)["muted"], false);

    // 他の人からは、A の投稿は A の著者のまま読め、以後の投稿は B が書いたものとして届く。
    subscribe(game, TOPIC);
    introduce(other, game);
    let after = call(
        game,
        "post",
        "create_post",
        json!({"topic": TOPIC, "content": "test: after the merge", "channel_ref": {"kind": "public"}}),
    );
    assert_eq!(author(other, &after), existing);
    assert_eq!(author(other, &post), player["public_key"]);
    // まとめた後も、A の投稿の thread へ B が返信でき、他の人の thread で A の投稿（A の著者・公開範囲のまま）と並ぶ
    // （#1981 AC-2。掲示板の thread と同じ `list_thread`）。A の投稿は A の手元にあるので、B の手元へ届くのを待つ。
    assert_eq!(author(game, &post), player["public_key"]);
    let reply = call(
        game,
        "reply",
        "create_post",
        json!({"topic": TOPIC, "content": "test: reply after the merge", "reply_to": post,
            "channel_ref": {"kind": "public"}}),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let thread = loop {
        let thread = call(
            other,
            &id("thread"),
            "list_thread",
            json!({"topic": TOPIC, "thread_id": post, "limit": 20}),
        );
        if thread["items"].as_array().unwrap().len() == 2 {
            break thread;
        }
        assert!(
            Instant::now() < deadline,
            "the reply did not reach the thread: {thread}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let items = thread["items"].as_array().unwrap();
    assert_eq!(items[0]["object_id"], post);
    assert_eq!(items[0]["author_pubkey"], player["public_key"]);
    assert_eq!(items[1]["object_id"], reply);
    assert_eq!(items[1]["author_pubkey"], existing);
    assert_eq!(items[1]["root_id"], post);
    assert_eq!(items[0]["audience_label"], items[1]["audience_label"]);

    // A が参加していた private channel は写さず、招待されれば B が入れる。B の private channel はそのまま。
    let channels = joined(game);
    assert!(!channels.contains(&others), "{channels:?}");
    assert!(channels.contains(&owned), "{channels:?}");
    // A の参加資格は残る（持ち主から見て、持ち主と A の 2 人のまま）。新しい招待を出すと epoch が変わり、以後は
    // その epoch に入った人を数える。
    let deadline = Instant::now() + Duration::from_secs(60);
    while participants(other, &others) != 2 {
        assert!(Instant::now() < deadline, "A is no longer a participant");
        std::thread::sleep(Duration::from_millis(100));
    }
    let again = secret_output(
        other,
        &id("invite"),
        "export_private_channel_invite",
        json!({"topic": TOPIC, "channel_id": others}),
    );
    join(game, &again);
    assert!(joined(game).contains(&others));

    // A へは切り替えない。
    assert_eq!(
        refused_reason(game, "switch_account", json!({"account_id": game_account})),
        "integration_required"
    );
    finish(game);
    finish(other);
}

#[test]
fn a_merge_keeps_the_given_progress_resumes_after_restart_and_is_refused_while_copying() {
    isolated_environment();
    let dir = tempfile::tempdir().unwrap();
    let source = Source::start(dir.path());
    let game = open_real(&dir.path().join("game"));
    wait_for_startup(game);
    consent(game);
    let start = json!({"application_id": GAME});
    call(game, "start", "start_game_player", start.clone());
    let game_account = accounts(game)["active_account_id"].clone();
    let muted = PEERS[0];
    call(game, &id("mute"), "mute_author", json!({"pubkey": muted}));
    let cloud = |enabled: Option<bool>| {
        let (command, mut payload) = match enabled {
            Some(enabled) => ("set_cloud_account", json!({"enabled": enabled})),
            None => ("get_cloud_account", json!({})),
        };
        payload["application_id"] = json!(GAME);
        call(game, &id("cloud"), command, payload)["state"].clone()
    };
    assert_eq!(cloud(Some(true)), "on");
    let received = receive(&source, game);

    // ゲームが B の進行の ID を渡すと、その ID を残す。
    let progress = "00000000-0000-4000-8000-0000000000b0";
    let merged = merge(
        game,
        json!({"application_id": GAME, "account_id": received, "game_player_id": progress}),
    );
    assert_eq!(merged["data"]["game_player_id"], progress, "{merged}");
    assert_eq!(
        merged["data"]["public_key"],
        source.runtime.local_author_pubkey()
    );
    assert_eq!(accounts(game)["active_account_id"], received);
    // まとめた先の鍵は Steam Cloud に置かない（ADR 0070 §4.3）。
    assert_eq!(cloud(None), "off");
    copied(game);
    assert_eq!(social(game, muted)["muted"], true);

    // ミュートの付け直しの途中で止まったことにする（B でミュートを外し、位置を戻す）。
    call(
        game,
        &id("unmute"),
        "unmute_author",
        json!({"pubkey": muted}),
    );
    finish(game);
    std::fs::write(
        dir.path().join("game").join("game-player-merge.json"),
        serde_json::to_vec(&json!({"version": 1, "from_account_id": game_account,
            "to_account_id": received, "merged_at": 0, "kind": "mutes", "after": null}))
        .unwrap(),
    )
    .unwrap();
    let game = reopen(&dir.path().join("game"));
    // 付け直しの途中のもう一度のまとめは断る。
    assert_eq!(
        refused_reason(
            game,
            "merge_game_player",
            json!({"application_id": GAME, "account_id": game_account}),
        ),
        "merge_in_progress"
    );
    let resumed = call(game, "resume", "start_game_player", start);
    assert_eq!(resumed["game_player_id"], progress);
    copied(game);
    assert_eq!(social(game, muted)["muted"], true);
    assert!(
        !dir.path()
            .join("game")
            .join("game-player-merge.json")
            .exists()
    );
    finish(game);
}
