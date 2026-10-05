//! #1219 W6 AC-1: 新しい世代を作るのは channel ごとの鍵更新の担当端末だけ。同じ account の別端末は何も書かずに保留する。

use super::super::*;

use crate::{
    PrivateChannelCapability, PrivateChannelControllerPending, StartOwnerDomeHostingInput,
    SubmitDomeSessionInput,
};
use kukuri_core::{DomeSessionInputKindV1, SpatialContextV1};
use kukuri_store::PrivateChannelParticipantRow;

const TOPIC: &str = "kukuri:topic:private-controller";

/// 同じ account の 1 端末(端末 ID は transport の endpoint ID)。
fn device(keys: &KukuriKeys, device_id: &str) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new(device_id, FakeNetwork::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    );
    (app, store)
}

async fn create(app: &AppService, audience_kind: ChannelAudienceKind) -> String {
    let _ = app.list_timeline(TOPIC, None, 20).await;
    app.create_private_channel(CreatePrivateChannelInput {
        topic_id: TopicId::new(TOPIC),
        label: "controller".into(),
        audience_kind,
    })
    .await
    .expect("create private channel")
    .channel_id
}

async fn capability(app: &AppService, channel_id: &str) -> PrivateChannelCapability {
    app.get_private_channel_capability(TOPIC, channel_id)
        .await
        .expect("read capability")
        .expect("joined capability")
}

async fn state(app: &AppService, channel_id: &str) -> JoinedPrivateChannelState {
    app.joined_private_channel_state(TOPIC, channel_id)
        .await
        .expect("joined channel")
}

async fn post(app: &AppService, channel_id: &str) -> Result<String> {
    app.create_post_in_channel(
        TOPIC,
        ChannelRef::PrivateChannel {
            channel_id: ChannelId::new(channel_id),
        },
        "private post",
        None,
    )
    .await
}

fn is_pending(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<PrivateChannelControllerPending>()
        .is_some()
}

/// 保留した端末は世代を作らず、旧世代の凍結・grant の配布・状態の保存のどれも行っていない。
async fn assert_nothing_written(
    app: &AppService,
    store: &MemoryStore,
    channel_id: &str,
    epoch: &str,
) {
    let after = state(app, channel_id).await;
    assert_eq!(after.current_epoch_id, epoch, "no new epoch");
    assert!(
        app.archived_private_channel_epochs(&after, 8)
            .await
            .unwrap()
            .is_empty(),
        "no archived epoch"
    );
    assert!(
        fetch_private_channel_policy_from_replica(
            app.docs_sync(),
            &current_private_channel_replica_id(&after),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read policy")
        .is_none(),
        "the current epoch is not frozen"
    );
    assert!(
        store.list_direct_message_outbox().await.unwrap().is_empty(),
        "no handoff grant is queued"
    );
}

/// TR-1: 担当 A と同じ account の B が同時に rotate すると、A の 1 回だけが世代を作り、B は保留になる。
/// B の共有(招待)前の auto rotate と、その再試行も同じ判定で保留になる。鍵更新を伴わない投稿はそのまま行える。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_controlling_device_creates_a_new_epoch() {
    let keys = generate_keys();
    let (app_a, _) = device(&keys, "device-a");
    let (app_b, store_b) = device(&keys, "device-b");
    let channel_id = create(&app_a, ChannelAudienceKind::InviteOnly).await;
    let created = capability(&app_a, &channel_id).await;
    assert_eq!(
        created.controller,
        Some(Some(PrivateChannelController {
            device_id: "device-a".into(),
            generation: 1,
            transfer_to: None,
        })),
        "the creating device is the first controller"
    );
    app_b
        .restore_private_channel_capability(created.clone())
        .await
        .expect("the other device holds the channel");

    let (rotated_a, rotated_b) = tokio::join!(
        app_a.rotate_private_channel(TOPIC, &channel_id),
        app_b.rotate_private_channel(TOPIC, &channel_id),
    );
    let rotated_a = rotated_a.expect("the controller rotates");
    assert_ne!(rotated_a.current_epoch_id, created.current_epoch_id);
    let rotated = state(&app_a, &channel_id).await;
    assert_eq!(
        app_a
            .archived_private_channel_epochs(&rotated, 8)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(is_pending(&rotated_b.expect_err("the other device waits")));
    assert_nothing_written(&app_b, &store_b, &channel_id, &created.current_epoch_id).await;

    for attempt in 0..2 {
        let shared = app_b
            .export_channel_access_token(TOPIC, &channel_id, None)
            .await
            .expect_err("sharing needs a new epoch");
        assert!(is_pending(&shared), "attempt {attempt} is pending");
        assert_nothing_written(&app_b, &store_b, &channel_id, &created.current_epoch_id).await;
    }

    post(&app_b, &channel_id)
        .await
        .expect("a post without a key update is not held");
}

/// 参加者変更: friend-only で資格を失った参加者がいると、write 前に auto rotate が要る。担当は世代を作って投稿し、
/// 同じ account の B は旧鍵のまま投稿せずに保留する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_needing_a_rotation_waits_on_the_other_device() {
    let (app_a, app_b, store_b, channel_id, created) =
        a_rotation_needed_on_the_other_device().await;
    let held = post(&app_b, &channel_id)
        .await
        .expect_err("the old key is not used");
    assert!(is_pending(&held));
    assert_nothing_written(&app_b, &store_b, &channel_id, &created.current_epoch_id).await;

    post(&app_a, &channel_id)
        .await
        .expect("the controller rotates and posts");
    assert_ne!(
        state(&app_a, &channel_id).await.current_epoch_id,
        created.current_epoch_id
    );
}

