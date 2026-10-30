//! #1978 AC-2・#1979 AC-1: SDK のゲームの action session への参加と寿命、packet の送受信(ADR 0070 §5.2)。分離した
//! profile(loopback の runtime)で、4 種類の公開範囲の許可と拒否、退出・終了・既存の権限の失効で session が閉じ、
//! 閉じた session では packet を送受信しないことを確かめる。

use super::*;
use crate::{GameDelivery, GameSendRefused};
use kukuri_test_support::{PollState, poll_until};
use tokio::sync::broadcast::Receiver;

async fn new_runtime(dir: &Path, name: &str) -> Result<DesktopRuntime> {
    DesktopRuntime::new_with_config_and_identity(
        &dir.join(format!("{name}.db")),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
}

async fn pubkey(runtime: &DesktopRuntime) -> String {
    runtime
        .get_sync_status()
        .await
        .expect("sync status")
        .local_author_pubkey
}

fn channel_scope(channel_id: &str) -> TimelineScope {
    TimelineScope::Channel {
        channel_id: kukuri_core::ChannelId::new(channel_id),
    }
}

async fn create_room(
    host: &DesktopRuntime,
    topic: &str,
    channel: ChannelRef,
    max_players: u32,
) -> String {
    host.create_game_room(CreateGameRoomRequest {
        topic: topic.into(),
        channel_ref: channel,
        title: "arena".into(),
        description: String::new(),
        participants: Vec::new(),
        action: Some(kukuri_app_api::CreateActionSessionInput {
            application_id: "com.kukuri.test.arena".into(),
            max_players,
        }),
    })
    .await
    .expect("create action room")
}

async fn join(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: &TimelineScope,
    room_id: &str,
) -> std::result::Result<String, String> {
    runtime
        .join_game_session(JoinGameSessionRequest {
            topic: topic.into(),
            scope: scope.clone(),
            room_id: room_id.into(),
        })
        .await
        .map(|joined| joined.session_id)
        .map_err(|error| {
            error
                .downcast_ref::<kukuri_app_api::GameSessionRejected>()
                .map_or_else(|| format!("{error:#}"), |rejected| rejected.0.clone())
        })
}

/// room が一覧に出るまで待ってから参加する。公開範囲の資格が host に伝わるまでの `access_denied` は待ち直す。
async fn join_when_allowed(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: &TimelineScope,
    room_id: &str,
) -> String {
    // 時間切れのときに、一覧に出なかったのか、断られ続けたのかを区別する。
    let last = std::sync::Mutex::new(String::from("not listed"));
    let result = poll_until(
        social_graph_propagation_timeout(),
        Duration::from_millis(500),
        1,
        || async {
            let listed = runtime
                .list_action_rooms(ListActionRoomsRequest {
                    topic: topic.into(),
                    scope: scope.clone(),
                    application_id: "com.kukuri.test.arena".into(),
                    cursor: None,
                })
                .await?;
            if !listed.items.iter().any(|room| room.room_id == room_id) {
                return Ok::<_, anyhow::Error>(PollState::Pending);
            }
            match join(runtime, topic, scope, room_id).await {
                Ok(session_id) => Ok(PollState::Ready(session_id)),
                Err(code) if code == "access_denied" || code == "host_unreachable" => {
                    *last.lock().expect("last") = code;
                    Ok(PollState::Pending)
                }
                Err(code) => Err(anyhow::anyhow!("join failed: {code}")),
            }
        },
    )
    .await;
    result.unwrap_or_else(|error| {
        panic!(
            "join the game session: {error}; last: {}",
            last.lock().expect("last")
        )
    })
}

/// `pick` が値を返す event が届くまで待つ。
async fn event<T>(
    events: &mut Receiver<RuntimeEvent>,
    mut pick: impl FnMut(RuntimeEvent) -> Option<T>,
) -> T {
    timeout(Duration::from_secs(60), async {
        loop {
            if let Ok(event) = events.recv().await
                && let Some(value) = pick(event)
            {
                return value;
            }
        }
    })
    .await
    .expect("the expected event")
}

/// `session_id` の session が閉じた知らせが届くまで待ち、理由を返す。
async fn closed(events: &mut Receiver<RuntimeEvent>, session_id: &str) -> String {
    event(events, |event| match event {
        RuntimeEvent::GameSessionClosed {
            session_id: closed,
            reason,
            ..
        } if closed == session_id => Some(reason),
        _ => None,
    })
    .await
}

/// `session_id` の packet が届くまで取り出す。
async fn packet(runtime: &DesktopRuntime, session_id: &str) -> kukuri_iroh_node::GamePacket {
    poll_until(
        Duration::from_secs(30),
        Duration::from_millis(20),
        1,
        || async {
            Ok::<_, anyhow::Error>(
                runtime
                    .take_game_packets(true, |_| true)
                    .into_iter()
                    .find(|(id, _)| id == session_id)
                    .map_or(PollState::Pending, |(_, packet)| PollState::Ready(packet)),
            )
        },
    )
    .await
    .expect("a packet")
}

/// 閉じた session には、どちらの端末からも送れない。
fn refuses_packets(runtimes: [&DesktopRuntime; 2], session_id: &str) {
    for runtime in runtimes {
        assert_eq!(
            runtime.send_game_packet(session_id, GameDelivery::Stream, b"late"),
            Err(GameSendRefused::Closed)
        );
    }
}

async fn eventually_closed_on_host(host: &DesktopRuntime, session_id: &str) {
    poll_until(
        Duration::from_secs(30),
        Duration::from_millis(100),
        1,
        || async {
            Ok::<_, anyhow::Error>(if host.game_sessions.is_open(session_id) {
                PollState::Pending
            } else {
                PollState::Ready(())
            })
        },
    )
    .await
    .expect("the host removes the session");
}

/// 公開 topic の room は誰でも参加でき、host を含めて最大人数まで。退出・host の block・host が外す・room の終了で閉じ、閉じた
/// session は両方の端末で送受信を認可しない。終わった room には参加できない。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_public_game_session_lasts_until_leave_block_or_end() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let topic = "kukuri:topic:test-game-session-public";
    let host = new_runtime(dir.path(), "host").await.expect("host");
    let first = new_runtime(dir.path(), "first").await.expect("first");
    let second = new_runtime(dir.path(), "second").await.expect("second");
    refresh_runtime_peer_tickets(&[&host, &first, &second])
        .await
        .expect("tickets");
    for runtime in [&host, &first, &second] {
        open_topic_column(runtime, topic, TimelineScope::Public)
            .await
            .expect("open topic");
    }
    let public = TimelineScope::Public;
    let room = create_room(&host, topic, ChannelRef::Public, 2).await;

    let mut first_events = first.subscribe_events();
    let mut host_events = host.subscribe_events();
    let session = join_when_allowed(&first, topic, &public, &room).await;
    assert!(first.game_sessions.is_open(&session));
    assert!(host.game_sessions.is_open(&session));
    // host には参加者とともに開いた知らせが届き、両方の向きで datagram と stream が届く。
    let first_key = pubkey(&first).await;
    event(&mut host_events, |event| match event {
        RuntimeEvent::GameSessionOpened {
            session_id,
            participant_pubkey,
            ..
        } if session_id == session && participant_pubkey == first_key => Some(()),
        _ => None,
    })
    .await;
    first
        .send_game_packet(&session, GameDelivery::Stream, b"catch")
        .expect("send to the host");
    let received = packet(&host, &session).await;
    assert_eq!(
        (received.delivery, &received.bytes[..]),
        (GameDelivery::Stream, &b"catch"[..])
    );
    host.send_game_packet(&session, GameDelivery::Datagram, b"state")
        .expect("send to the participant");
    let received = packet(&first, &session).await;
    assert_eq!(
        (received.delivery, &received.bytes[..]),
        (GameDelivery::Datagram, &b"state"[..])
    );
    let path = event(&mut first_events, |event| match event {
        RuntimeEvent::GameSessionPath {
            session_id, path, ..
        } if session_id == session => Some(path),
        _ => None,
    })
    .await;
    assert_eq!(path, "direct_p2p");
    // host を含めて 2 人の room は満員。
    let _ = join_when_allowed_or(&second, topic, &public, &room, "room_full").await;

    first
        .leave_game_session(LeaveGameSessionRequest {
            session_id: session.clone(),
        })
        .await
        .expect("leave");
    assert_eq!(closed(&mut first_events, &session).await, "left");
    assert!(!first.game_sessions.is_open(&session));
    assert_eq!(closed(&mut host_events, &session).await, "left");
    eventually_closed_on_host(&host, &session).await;
    refuses_packets([&first, &host], &session);

    // 退出で空いた枠に入れる。host が block すると、評価し直しで閉じる。
    let mut second_events = second.subscribe_events();
    let blocked = join_when_allowed(&second, topic, &public, &room).await;
    host.block_author(AuthorRequest {
        pubkey: pubkey(&second).await,
    })
    .await
    .expect("block");
    assert_eq!(closed(&mut second_events, &blocked).await, "access_revoked");
    assert!(!second.game_sessions.is_open(&blocked));
    eventually_closed_on_host(&host, &blocked).await;
    refuses_packets([&second, &host], &blocked);
    // 一覧には出ていて参加者の端末は要求を作れるが、host が断る。
    assert_eq!(
        join(&second, topic, &public, &room)
            .await
            .expect_err("blocked by the host"),
        "access_denied"
    );

    // host が参加者を外すと、両方の端末に `removed` が届いて閉じる(NGO の DisconnectRemoteClient。#1979 AC-2)。
    let removed = join_when_allowed(&first, topic, &public, &room).await;
    host.leave_game_session(LeaveGameSessionRequest {
        session_id: removed.clone(),
    })
    .await
    .expect("remove the participant");
    assert_eq!(closed(&mut first_events, &removed).await, "removed");
    assert_eq!(closed(&mut host_events, &removed).await, "removed");
    eventually_closed_on_host(&host, &removed).await;
    refuses_packets([&first, &host], &removed);

    // host が room を終えると閉じ、それ以上は参加できない。
    let ended = join_when_allowed(&first, topic, &public, &room).await;
    host.update_game_room(UpdateGameRoomRequest {
        topic: topic.into(),
        room_id: room.clone(),
        status: GameRoomStatus::Ended,
        phase_label: None,
        scores: Vec::new(),
    })
    .await
    .expect("end the room");
    assert_eq!(closed(&mut first_events, &ended).await, "room_ended");
    assert!(!first.game_sessions.is_open(&ended));
    assert!(!host.game_sessions.is_open(&ended));
    refuses_packets([&first, &host], &ended);
    assert_eq!(
        join(&first, topic, &public, &room)
            .await
            .expect_err("ended"),
        "room_ended"
    );

    for runtime in [host, first, second] {
        runtime.shutdown().await;
    }
}

