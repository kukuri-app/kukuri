//! #1219 W6 AC-5: owner の端末が受けた参加者の record と、参加者との follow の edge を本人の端末間で同期し、鍵の配布を
//! 行う端末（担当）が使う（ADR 0061 §2、ADR 0018 §8）。owner の端末は 2 台（担当 A・担当でない B）で、record と edge は
//! 片方にだけ入れ、同期は差分の取得で行う（端末ごとに別の FakeNetwork なので、受信の offer は相手の端末に届かない）。

use super::controller_requests::{TOPIC, create, restored};
use super::*;
use crate::service::epoch_control_support::EPOCH_CONTROL_OUTBOX_PREFIX;
use kukuri_core::{ChannelControllerRequestV1, ChannelParticipantV1};

struct Owners {
    keys: KukuriKeys,
    a: AppService,
    b: AppService,
    a_store: Arc<MemoryStore>,
    b_store: Arc<MemoryStore>,
    channel: String,
}

/// 同じ account の owner の端末 A（channel を作った担当）と B。B は A から channel を取得済み。
async fn owners(audience_kind: ChannelAudienceKind) -> Owners {
    let keys = generate_keys();
    let (a_docs, b_docs) = (DeviceDocs::new(), DeviceDocs::new());
    let (a_store, b_store) = (
        Arc::new(MemoryStore::default()),
        Arc::new(MemoryStore::default()),
    );
    let a = own_device(&keys, "device-a", &a_docs, a_store.clone(), a_store.clone()).await;
    let b = own_device(&keys, "device-b", &b_docs, b_store.clone(), b_store.clone()).await;
    a_docs.reading_from(&b_docs);
    b_docs.reading_from(&a_docs);
    restored(&[&a, &b]).await;
    let channel = create(&a, audience_kind).await;
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    Owners {
        keys,
        a,
        b,
        a_store,
        b_store,
        channel,
    }
}

async fn current_epoch(app: &AppService, channel: &str) -> String {
    app.services
        .stored_private_channel_state(&joined_private_channel_key(TOPIC, channel))
        .await
        .expect("state")
        .expect("joined channel")
        .current_epoch_id
}

/// その端末が参加者の record を受ける（受信の検証の後と同じ入口）。
async fn receive(app: &AppService, channel: &str, participant: &str, record: ChannelParticipantV1) {
    let state = app
        .services
        .stored_private_channel_state(&joined_private_channel_key(TOPIC, channel))
        .await
        .expect("state")
        .expect("joined channel");
    app.take_in_private_channel_participant(
        &state,
        &Pubkey::from(participant.to_string()),
        &record,
        true,
    )
    .await
    .expect("take in the participant record");
}

fn joined(epoch_id: &str) -> ChannelParticipantV1 {
    ChannelParticipantV1 {
        epoch_id: epoch_id.into(),
        joined_at: 1,
        left_at: None,
    }
}

fn left(epoch_id: &str, left_at: i64) -> ChannelParticipantV1 {
    ChannelParticipantV1 {
        left_at: Some(left_at),
        ..joined(epoch_id)
    }
}

fn edge(subject: &str, target: &str, status: FollowEdgeStatus, updated_at: i64) -> FollowEdge {
    FollowEdge {
        subject_pubkey: subject.into(),
        target_pubkey: target.into(),
        status,
        updated_at,
        envelope_id: EnvelopeId::from(
            blake3::hash(format!("{subject}{target}{updated_at}").as_bytes())
                .to_hex()
                .to_string(),
        ),
    }
}

/// その端末の outbox にある、相手宛ての grant の数。
async fn grants(store: &MemoryStore, recipient: &str) -> usize {
    store
        .list_direct_message_outbox()
        .await
        .expect("outbox")
        .into_iter()
        .filter(|row| {
            row.peer_pubkey == recipient && row.dm_id.starts_with(EPOCH_CONTROL_OUTBOX_PREFIX)
        })
        .count()
}

