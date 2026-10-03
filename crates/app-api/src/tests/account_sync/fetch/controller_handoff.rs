//! #1219 W6 AC-4: 担当の明示の移譲（引き取りたい端末の依頼 → 旧担当の停止 → 移譲先の有効化）と、backup から復元した
//! 端末の引き取り（ADR 0018 §8）。固定した境界で止めて作り直しても、二重担当・世代の巻き戻しは起きず、未確定の間は
//! 保留する。

use super::controller_requests::{TOPIC, create, joined_on, restored, running_devices};
use super::*;
use crate::service::channel_sync_merge::controller_item;
use crate::{
    PrivateChannelControllerPending, PrivateChannelControllerState, PrivateChannelControllerTake,
};
use kukuri_core::ChannelControllerRequestV1;
use kukuri_store::PrivateChannelRow;

fn record(device_id: &str, generation: u64, transfer_to: Option<&str>) -> PrivateChannelController {
    PrivateChannelController {
        device_id: device_id.into(),
        generation,
        transfer_to: transfer_to.map(str::to_string),
    }
}

/// 参加の行の担当の記録（正本）。
async fn controller(app: &AppService, channel_id: &str) -> Option<PrivateChannelController> {
    app.services
        .projection_store
        .get_private_channel_by_id(channel_id)
        .await
        .expect("channel row")
        .and_then(|row| row.controller)
        .map(|json| serde_json::from_str(&json).expect("controller record"))
}

/// 新しい世代を作れる（判定を通る）端末。2 つあれば二重担当。
async fn updaters(devices: &[(&AppService, &str)], channel_id: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (app, id) in devices {
        if controller(app, channel_id).await.is_some_and(|controller| {
            controller.device_id == *id && controller.transfer_to.is_none()
        }) {
            found.push(id.to_string());
        }
    }
    found
}

fn devices<'a>(a: &'a AppService, b: &'a AppService) -> [(&'a AppService, &'static str); 2] {
    [(a, "device-a"), (b, "device-b")]
}

async fn view_state(app: &AppService, channel_id: &str) -> Option<PrivateChannelControllerState> {
    let state = app
        .services
        .stored_private_channel_state(&joined_private_channel_key(TOPIC, channel_id))
        .await
        .expect("state")
        .expect("joined channel");
    app.joined_private_channel_view_for_state(&state)
        .await
        .expect("view")
        .controller
}

fn record_item(channel_id: &str, controller: &PrivateChannelController) -> AccountSyncItem {
    controller_item(&ChannelId::new(channel_id), controller).expect("controller item")
}

/// B から旧担当（世代 1）への移譲の依頼。
fn request_item(channel_id: &str) -> AccountSyncItem {
    AccountSyncItem::edit(
        AccountSyncItemKey::ChannelControllerRequest {
            channel_id: ChannelId::new(channel_id),
        },
        Utc::now().timestamp_millis(),
        Some(
            serde_json::to_value(ChannelControllerRequestV1 {
                to_device_id: "device-b".into(),
                generation: 1,
            })
            .expect("request value"),
        ),
    )
}

/// 担当の記録を参加の行にだけ書く（遷移の行を書いた後、account 同期の台帳へ書く前に止まった端末の状態）。
async fn write_row_only(app: &AppService, channel_id: &str, controller: &PrivateChannelController) {
    let store = &app.services.projection_store;
    let row = store
        .get_private_channel_by_id(channel_id)
        .await
        .expect("row")
        .expect("joined row");
    store
        .put_private_channel(
            &PrivateChannelRow {
                controller: Some(serde_json::to_string(controller).expect("record")),
                ..row
            },
            &[],
        )
        .await
        .expect("row only");
}

/// 本人の端末の候補を `peers` で返す端末（差分の取得の task は起こさない。試験が契機を呼ぶ）。
async fn device_seeing(
    keys: &KukuriKeys,
    id: &str,
    docs: &DeviceDocs,
    store: Arc<MemoryStore>,
    peers: &[&str],
) -> AppService {
    let transport = Arc::new(FakeTransport::new(id, FakeNetwork::default()));
    let hints = CandidateHints {
        inner: transport.clone(),
        peers: Arc::new(std::sync::Mutex::new(
            peers
                .iter()
                .map(|peer| kukuri_transport::SeedPeer {
                    endpoint_id: (*peer).into(),
                    addr_hint: None,
                })
                .collect(),
        )),
    };
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        Arc::new(hints),
        Arc::new(docs.clone()),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    );
    register_account_replica(&app).await;
    app
}

