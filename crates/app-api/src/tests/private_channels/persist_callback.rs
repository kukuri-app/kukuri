use super::super::*;

use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicUsize;

use crate::PrivateChannelCapability;
use kukuri_core::{private_topic_rendezvous_key_hex_secret, wire::hint_topic_id};
use kukuri_docs_sync::private_channel_hint_topic;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_channel_rendezvous_keys_follow_the_current_epoch_secret() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:private-rendezvous";
    let _ = app.list_timeline(topic, None, 20).await;

    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "非公開ランデブー".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("非公開チャンネルを作成できる");
    let capability = app
        .get_private_channel_capability(topic, channel.channel_id.as_str())
        .await
        .expect("非公開チャンネルの権限を読み出せる")
        .expect("非公開チャンネルの権限がある");
    let rendezvous_topic = hint_topic_id(&private_channel_hint_topic(channel.channel_id.as_str()));
    let expected_before = private_topic_rendezvous_key_hex_secret(
        capability.current_epoch_secret_hex.as_str(),
        &rendezvous_topic,
    )
    .expect("世代切替前の非公開ランデブー鍵を派生できる");

    let keys_before = app.private_channel_rendezvous_keys().await;
    assert_eq!(
        keys_before.get(rendezvous_topic.as_str()),
        Some(&expected_before)
    );

    app.rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("非公開チャンネルの世代を切り替えられる");
    let rotated_capability = app
        .get_private_channel_capability(topic, channel.channel_id.as_str())
        .await
        .expect("世代切替後の非公開チャンネル権限を読み出せる")
        .expect("世代切替後の非公開チャンネル権限がある");
    let expected_after = private_topic_rendezvous_key_hex_secret(
        rotated_capability.current_epoch_secret_hex.as_str(),
        &rendezvous_topic,
    )
    .expect("世代切替後の非公開ランデブー鍵を派生できる");
    let keys_after = app.private_channel_rendezvous_keys().await;

    assert_ne!(expected_before, expected_after);
    assert_eq!(
        keys_after.get(rendezvous_topic.as_str()),
        Some(&expected_after)
    );
    assert!(!keys_after.values().any(|key| key == &expected_before));
}

/// capability registry の write-through callback が変異経路(register/remove)で
/// 発火し、state-only スナップショット(diagnostics 既定値)を受け取ることを固定する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_persist_callback_fires_on_registry_mutations() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let persist_calls = Arc::new(AtomicUsize::new(0));
    let last_snapshot: Arc<StdMutex<Vec<PrivateChannelCapability>>> =
        Arc::new(StdMutex::new(Vec::new()));
    {
        let persist_calls = persist_calls.clone();
        let last_snapshot = last_snapshot.clone();
        app.set_private_channel_capability_persist(Arc::new(move |capabilities| {
            persist_calls.fetch_add(1, Ordering::SeqCst);
            *last_snapshot.lock().expect("snapshot lock") = capabilities;
            Box::pin(async { Ok(()) })
        }));
    }

    let topic = "kukuri:topic:persist-callback";
    let _ = app.list_timeline(topic, None, 20).await;

    // register 経由(create)で発火し、スナップショットに channel が入る。
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "persist-callback".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let calls_after_create = persist_calls.load(Ordering::SeqCst);
    assert!(calls_after_create >= 1, "create must trigger persist");
    {
        let snapshot = last_snapshot.lock().expect("snapshot lock");
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].channel_id, channel.channel_id);
        assert_eq!(snapshot[0].current_epoch_id, channel.current_epoch_id);
        assert!(
            !snapshot[0].current_epoch_secret_hex.is_empty(),
            "secret must be persisted"
        );
        // state-only スナップショット: diagnostics 派生フィールドは既定値
        // (復元時に読まれないことは capability_registry_snapshot テストが固定)。
        assert!(!snapshot[0].rotation_required);
        assert_eq!(snapshot[0].participant_count, 0);
    }

    // rotate(register 経由の epoch 置換)で発火し、新 epoch が反映される。
    let rotated = app
        .rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("rotate private channel");
    assert!(persist_calls.load(Ordering::SeqCst) > calls_after_create);
    let calls_after_rotate = persist_calls.load(Ordering::SeqCst);
    {
        let snapshot = last_snapshot.lock().expect("snapshot lock");
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].current_epoch_id, rotated.current_epoch_id);
        assert!(
            snapshot[0]
                .archived_epochs
                .iter()
                .any(|epoch| epoch.epoch_id == channel.current_epoch_id),
            "pre-rotate epoch must be archived in the persisted snapshot"
        );
    }

    // remove 経由(leave)で発火し、スナップショットから消える。
    app.leave_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("leave private channel");
    assert!(persist_calls.load(Ordering::SeqCst) > calls_after_rotate);
    assert!(
        last_snapshot.lock().expect("snapshot lock").is_empty(),
        "leave must persist the removal"
    );
}

/// callback 未接続なら registry 変異は従来どおり成功する(テスト・harness 互換)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_mutations_succeed_without_persist_callback() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let topic = "kukuri:topic:persist-callback-none";
    let _ = app.list_timeline(topic, None, 20).await;
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "no-callback".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel without callback");
    app.leave_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("leave private channel without callback");
}

/// persist 失敗は変異コマンドのエラーとして伝播する(rollback はしない — 現行同等)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persist_failure_propagates_from_mutation() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    app.set_private_channel_capability_persist(Arc::new(|_| {
        Box::pin(async { anyhow::bail!("fake persist failure") })
    }));

    let topic = "kukuri:topic:persist-callback-fail";
    let _ = app.list_timeline(topic, None, 20).await;
    let error = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "persist-fail".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect_err("persist failure must propagate");
    assert!(
        format!("{error:#}").contains("fake persist failure"),
        "unexpected error: {error:#}"
    );
}