/// 相手が grant を受け取った（ACK で outbox の行が消える）。
async fn acknowledge(store: &MemoryStore, recipient: &str) {
    for row in store.list_direct_message_outbox().await.expect("outbox") {
        if row.peer_pubkey == recipient {
            store
                .remove_direct_message_outbox(&row.dm_id, &row.message_id)
                .await
                .expect("acknowledged");
        }
    }
}

async fn rotate(app: &AppService, channel: &str) {
    app.rotate_private_channel(TOPIC, channel)
        .await
        .expect("rotate");
}

/// P の参加は A、退会は B が受ける。同期の後の A の鍵更新で、P へ grant を出さない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_leave_received_on_another_device_is_not_distributed() {
    let o = owners(ChannelAudienceKind::InviteOnly).await;
    let (p, e1) = (
        generate_keys().public_key_hex(),
        current_epoch(&o.a, &o.channel).await,
    );
    receive(&o.a, &o.channel, &p, joined(&e1)).await;
    receive(&o.b, &o.channel, &p, left(&e1, 2)).await;
    o.a.fetch_account_sync_from("device-b")
        .await
        .expect("fetch");
    rotate(&o.a, &o.channel).await;
    assert_eq!(grants(&o.a_store, &p).await, 0);
}

/// P の参加は B だけが受ける。同期の後の A の鍵更新で、P へ grant を出す（配布から漏れない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_join_received_on_another_device_is_distributed() {
    let o = owners(ChannelAudienceKind::InviteOnly).await;
    let (p, e1) = (
        generate_keys().public_key_hex(),
        current_epoch(&o.a, &o.channel).await,
    );
    receive(&o.b, &o.channel, &p, joined(&e1)).await;
    o.a.fetch_account_sync_from("device-b")
        .await
        .expect("fetch");
    rotate(&o.a, &o.channel).await;
    assert_eq!(grants(&o.a_store, &p).await, 1);
}

/// 相互フォロー限定の資格は、どちらの端末で観測した edge でも判断する。参加より前に B だけが観測した P → owner の
/// follow で、A は資格のある P へ grant を出す。owner が B で P のフォローを解除すると、A の次の鍵更新で P へ出さない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutual_follows_seen_on_either_device_decide_the_friend_only_grant() {
    let o = owners(ChannelAudienceKind::FriendOnly).await;
    let (owner, p) = (o.keys.public_key_hex(), generate_keys().public_key_hex());
    let e1 = current_epoch(&o.a, &o.channel).await;
    o.a_store
        .upsert_follow_edge(edge(&owner, &p, FollowEdgeStatus::Active, 1))
        .await
        .expect("owner follows");
    o.b_store
        .upsert_follow_edge(edge(&p, &owner, FollowEdgeStatus::Active, 1))
        .await
        .expect("the participant follows");
    receive(&o.a, &o.channel, &p, joined(&e1)).await;
    o.b.fetch_account_sync_from("device-a")
        .await
        .expect("fetch");
    o.a.fetch_account_sync_from("device-b")
        .await
        .expect("fetch");
    rotate(&o.a, &o.channel).await;
    assert_eq!(grants(&o.a_store, &p).await, 1, "the qualified participant");

    acknowledge(&o.a_store, &p).await;
    o.b.unfollow_author(&p)
        .await
        .expect("unfollow on the other device");
    o.a.fetch_account_sync_from("device-b")
        .await
        .expect("fetch");
    rotate(&o.a, &o.channel).await;
    assert_eq!(
        grants(&o.a_store, &p).await,
        0,
        "unfollowed on the other device"
    );
}