/// #1552: Dome の読取り(hosting の取得・削除待ちの一覧)と session の入力は、鍵更新が要る channel でも担当でない端末で
/// 鍵更新の判定(自動の鍵更新・依頼・保留)を通らず、保留にならず何も書かない(ADR 0018 §8)。hosting の開始などの
/// 書き込みは、今のまま判定を通って保留になる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dome_reads_and_inputs_skip_the_key_update_on_the_other_device() {
    let (_app_a, app_b, store_b, channel_id, created) =
        a_rotation_needed_on_the_other_device().await;
    let context = SpatialContextV1::Channel {
        topic_id: TopicId::new(TOPIC),
        channel_id: ChannelId::new(channel_id.as_str()),
    };
    let not_pending = |result: Result<()>, what: &str| {
        if let Err(error) = result {
            assert!(!is_pending(&error), "{what} is held: {error:#}");
        }
    };
    not_pending(
        app_b
            .get_dome_hosting(context.clone(), "room")
            .await
            .map(drop),
        "reading the hosting",
    );
    not_pending(
        app_b
            .list_pending_dome_deletions(context.clone())
            .await
            .map(drop),
        "listing the pending deletions",
    );
    not_pending(
        app_b
            .submit_dome_session_input(SubmitDomeSessionInput {
                expected_generation: None,
                spatial_context: context.clone(),
                instance_id: "room".into(),
                sequence: 1,
                input: DomeSessionInputKindV1::KeepAlive,
            })
            .await
            .map(drop),
        "a session input",
    );
    assert_nothing_written(&app_b, &store_b, &channel_id, &created.current_epoch_id).await;
    let held = app_b
        .start_owner_dome_hosting(StartOwnerDomeHostingInput {
            expected_generation: None,
            spatial_context: context,
            instance_id: "room".into(),
            endpoint_id: "device-b".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .expect_err("starting the hosting writes with the old key");
    assert!(is_pending(&held));
}

/// 担当 A と同じ account の B が friend_only の channel を持ち、資格を失った参加者がいる(鍵更新が要る)。
async fn a_rotation_needed_on_the_other_device() -> (
    AppService,
    AppService,
    Arc<MemoryStore>,
    String,
    PrivateChannelCapability,
) {
    let keys = generate_keys();
    let (app_a, store_a) = device(&keys, "device-a");
    let (app_b, store_b) = device(&keys, "device-b");
    let channel_id = create(&app_a, ChannelAudienceKind::FriendOnly).await;
    let created = capability(&app_a, &channel_id).await;
    app_b
        .restore_private_channel_capability(created.clone())
        .await
        .expect("the other device holds the channel");
    // 相互 follow でなくなった参加者(資格を失った人。相手からの follow だけが残る)。
    let stale = generate_keys().public_key_hex();
    for store in [&store_a, &store_b] {
        store
            .upsert_follow_edge(FollowEdge {
                subject_pubkey: Pubkey::from(stale.as_str()),
                target_pubkey: Pubkey::from(keys.public_key_hex()),
                status: FollowEdgeStatus::Active,
                updated_at: 1,
                envelope_id: EnvelopeId::from("follow-of-the-stale-participant"),
            })
            .await
            .expect("one-way follow");
        store
            .put_private_channel_participant(PrivateChannelParticipantRow {
                channel_id: channel_id.clone(),
                epoch_id: created.current_epoch_id.clone(),
                participant_pubkey: stale.clone(),
                left_at: None,
                updated_at: 1,
            })
            .await
            .expect("stale participant");
    }
    (app_a, app_b, store_b, channel_id, created)
}

/// 担当が不明な channel は、どの端末でも世代を作らない。本変更前の保存(担当の欄が無い)は、保存していた端末を担当にし、
/// 再起動しても同じ端末 ID なら担当のまま。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_and_legacy_controllers_after_restart() {
    let keys = generate_keys();
    let (app_a, _) = device(&keys, "device-a");
    let channel_id = create(&app_a, ChannelAudienceKind::InviteOnly).await;
    let created = capability(&app_a, &channel_id).await;

    // 担当が不明な記録と、引継ぎ中(旧担当は停止済み)の記録。
    for controller in [
        None,
        Some(PrivateChannelController {
            device_id: "device-a".into(),
            generation: 1,
            transfer_to: Some("device-b".into()),
        }),
    ] {
        let (waiting, store) = device(&keys, "device-a");
        waiting
            .restore_private_channel_capability(PrivateChannelCapability {
                controller: Some(controller),
                ..created.clone()
            })
            .await
            .expect("restore the channel");
        let error = waiting
            .rotate_private_channel(TOPIC, &channel_id)
            .await
            .expect_err("no device rotates");
        assert!(is_pending(&error));
        assert_nothing_written(&waiting, &store, &channel_id, &created.current_epoch_id).await;
    }

    let (restarted, _) = device(&keys, "device-a");
    restarted
        .restore_private_channel_capability(created.clone())
        .await
        .expect("restart with the same device id");
    restarted
        .rotate_private_channel(TOPIC, &channel_id)
        .await
        .expect("the controller stays after a restart");

    let (legacy, _) = device(&keys, "device-legacy");
    legacy
        .restore_private_channel_capability(PrivateChannelCapability {
            controller: None,
            ..created
        })
        .await
        .expect("restore a capability saved before the controller record");
    assert_eq!(
        state(&legacy, &channel_id).await.controller,
        Some(PrivateChannelController {
            device_id: "device-legacy".into(),
            generation: 1,
            transfer_to: None,
        })
    );
    legacy
        .rotate_private_channel(TOPIC, &channel_id)
        .await
        .expect("the device that kept the legacy channel controls it");
}
