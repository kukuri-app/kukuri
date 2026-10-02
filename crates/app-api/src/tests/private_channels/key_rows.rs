//! #1218 AC-4b: private channel の鍵を (channel, epoch) ごとの行で持ち、各操作は対象の行だけを読み書きする
//! (ADR 0061 §9)。他の channel と対象の channel の過去の世代を 10 倍にしても、読み書きする行数が変わらない。

use super::super::*;

use crate::PrivateChannelCapability;
use crate::service::remote_read_support::private_epochs_for_bucket;
use kukuri_docs_sync::TimeBucket;

const TOPIC: &str = "kukuri:topic:key-rows";
const OTHER_TOPIC: &str = "kukuri:topic:key-rows-other";

/// 購読の task は背景で行を読むので、測る topic の gossip を止めて task を起こさない(lease とメモリは今と同じ)。
async fn device(
    store: &Arc<MemoryStore>,
    docs: &Arc<MemoryDocsSync>,
    keys: &KukuriKeys,
) -> AppService {
    let transport = Arc::new(FakeTransport::new("device", FakeNetwork::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    );
    app.gossip_disabled_topics
        .lock()
        .await
        .extend([TOPIC.to_string(), OTHER_TOPIC.to_string()]);
    app
}

fn epoch(day: usize) -> String {
    format!("epoch-{}-x", day * 86_400_000)
}

fn secret(day: usize) -> [u8; 32] {
    [u8::try_from(day % 250).expect("byte") + 1; 32]
}

/// 旧 registry の 1 件。過去の世代を `archived` 件持ち、担当の欄が無い。
fn capability(
    topic: &str,
    channel: &str,
    owner: &str,
    archived: usize,
) -> PrivateChannelCapability {
    PrivateChannelCapability {
        topic_id: topic.into(),
        channel_id: channel.into(),
        label: channel.into(),
        creator_pubkey: owner.into(),
        owner_pubkey: owner.into(),
        joined_via_pubkey: None,
        audience_kind: ChannelAudienceKind::InviteOnly,
        current_epoch_id: epoch(archived + 1),
        current_epoch_secret_hex: hex::encode(secret(archived + 1)),
        archived_epochs: (1..=archived)
            .map(|day| PrivateChannelEpochCapability {
                epoch_id: epoch(day),
                namespace_secret_hex: hex::encode(secret(day)),
            })
            .collect(),
        rotation_required: false,
        participant_count: 0,
        stale_participant_count: 0,
        namespace_secret_hex: String::new(),
        controller: None,
    }
}

/// 各操作が読み書きした鍵と参加の行の数。
async fn measure(scale: usize) -> Vec<(&'static str, usize)> {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(MemoryDocsSync::default());
    let keys = generate_keys();
    let owner = keys.public_key_hex();
    let first = device(&store, &docs, &keys).await;
    // 他の topic の参加中の channel(lease の上限を超える数)を scale 倍にする。
    for index in 0..200 * scale {
        let other = capability(OTHER_TOPIC, &format!("other-{index:05}"), &owner, 2);
        let state = joined_private_channel_state_from_capability(other.clone()).expect("state");
        first
            .persist_private_channel(&state, 0, &other.archived_epochs)
            .await
            .expect("other channel rows");
    }
    // 対象の channel の過去の世代を scale 倍にする。
    let archived = 8 * scale;
    first
        .restore_private_channel_capability(capability(TOPIC, "target", &owner, archived))
        .await
        .expect("target channel rows");
    let touched = || store.private_channel_key_rows_touched();
    let mut counts = Vec::new();

    let before = touched();
    let created = first
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "joined".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("join");
    counts.push(("join", touched() - before));

    let before = touched();
    first
        .rotate_private_channel(TOPIC, "target")
        .await
        .expect("rotate");
    counts.push(("rotate", touched() - before));

    let before = touched();
    let receive_key = kukuri_core::receive_epoch_key_id(&secret(1), "target", &epoch(1)).unwrap();
    let (_, epoch_id, _) = first
        .services
        .private_channel_epoch_by_receive_key(&receive_key)
        .await
        .expect("offer lookup")
        .expect("archived epoch");
    assert_eq!(epoch_id, epoch(1));
    counts.push(("offer", touched() - before));

    let state = first
        .joined_private_channel_state(TOPIC, "target")
        .await
        .expect("target");
    let before = touched();
    let epochs = private_epochs_for_bucket(
        &first.services,
        &state,
        TimeBucket::from_unix_seconds(3 * 86_400 + 60).expect("bucket"),
    )
    .await
    .expect("bucket epochs");
    assert_eq!(epochs[0].0, epoch(3));
    counts.push(("bucket", touched() - before));

    let before = touched();
    assert_eq!(
        first
            .joined_private_channel_states_for_topic(TOPIC, "")
            .await
            .expect("list")
            .0
            .len(),
        2
    );
    counts.push(("list", touched() - before));

    let before = touched();
    first
        .migrate_legacy_private_channel_participants("")
        .await
        .expect("owner walk");
    counts.push(("owner walk", touched() - before));

    let before = touched();
    first
        .leave_private_channel(TOPIC, &created.channel_id)
        .await
        .expect("leave");
    counts.push(("leave", touched() - before));

    // 再起動: lease を取る channel の行だけを読む(上限を超える参加は読まない)。
    let second = device(&store, &docs, &keys).await;
    let before = touched();
    second
        .restore_joined_private_channels()
        .await
        .expect("restore");
    counts.push(("restore", touched() - before));

    // 鍵を更新して再起動した後も、過去の世代の replica を開ける(相手への private の応答と同じ参照)。
    let before = touched();
    assert!(
        docs.query_replica(
            &private_channel_epoch_replica_id("target", &epoch(1)),
            DocQuery::Exact("k".into()),
        )
        .await
        .is_ok(),
        "the archived epoch replica opens after the restart"
    );
    counts.push(("peer read", touched() - before));
    counts
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_operation_touches_only_its_rows_regardless_of_other_channels_and_epochs() {
    let small = measure(1).await;
    let large = measure(10).await;
    assert_eq!(small, large);
}

/// topic の参加中の channel の一覧は 128 件の page で、続きは cursor から読める(2026-10-02 ユーザー判断)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn joined_channel_list_continues_from_the_cursor() {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(MemoryDocsSync::default());
    let keys = generate_keys();
    let app = device(&store, &docs, &keys).await;
    app.install_private_epoch_secrets()
        .await
        .expect("install private epoch secrets");
    for index in 0..130 {
        let channel = capability(
            TOPIC,
            &format!("channel-{index:03}"),
            &keys.public_key_hex(),
            0,
        );
        let state = joined_private_channel_state_from_capability(channel).expect("state");
        app.persist_private_channel(&state, 0, &[])
            .await
            .expect("channel rows");
    }
    let first = app
        .list_joined_private_channels(TOPIC, None)
        .await
        .expect("first page");
    assert_eq!(first.items.len(), 128);
    let second = app
        .list_joined_private_channels(TOPIC, first.next_cursor.as_deref())
        .await
        .expect("second page");
    assert_eq!(
        second
            .items
            .iter()
            .map(|view| view.channel_id.as_str())
            .collect::<Vec<_>>(),
        ["channel-128", "channel-129"]
    );
    assert_eq!(second.next_cursor, None);
}