/// 一覧に出た room への参加が `expected` で断られるまで待つ。
async fn join_when_allowed_or(
    runtime: &DesktopRuntime,
    topic: &str,
    scope: &TimelineScope,
    room_id: &str,
    expected: &str,
) -> String {
    poll_until(
        social_graph_propagation_timeout(),
        Duration::from_millis(500),
        1,
        || async {
            match join(runtime, topic, scope, room_id).await {
                Err(code) if code == expected => Ok::<_, anyhow::Error>(PollState::Ready(code)),
                Err(code) if code == "room_not_found" || code == "host_unreachable" => {
                    Ok(PollState::Pending)
                }
                other => Err(anyhow::anyhow!("unexpected join result: {other:?}")),
            }
        },
    )
    .await
    .expect("the join is refused")
}

/// 3 人を互いにつなぎ、topic を開く。
async fn three(dir: &Path, topic: &str) -> [DesktopRuntime; 3] {
    let owner = new_runtime(dir, "owner").await.expect("owner");
    let member = new_runtime(dir, "member").await.expect("member");
    let outsider = new_runtime(dir, "outsider").await.expect("outsider");
    refresh_runtime_peer_tickets(&[&owner, &member, &outsider])
        .await
        .expect("tickets");
    for runtime in [&owner, &member, &outsider] {
        open_topic_column(runtime, topic, TimelineScope::Public)
            .await
            .expect("open topic");
    }
    [owner, member, outsider]
}