/// 担当を引き取る端末 B の依頼で、接続している旧担当 A が停止し、B が次の世代で有効になる。その後は B だけが鍵を
/// 更新でき、A は保留になる。画面へ渡す状態も、B では「この端末」、A では「別の端末」になる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_device_that_takes_over_becomes_the_only_one_that_updates_keys() {
    let keys = generate_keys();
    let (a, b, _, _) = running_devices(&keys).await;
    let channel = create(&a, ChannelAudienceKind::InviteOnly).await;
    joined_on(&b, &channel).await;
    assert_eq!(
        view_state(&b, &channel).await,
        Some(PrivateChannelControllerState::OtherDevice)
    );
    assert_eq!(
        b.take_private_channel_controller(TOPIC, &channel)
            .await
            .expect("take over"),
        PrivateChannelControllerTake::Taken
    );
    assert_eq!(
        controller(&b, &channel).await,
        Some(record("device-b", 2, None))
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while controller(&a, &channel).await != Some(record("device-b", 2, None)) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the old device follows the new one");
    let held = a
        .rotate_private_channel(TOPIC, &channel)
        .await
        .expect_err("the old device stopped");
    assert!(held.is::<PrivateChannelControllerPending>());
    b.rotate_private_channel(TOPIC, &channel)
        .await
        .expect("the new device updates keys");
    assert_eq!(
        view_state(&b, &channel).await,
        Some(PrivateChannelControllerState::ThisDevice)
    );
    assert_eq!(
        view_state(&a, &channel).await,
        Some(PrivateChannelControllerState::OtherDevice)
    );
    assert_eq!(
        b.take_private_channel_controller(TOPIC, &channel)
            .await
            .expect("take again"),
        PrivateChannelControllerTake::Taken,
        "taking over twice changes nothing"
    );
}

/// 旧担当が本人の端末の候補にいなければ、何も書かずに「接続していない」を返す。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn taking_over_needs_the_old_device_online() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    restored(&[&a, &b]).await;
    let channel = create(&a, ChannelAudienceKind::InviteOnly).await;
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert_eq!(
        b.take_private_channel_controller(TOPIC, &channel)
            .await
            .expect("take over"),
        PrivateChannelControllerTake::NotConnected
    );
    assert!(
        b.read_account_sync_item(
            &AccountSyncItemKey::ChannelControllerRequest {
                channel_id: ChannelId::new(channel.as_str()),
            },
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read the request")
        .is_none(),
        "no request is written"
    );
    assert_eq!(
        controller(&b, &channel).await,
        Some(record("device-a", 1, None))
    );
}