/// 回転の前に参加した人の record が遅れて担当でない B に届いたら、B は grant を出さず、同期した担当 A が出す。退会した
/// 人の遅れた参加の record では、どちらの端末も grant を出さず、参加中に戻らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_join_grants_come_only_from_the_controller() {
    let o = owners(ChannelAudienceKind::InviteOnly).await;
    let (p, q) = (
        generate_keys().public_key_hex(),
        generate_keys().public_key_hex(),
    );
    let e1 = current_epoch(&o.a, &o.channel).await;
    receive(&o.a, &o.channel, &p, joined(&e1)).await;
    receive(&o.a, &o.channel, &p, left(&e1, 2)).await;
    rotate(&o.a, &o.channel).await;
    o.b.fetch_account_sync_from("device-a")
        .await
        .expect("fetch");
    assert_ne!(current_epoch(&o.b, &o.channel).await, e1);

    receive(&o.b, &o.channel, &q, joined(&e1)).await;
    assert_eq!(grants(&o.b_store, &q).await, 0, "not the controller");
    o.a.fetch_account_sync_from("device-b")
        .await
        .expect("fetch");
    assert_eq!(grants(&o.a_store, &q).await, 1, "the controller");

    for (app, store) in [(&o.b, &o.b_store), (&o.a, &o.a_store)] {
        receive(app, &o.channel, &p, joined(&e1)).await;
        assert_eq!(grants(store, &p).await, 0);
        assert!(
            !store
                .is_active_private_channel_participant(&o.channel, &p)
                .await
                .expect("participant")
        );
    }
}

/// 積んだ grant は、送るたびに宛先の資格を確かめる。B で受けた P の退会と、B での Q のフォローの解除を同期した後の
/// 再送の 1 回で、A は P と Q への grant を送らずに消す。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_grant_is_dropped_when_its_recipient_loses_access() {
    let o = owners(ChannelAudienceKind::FriendOnly).await;
    let owner = o.keys.public_key_hex();
    let (p, q) = (
        generate_keys().public_key_hex(),
        generate_keys().public_key_hex(),
    );
    let e1 = current_epoch(&o.a, &o.channel).await;
    for participant in [&p, &q] {
        for (subject, target) in [(&owner, participant), (participant, &owner)] {
            o.a_store
                .upsert_follow_edge(edge(subject, target, FollowEdgeStatus::Active, 1))
                .await
                .expect("mutual");
        }
        receive(&o.a, &o.channel, participant, joined(&e1)).await;
    }
    o.b.fetch_account_sync_from("device-a")
        .await
        .expect("fetch");
    rotate(&o.a, &o.channel).await;
    assert_eq!(
        (grants(&o.a_store, &p).await, grants(&o.a_store, &q).await),
        (1, 1)
    );

    receive(&o.b, &o.channel, &p, left(&e1, 5)).await;
    o.b.unfollow_author(&q)
        .await
        .expect("unfollow on the other device");
    o.a.fetch_account_sync_from("device-b")
        .await
        .expect("fetch");
    AppService::flush_due_direct_message_outbox(&o.a.services, Utc::now().timestamp_millis())
        .await
        .expect("resend");
    assert_eq!(
        (grants(&o.a_store, &p).await, grants(&o.a_store, &q).await),
        (0, 0)
    );
}

/// 移譲の後の新しい担当 B は、旧担当 A だけが受けた参加者へ配り、退会者へは配らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_new_controller_uses_the_participants_the_old_one_received() {
    let o = owners(ChannelAudienceKind::InviteOnly).await;
    let (p, q) = (
        generate_keys().public_key_hex(),
        generate_keys().public_key_hex(),
    );
    let e1 = current_epoch(&o.a, &o.channel).await;
    receive(&o.a, &o.channel, &p, joined(&e1)).await;
    receive(&o.a, &o.channel, &q, joined(&e1)).await;
    receive(&o.a, &o.channel, &q, left(&e1, 2)).await;
    o.b.fetch_account_sync_from("device-a")
        .await
        .expect("fetch");
    o.b.publish_account_sync_item(AccountSyncItem::edit(
        AccountSyncItemKey::ChannelControllerRequest {
            channel_id: ChannelId::new(o.channel.as_str()),
        },
        Utc::now().timestamp_millis(),
        Some(
            serde_json::to_value(ChannelControllerRequestV1 {
                to_device_id: "device-b".into(),
                generation: 1,
            })
            .expect("request value"),
        ),
    ))
    .await
    .expect("request");
    o.a.fetch_account_sync_from("device-b")
        .await
        .expect("the old device stops");
    o.b.fetch_account_sync_from("device-a")
        .await
        .expect("the new device activates");
    rotate(&o.b, &o.channel).await;
    assert_eq!(
        (grants(&o.b_store, &p).await, grants(&o.b_store, &q).await),
        (1, 0)
    );
}