async fn mutual(a: &DesktopRuntime, b: &DesktopRuntime, topic: &str) {
    let (a_key, b_key) = (pubkey(a).await, pubkey(b).await);
    warm_author_social_view(a, &b_key, "warm b").await;
    warm_author_social_view(b, &a_key, "warm a").await;
    a.follow_author(AuthorRequest {
        pubkey: b_key.clone(),
    })
    .await
    .expect("follow");
    b.follow_author(AuthorRequest {
        pubkey: a_key.clone(),
    })
    .await
    .expect("follow back");
    wait_for_mutual_author_view(a, &b_key, topic).await;
    wait_for_mutual_author_view(b, &a_key, topic).await;
}

async fn create_channel(
    owner: &DesktopRuntime,
    topic: &str,
    audience_kind: ChannelAudienceKind,
) -> String {
    owner
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "game".into(),
            audience_kind,
        })
        .await
        .expect("create private channel")
        .channel_id
}

/// private channel の room に、channel の参加者は入れ、参加者でない人は入れない。
async fn assert_members_only(
    owner: &DesktopRuntime,
    member: &DesktopRuntime,
    outsider: &DesktopRuntime,
    topic: &str,
    channel_id: &str,
) -> (String, String) {
    let scope = channel_scope(channel_id);
    open_topic_column(member, topic, scope.clone())
        .await
        .expect("open channel");
    let room = create_room(
        owner,
        topic,
        ChannelRef::PrivateChannel {
            channel_id: kukuri_core::ChannelId::new(channel_id),
        },
        4,
    )
    .await;
    let session = join_when_allowed(member, topic, &scope, &room).await;
    assert!(owner.game_sessions.is_open(&session));
    assert_eq!(
        join(outsider, topic, &scope, &room)
            .await
            .expect_err("an outsider is refused"),
        "access_denied"
    );
    (room, session)
}