/// 固定した境界（依頼の後・A の停止の行の後で account 同期の前・B の有効化の行の後で account 同期の前）で止めて端末を
/// 作り直しても、新しい世代を作れる端末は高々 1 つで、世代は戻らない。未確定の間はどちらも保留し、送り直しで完了する。
/// 移譲の後に同じ依頼や古い記録を再送しても戻らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handoff_stopped_at_each_boundary_ends_with_one_device() {
    let keys = generate_keys();
    let (a_docs, b_docs) = (DeviceDocs::new(), DeviceDocs::new());
    let (a_store, b_store) = (
        Arc::new(MemoryStore::default()),
        Arc::new(MemoryStore::default()),
    );
    let a = own_device(&keys, "device-a", &a_docs, a_store.clone(), a_store.clone()).await;
    let b = device_seeing(&keys, "device-b", &b_docs, b_store.clone(), &["device-a"]).await;
    a_docs.reading_from(&b_docs);
    b_docs.reading_from(&a_docs);
    restored(&[&a, &b]).await;
    let channel = create(&a, ChannelAudienceKind::InviteOnly).await;
    b.fetch_account_sync_from("device-a").await.expect("fetch");

    // 境界 1: B の依頼の後、A が取得する前に B を作り直す。A だけが鍵を更新できる。
    let request = request_item(&channel);
    b.publish_account_sync_item(request.clone())
        .await
        .expect("request");
    drop(b);
    let b = device_seeing(&keys, "device-b", &b_docs, b_store.clone(), &["device-a"]).await;
    restored(&[&b]).await;
    assert_eq!(updaters(&devices(&a, &b), &channel).await, ["device-a"]);

    // 境界 2: A が依頼を取り込んで停止の行を書いたが、account 同期へ書けないまま A を作り直す。どちらも保留する。
    a_docs.writes_down.store(true, Ordering::SeqCst);
    a.fetch_account_sync_from("device-b")
        .await
        .expect("the old device fetches the request");
    assert_eq!(
        controller(&a, &channel).await,
        Some(record("device-a", 1, Some("device-b")))
    );
    drop(a);
    a_docs.writes_down.store(false, Ordering::SeqCst);
    let a = own_device(&keys, "device-a", &a_docs, a_store.clone(), a_store.clone()).await;
    restored(&[&a]).await;
    assert!(updaters(&devices(&a, &b), &channel).await.is_empty());
    for app in [&a, &b] {
        let held = app
            .rotate_private_channel(TOPIC, &channel)
            .await
            .expect_err("no device updates keys while moving");
        assert!(held.is::<PrivateChannelControllerPending>());
    }
    assert_eq!(
        view_state(&a, &channel).await,
        Some(PrivateChannelControllerState::Moving)
    );
    a.resend_account_sync_items()
        .await
        .expect("resend the stop");

    // 境界 3: B が停止の記録を取り込んで有効化の行を書いたが、account 同期へ書けないまま B を作り直す。B だけが有効。
    b_docs.writes_down.store(true, Ordering::SeqCst);
    b.fetch_account_sync_from("device-a")
        .await
        .expect("the new device fetches the stop");
    assert_eq!(
        controller(&b, &channel).await,
        Some(record("device-b", 2, None))
    );
    drop(b);
    b_docs.writes_down.store(false, Ordering::SeqCst);
    let b = device_seeing(&keys, "device-b", &b_docs, b_store.clone(), &["device-a"]).await;
    restored(&[&b]).await;
    assert_eq!(updaters(&devices(&a, &b), &channel).await, ["device-b"]);
    b.resend_account_sync_items()
        .await
        .expect("resend the activation");
    a.fetch_account_sync_from("device-b")
        .await
        .expect("the old device follows");
    assert_eq!(
        controller(&a, &channel).await,
        Some(record("device-b", 2, None))
    );
    assert_eq!(updaters(&devices(&a, &b), &channel).await, ["device-b"]);

    // 同じ依頼・古い記録の再送では戻らない。
    a.merge_account_sync_item(request)
        .await
        .expect("the same request again");
    for app in [&a, &b] {
        app.merge_account_sync_item(record_item(&channel, &record("device-a", 1, None)))
            .await
            .expect("an old record again");
        assert_eq!(
            controller(app, &channel).await,
            Some(record("device-b", 2, None))
        );
    }
    assert_eq!(updaters(&devices(&a, &b), &channel).await, ["device-b"]);
}