/// 鍵更新は本人の端末間の同期（bootstrap。#1213 D-12）から独立する。鍵更新の前後で account 同期の replica と hint の
/// topic は同じで、本人は grant の宛先に入らず、別の端末は同じ同期で新しい世代を受け取る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotations_leave_the_account_sync_channel_alone() {
    let o = owners(ChannelAudienceKind::InviteOnly).await;
    let before = o.keys.derive_account_sync();
    rotate(&o.a, &o.channel).await;
    let after = o.keys.derive_account_sync();
    assert_eq!(
        (before.replica_id(), before.hint_topic()),
        (after.replica_id(), after.hint_topic())
    );
    assert_eq!(grants(&o.a_store, &o.keys.public_key_hex()).await, 0);
    o.b.fetch_account_sync_from("device-a")
        .await
        .expect("fetch");
    assert_eq!(
        current_epoch(&o.b, &o.channel).await,
        current_epoch(&o.a, &o.channel).await
    );
}

/// 相互フォロー限定で、遅れて届いた参加の record を担当でない B が受けたら、B が手元で観測した edge の記録を参加者の
/// 記録より先に同期する。担当 A は、B だけが相互フォローを観測した P へ grant を 1 件出し、B でフォローを解除された Q へは
/// 出さない（#1219 AC-5 監査 B-1）。A が B から初めて取得する（周回の）場合と、差分の窓で取得する場合の両方。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_friend_only_join_received_elsewhere_uses_the_edges_seen_there() {
    for first_fetch_is_a_cycle in [false, true] {
        let o = owners(ChannelAudienceKind::FriendOnly).await;
        let owner = o.keys.public_key_hex();
        let (p, q) = (
            generate_keys().public_key_hex(),
            generate_keys().public_key_hex(),
        );
        let e1 = current_epoch(&o.a, &o.channel).await;
        if !first_fetch_is_a_cycle {
            o.a.fetch_account_sync_from("device-b")
                .await
                .expect("fetch");
        }
        rotate(&o.a, &o.channel).await;
        o.b.fetch_account_sync_from("device-a")
            .await
            .expect("fetch");
        // P とは B だけが相互フォローを観測した。Q とは A が相互フォローを観測し、B では owner がフォローを解除していた
        // （どちらも参加者になる前の観測なので、その時点では同期していない）。
        for (subject, target, status, updated_at) in [
            (&owner, &p, FollowEdgeStatus::Active, 1),
            (&p, &owner, FollowEdgeStatus::Active, 1),
            (&owner, &q, FollowEdgeStatus::Revoked, 2),
            (&q, &owner, FollowEdgeStatus::Active, 1),
        ] {
            o.b_store
                .upsert_follow_edge(edge(subject, target, status, updated_at))
                .await
                .expect("edge seen on B");
        }
        for (subject, target) in [(&owner, &q), (&q, &owner)] {
            o.a_store
                .upsert_follow_edge(edge(subject, target, FollowEdgeStatus::Active, 1))
                .await
                .expect("edge seen on A");
        }
        for participant in [&p, &q] {
            receive(&o.b, &o.channel, participant, joined(&e1)).await;
        }
        o.a.fetch_account_sync_from("device-b")
            .await
            .expect("fetch");
        assert_eq!(
            (grants(&o.a_store, &p).await, grants(&o.a_store, &q).await),
            (1, 0),
            "the first fetch from B is a cycle: {first_fetch_is_a_cycle}"
        );
        assert_eq!(
            (grants(&o.b_store, &p).await, grants(&o.b_store, &q).await),
            (0, 0)
        );
    }
}