/// invite_only: 有効な招待を読み込んだ人だけが入れる。channel から退出すると、評価し直しで閉じる。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invite_only_game_session_admits_invited_members_until_they_leave_the_channel() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let topic = "kukuri:topic:test-game-session-invite";
    let [owner, member, outsider] = three(dir.path(), topic).await;
    let channel_id = create_channel(&owner, topic, ChannelAudienceKind::InviteOnly).await;
    let invite = owner
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("invite");
    member
        .import_private_channel_invite(ImportPrivateChannelInviteRequest { token: invite })
        .await
        .expect("import invite");
    let mut events = member.subscribe_events();
    let (_, session) = assert_members_only(&owner, &member, &outsider, topic, &channel_id).await;

    member
        .leave_private_channel(LeavePrivateChannelRequest {
            topic: topic.into(),
            channel_id: channel_id.clone(),
        })
        .await
        .expect("leave the channel");
    assert_eq!(closed(&mut events, &session).await, "access_revoked");
    assert!(!member.game_sessions.is_open(&session));
    eventually_closed_on_host(&owner, &session).await;

    for runtime in [owner, member, outsider] {
        runtime.shutdown().await;
    }
}

/// friend_only: mutual で grant を受けた人は入れ、片方向の follow の人は入れない。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_friend_only_game_session_admits_mutual_grantees_only() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let topic = "kukuri:topic:test-game-session-friend-only";
    let [owner, member, outsider] = three(dir.path(), topic).await;
    mutual(&owner, &member, topic).await;
    outsider
        .follow_author(AuthorRequest {
            pubkey: pubkey(&owner).await,
        })
        .await
        .expect("one-way follow");
    let channel_id = create_channel(&owner, topic, ChannelAudienceKind::FriendOnly).await;
    let grant = owner
        .export_friend_only_grant(ExportFriendOnlyGrantRequest {
            topic: topic.into(),
            channel_id: channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("grant");
    wait_for_friend_only_grant_import(
        &member,
        &grant,
        social_graph_propagation_timeout(),
        "member imports the grant",
    )
    .await;
    assert!(
        outsider
            .import_friend_only_grant(ImportFriendOnlyGrantRequest { token: grant })
            .await
            .is_err(),
        "a one-way follower cannot use the grant"
    );
    assert_members_only(&owner, &member, &outsider, topic, &channel_id).await;

    for runtime in [owner, member, outsider] {
        runtime.shutdown().await;
    }
}

/// friend_plus: owner の mutual から既存の share の連鎖で入った人は入れ、連鎖の外の人は入れない。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_friend_plus_game_session_admits_the_share_chain_only() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let topic = "kukuri:topic:test-game-session-friend-plus";
    let [owner, sponsor, outsider] = three(dir.path(), topic).await;
    let member = new_runtime(dir.path(), "chain-member")
        .await
        .expect("member");
    refresh_runtime_peer_tickets(&[&owner, &sponsor, &outsider, &member])
        .await
        .expect("tickets");
    open_topic_column(&member, topic, TimelineScope::Public)
        .await
        .expect("open topic");
    mutual(&owner, &sponsor, topic).await;
    mutual(&sponsor, &member, topic).await;
    let channel_id = create_channel(&owner, topic, ChannelAudienceKind::FriendPlus).await;
    let share = || ExportFriendPlusShareRequest {
        topic: topic.into(),
        channel_id: channel_id.clone(),
        expires_at: None,
    };
    let owner_share = owner
        .export_friend_plus_share(share())
        .await
        .expect("owner share");
    wait_for_friend_plus_share_import(
        &sponsor,
        &owner_share,
        social_graph_propagation_timeout(),
        "sponsor imports the share",
    )
    .await;
    let sponsor_share = sponsor
        .export_friend_plus_share(share())
        .await
        .expect("sponsor share");
    wait_for_friend_plus_share_import(
        &member,
        &sponsor_share,
        social_graph_propagation_timeout(),
        "member imports the share",
    )
    .await;
    assert!(
        outsider
            .import_friend_plus_share(ImportFriendPlusShareRequest {
                token: sponsor_share,
            })
            .await
            .is_err(),
        "someone outside the chain cannot use the share"
    );
    refresh_runtime_peer_tickets(&[&owner, &sponsor, &outsider, &member])
        .await
        .expect("tickets");
    assert_members_only(&owner, &member, &outsider, topic, &channel_id).await;

    for runtime in [owner, sponsor, outsider, member] {
        runtime.shutdown().await;
    }
}