/// 停止・有効化の行を書いた後、account 同期の台帳へ書く前に止まった端末は、作り直した後の同じ item の再受信（差分の
/// 取得は merge を終えてから cursor を進める）で同じ記録を書き直し、移譲を終える（#1219 AC-4 監査 B-1）。channel ごとに
/// 1 つの境界で止める。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transition_stopped_after_its_row_ends_when_the_item_comes_again() {
    let keys = generate_keys();
    let (a_docs, b_docs) = (DeviceDocs::new(), DeviceDocs::new());
    let (a_store, b_store) = (
        Arc::new(MemoryStore::default()),
        Arc::new(MemoryStore::default()),
    );
    let a = own_device(&keys, "device-a", &a_docs, a_store.clone(), a_store.clone()).await;
    let b = device_seeing(&keys, "device-b", &b_docs, b_store.clone(), &["device-a"]).await;
    a_docs.reading_from(&b_docs);
    b_docs.reading_from(&a_docs);
    restored(&[&a, &b]).await;
    let (stopped, activated) = (
        create(&a, ChannelAudienceKind::InviteOnly).await,
        create(&a, ChannelAudienceKind::InviteOnly).await,
    );
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    for channel in [&stopped, &activated] {
        b.publish_account_sync_item(request_item(channel))
            .await
            .expect("request");
    }

    // A が依頼を取り込んで `stopped` の停止の行だけを書いて止まり、作り直した後に依頼を受け直す。
    write_row_only(&a, &stopped, &record("device-a", 1, Some("device-b"))).await;
    drop(a);
    let a = own_device(&keys, "device-a", &a_docs, a_store.clone(), a_store.clone()).await;
    restored(&[&a]).await;
    a.fetch_account_sync_from("device-b")
        .await
        .expect("the old device fetches the requests again");

    // B が停止の記録を取り込んで `activated` の有効化の行だけを書いて止まり、作り直した後に停止の記録を受け直す。
    write_row_only(&b, &activated, &record("device-b", 2, None)).await;
    drop(b);
    let b = device_seeing(&keys, "device-b", &b_docs, b_store.clone(), &["device-a"]).await;
    restored(&[&b]).await;
    b.fetch_account_sync_from("device-a")
        .await
        .expect("the new device fetches the stops");
    a.fetch_account_sync_from("device-b")
        .await
        .expect("the old device follows");
    for channel in [&stopped, &activated] {
        for app in [&a, &b] {
            assert_eq!(
                controller(app, channel).await,
                Some(record("device-b", 2, None)),
                "{channel}"
            );
        }
        assert_eq!(updaters(&devices(&a, &b), channel).await, ["device-b"]);
        assert_eq!(
            view_state(&a, channel).await,
            Some(PrivateChannelControllerState::OtherDevice)
        );
    }
}

/// 同じ世代で端末が異なる 2 つの記録（古い backup の復元で起きる）は、どの順で受けても、どの端末でも端末 ID の大きい方に
/// 決まる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_of_the_same_generation_settle_on_one_device() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    restored(&[&a, &b]).await;
    let channel = create(&a, ChannelAudienceKind::InviteOnly).await;
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    let (low, high) = (record("device-c", 2, None), record("device-d", 2, None));
    for (app, first, second) in [(&a, &low, &high), (&b, &high, &low)] {
        app.merge_account_sync_item(record_item(&channel, first))
            .await
            .expect("first");
        app.merge_account_sync_item(record_item(&channel, second))
            .await
            .expect("second");
        assert_eq!(controller(app, &channel).await, Some(high.clone()));
    }
}

/// backup から復元した端末は、自分の channel の担当を次の世代で引き取る。旧担当は account 同期でそれに従い、保留になる。
/// 1 件目の行を書いた後・台帳の前で止まった起動のやり直しでも、その channel の記録を書き直す（#1219 AC-4 監査 B-1）。
/// やり直しても同じ記録のまま。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_device_takes_over_its_channels() {
    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    a_docs.reading_from(&b_docs);
    restored(&[&a, &b]).await;
    let first = create(&a, ChannelAudienceKind::InviteOnly).await;
    let second = create(&a, ChannelAudienceKind::FriendPlus).await;
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    write_row_only(&b, &first, &record("device-b", 2, None)).await;
    let mut after = String::new();
    while let Some(next) = b
        .claim_private_channel_controllers(&after, 1)
        .await
        .expect("claim")
    {
        after = next;
    }
    for channel in [&first, &second] {
        assert_eq!(
            controller(&b, channel).await,
            Some(record("device-b", 2, None))
        );
    }
    assert_eq!(
        b.claim_private_channel_controllers("", 8)
            .await
            .expect("claim again"),
        None
    );
    assert_eq!(
        controller(&b, &first).await,
        Some(record("device-b", 2, None)),
        "claiming again changes nothing"
    );
    a.fetch_account_sync_from("device-b")
        .await
        .expect("the old device follows");
    for channel in [&first, &second] {
        assert_eq!(
            controller(&a, channel).await,
            Some(record("device-b", 2, None))
        );
        let held = a
            .rotate_private_channel(TOPIC, channel)
            .await
            .expect_err("the old device stopped");
        assert!(held.is::<PrivateChannelControllerPending>());
    }
}
