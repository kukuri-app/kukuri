//! #1219 W6 AC-2: 鍵更新を、予約した新しい世代(操作 ID)へ束縛する。各書込みの境界での失敗・再起動・同じ操作の
//! 再送は同じ世代を再開し、配布は参加者の表の 128 件の page と保存した cursor で背景に進む。受付・配布の 1 step・
//! view の仕事は、対象外の参加者・完了した鍵更新・参加者の数に比例しない。

use super::super::*;

use kukuri_docs_sync::{DocEventStream, PrivateEpochSecrets};
use kukuri_store::{
    PrivateChannelEpochRange, PrivateChannelKeyStore, PrivateChannelParticipantRow,
};
use std::sync::atomic::AtomicUsize;

const TOPIC: &str = "kukuri:topic:private-rotation";
const OTHER_TOPIC: &str = "kukuri:topic:private-rotation-other";

/// `fail_at` 番目の docs の書込みと、`failing_replica` への書込みを失敗させる docs(0・`None` なら失敗させない)。
#[derive(Default)]
struct FaultDocs {
    inner: MemoryDocsSync,
    writes: AtomicUsize,
    fail_at: AtomicUsize,
    failing_replica: std::sync::Mutex<Option<ReplicaId>>,
}

#[async_trait]
impl DocsSync for FaultDocs {
    async fn has_local_replica(&self, replica: &ReplicaId) -> Result<bool> {
        self.inner.has_local_replica(replica).await
    }
    async fn close_replica(&self, replica: &ReplicaId) -> Result<()> {
        self.inner.close_replica(replica).await
    }
    async fn open_replica(&self, replica: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica).await
    }
    async fn apply_doc_op(&self, replica: &ReplicaId, op: DocOp) -> Result<()> {
        let sequence = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        if sequence == self.fail_at.load(Ordering::SeqCst)
            || self.failing_replica.lock().unwrap().as_ref() == Some(replica)
        {
            anyhow::bail!("injected docs write failure");
        }
        self.inner.apply_doc_op(replica, op).await
    }
    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.inner
            .query_replica_with_policy(replica, query, policy)
            .await
    }
    async fn query_replica_keys(
        &self,
        replica: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica, query).await
    }
    async fn subscribe_replica(&self, replica: &ReplicaId) -> Result<DocEventStream> {
        self.inner.subscribe_replica(replica).await
    }
    async fn register_private_replica_secret(
        &self,
        replica: &ReplicaId,
        secret_hex: &str,
    ) -> Result<()> {
        self.inner
            .register_private_replica_secret(replica, secret_hex)
            .await
    }
    async fn remove_private_replica_secret(&self, replica: &ReplicaId) -> Result<()> {
        self.inner.remove_private_replica_secret(replica).await
    }
    async fn install_private_epoch_secrets(
        &self,
        source: Arc<dyn PrivateEpochSecrets>,
    ) -> Result<()> {
        self.inner.install_private_epoch_secrets(source).await
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

/// owner の端末(同じ store・docs・blob で作り直すと再起動)。端末 ID は固定で、担当のまま。
async fn owner_device(
    store: &Arc<MemoryStore>,
    docs: &Arc<dyn DocsSync>,
    blobs: &Arc<MemoryBlobService>,
    keys: &KukuriKeys,
) -> AppService {
    let transport = Arc::new(FakeTransport::new("owner-device", FakeNetwork::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs.clone(),
        blobs.clone(),
        keys.clone(),
    );
    app.gossip_disabled_topics
        .lock()
        .await
        .extend([TOPIC.to_string(), OTHER_TOPIC.to_string()]);
    app.restore_joined_private_channels()
        .await
        .expect("restore joined channels");
    // account の replica の秘密だけを登録する（差分の取得・送り直しの背景の task は起こさない。背景の送り直しが
    // 未書込みの過去の世代を読み、鍵更新の受付の計測に混ざらないように。ADR 0061 §10）。
    super::super::account_sync::register_account_replica(&app).await;
    app
}

/// 公開鍵が `f` で始まる owner の鍵(参加者の公開鍵より後に並び、配布の page に入らない)。
fn owner_keys() -> KukuriKeys {
    loop {
        let keys = generate_keys();
        if keys.public_key_hex().starts_with('f') {
            return keys;
        }
    }
}

/// 公開鍵が `f` で始まらない参加者の公開鍵を `count` 件。
fn participant_pubkeys(count: usize) -> Vec<String> {
    std::iter::repeat_with(|| generate_keys().public_key_hex())
        .filter(|pubkey| !pubkey.starts_with('f'))
        .take(count)
        .collect()
}

async fn create(app: &AppService, topic: &str, audience_kind: ChannelAudienceKind) -> String {
    app.create_private_channel(CreatePrivateChannelInput {
        topic_id: TopicId::new(topic),
        label: "rotation".into(),
        audience_kind,
    })
    .await
    .expect("create private channel")
    .channel_id
}

async fn join(store: &MemoryStore, channel_id: &str, epoch_id: &str, pubkey: &str, left: bool) {
    store
        .put_private_channel_participant(PrivateChannelParticipantRow {
            channel_id: channel_id.into(),
            epoch_id: epoch_id.into(),
            participant_pubkey: pubkey.into(),
            left_at: left.then_some(2),
            updated_at: if left { 2 } else { 1 },
        })
        .await
        .expect("participant row");
}

async fn mutual(store: &MemoryStore, owner: &str, pubkey: &str) {
    for (subject, target) in [(owner, pubkey), (pubkey, owner)] {
        store
            .upsert_follow_edge(FollowEdge {
                subject_pubkey: subject.into(),
                target_pubkey: target.into(),
                status: FollowEdgeStatus::Active,
                updated_at: 1,
                envelope_id: EnvelopeId::from(format!("follow-{subject}-{target}")),
            })
            .await
            .expect("follow edge");
    }
}

/// 背景の step を、終わっていない鍵更新が無くなるまで回す(回数に上限を置く)。
async fn finish_steps(app: &AppService) -> usize {
    let mut cursor = Default::default();
    for steps in 0..64 {
        if app
            .services
            .projection_store
            .list_private_channel_rotations(("", ""), 1)
            .await
            .unwrap()
            .is_empty()
        {
            return steps;
        }
        AppService::step_private_channel_rotations(&app.services, &mut cursor)
            .await
            .expect("rotation step");
    }
    panic!("the rotation does not finish");
}

/// C1・C3: 鍵更新の docs の各書込みで失敗させ、再起動して同じ操作を再送(確定前に失敗したとき)・背景の step を
/// 回すと、新しい世代は 1 つだけで、参加者は全員その世代の grant を受け取り、終わった鍵更新は印が消える。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_write_boundary_resumes_the_same_new_epoch() {
    let participants = (0..3).map(|_| generate_keys()).collect::<Vec<_>>();
    let mut fault_reached = true;
    let mut fail_at = 0;
    while fault_reached {
        fail_at += 1;
        let store = Arc::new(MemoryStore::default());
        let fault = Arc::new(FaultDocs::default());
        let docs: Arc<dyn DocsSync> = fault.clone();
        let blobs = Arc::new(MemoryBlobService::default());
        let keys = generate_keys();
        let owner = owner_device(&store, &docs, &blobs, &keys).await;
        let channel_id = create(&owner, TOPIC, ChannelAudienceKind::InviteOnly).await;
        let original = owner
            .joined_private_channel_state(TOPIC, &channel_id)
            .await
            .expect("joined");
        for participant in &participants {
            join(
                &store,
                &channel_id,
                &original.current_epoch_id,
                &participant.public_key_hex(),
                false,
            )
            .await;
        }
        let base = fault.writes.load(Ordering::SeqCst);
        fault.fail_at.store(base + fail_at, Ordering::SeqCst);
        let first = owner.rotate_private_channel(TOPIC, &channel_id).await;
        fault_reached = fault.writes.load(Ordering::SeqCst) >= base + fail_at;
        fault.fail_at.store(0, Ordering::SeqCst);
        drop(owner);

        // 再起動。確定前に失敗した操作は、同じ操作の再送で再開する。
        let owner = owner_device(&store, &docs, &blobs, &keys).await;
        if first.is_err() {
            owner
                .rotate_private_channel(TOPIC, &channel_id)
                .await
                .unwrap_or_else(|error| panic!("fault {fail_at}: resend failed: {error:#}"));
        }
        finish_steps(&owner).await;

        let epochs = store
            .list_private_channel_epochs(
                &channel_id,
                PrivateChannelEpochRange::After(epoch_started_at(&original.current_epoch_id)),
                8,
            )
            .await
            .unwrap();
        assert_eq!(epochs.len(), 1, "fault {fail_at}: one new epoch");
        let new_epoch = &epochs[0].epoch_id;
        let state = owner
            .joined_private_channel_state(TOPIC, &channel_id)
            .await
            .expect("joined");
        assert_eq!(&state.current_epoch_id, new_epoch, "fault {fail_at}");
        for participant in &participants {
            let grant = fetch_private_channel_epoch_handoff_grant_from_replica(
                owner.docs_sync(),
                &current_private_channel_replica_id(&original),
                &participant.public_key_hex(),
                DocFetchPolicy::LocalOnly,
            )
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("fault {fail_at}: grant for every participant"));
            let payload = decrypt_private_channel_epoch_handoff_grant(participant, &grant).unwrap();
            assert_eq!(&payload.new_epoch_id, new_epoch, "fault {fail_at}");
        }
        assert!(
            store
                .list_private_channel_rotations(("", ""), 1)
                .await
                .unwrap()
                .is_empty(),
            "fault {fail_at}: the finished rotation leaves no record"
        );
    }
    assert!(fail_at > 6, "the faults cover every docs write: {fail_at}");
}

/// C1・C3: 確定の後、account 同期への記録と印の更新の間で止まった操作(store の書込みの境界)は、再起動の後の背景の
/// step が同じ世代の鍵を account 同期へ書いてから配布する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_rotation_without_its_cursor_writes_the_account_item_and_distributes() {
    let store = Arc::new(MemoryStore::default());
    let fault = Arc::new(FaultDocs::default());
    let docs: Arc<dyn DocsSync> = fault.clone();
    let blobs = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let participant = generate_keys();
    let owner = owner_device(&store, &docs, &blobs, &keys).await;
    let channel_id = create(&owner, TOPIC, ChannelAudienceKind::InviteOnly).await;
    let original = owner
        .joined_private_channel_state(TOPIC, &channel_id)
        .await
        .expect("joined");
    // account 同期への記録だけを失敗させる(失敗しても受付は続く)。
    *fault.failing_replica.lock().unwrap() = Some(keys.derive_account_sync().replica_id().clone());
    let rotated = owner
        .rotate_private_channel(TOPIC, &channel_id)
        .await
        .expect("rotate");
    let key = kukuri_core::AccountSyncItemKey::ChannelCapability {
        channel_id: ChannelId::new(&channel_id),
        epoch_id: rotated.current_epoch_id.clone(),
    };
    assert!(
        owner
            .read_account_sync_item(&key, DocFetchPolicy::LocalOnly)
            .await
            .unwrap()
            .is_none()
    );
    // 確定と印の更新の間で止まった状態: 新しい世代の行に、配布前の印を戻す。
    let mut row = store
        .get_private_channel_epoch(&channel_id, &rotated.current_epoch_id)
        .await
        .unwrap()
        .unwrap();
    row.rotation_from = Some(original.current_epoch_id.clone());
    let from = store
        .get_private_channel_epoch(&channel_id, &original.current_epoch_id)
        .await
        .unwrap()
        .unwrap();
    store
        .delete_private_channel_epochs(&channel_id, 8)
        .await
        .unwrap();
    store.put_private_channel_epoch(&from).await.unwrap();
    store.put_private_channel_epoch(&row).await.unwrap();
    join(
        &store,
        &channel_id,
        &original.current_epoch_id,
        &participant.public_key_hex(),
        false,
    )
    .await;
    drop(owner);
    *fault.failing_replica.lock().unwrap() = None;

    let owner = owner_device(&store, &docs, &blobs, &keys).await;
    assert_eq!(finish_steps(&owner).await, 1);
    assert!(
        owner
            .read_account_sync_item(&key, DocFetchPolicy::LocalOnly)
            .await
            .unwrap()
            .is_some(),
        "the new epoch reaches the other devices"
    );
    let grant = fetch_private_channel_epoch_handoff_grant_from_replica(
        owner.docs_sync(),
        &current_private_channel_replica_id(&original),
        &participant.public_key_hex(),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .unwrap()
    .expect("grant");
    assert_eq!(
        decrypt_private_channel_epoch_handoff_grant(&participant, &grant)
            .unwrap()
            .new_epoch_id,
        rotated.current_epoch_id
    );
}

/// C1: 同じ端末で同時に受け付けた鍵更新(例: 2 つの共有の前の auto rotate)は、予約を共有するか順に並ぶ。どの
/// 新しい世代も 1 本の鎖(policy の `previous_epoch_id`)に載り、確定されない予約が背景で配られることは無い。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_rotations_on_one_device_stay_on_one_chain() {
    let store = Arc::new(MemoryStore::default());
    let docs: Arc<dyn DocsSync> = Arc::new(FaultDocs::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let owner = owner_device(&store, &docs, &blobs, &generate_keys()).await;
    let channel_id = create(&owner, TOPIC, ChannelAudienceKind::InviteOnly).await;
    let original = owner
        .joined_private_channel_state(TOPIC, &channel_id)
        .await
        .expect("joined");
    let (first, second) = tokio::join!(
        owner.rotate_private_channel(TOPIC, &channel_id),
        owner.rotate_private_channel(TOPIC, &channel_id),
    );
    let returned =
        [first.expect("first"), second.expect("second")].map(|view| view.current_epoch_id);
    finish_steps(&owner).await;
    let epochs = store
        .list_private_channel_epochs(
            &channel_id,
            PrivateChannelEpochRange::After(epoch_started_at(&original.current_epoch_id)),
            8,
        )
        .await
        .unwrap();
    let mut previous = original.current_epoch_id.clone();
    for epoch in &epochs {
        let policy = fetch_private_channel_policy_from_replica(
            owner.docs_sync(),
            &private_channel_epoch_replica_id(&channel_id, &epoch.epoch_id),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .unwrap()
        .expect("every new epoch is seeded");
        assert_eq!(policy.previous_epoch_id.as_ref(), Some(&previous));
        previous = epoch.epoch_id.clone();
    }
    assert!(
        returned
            .iter()
            .all(|epoch| epochs.iter().any(|row| &row.epoch_id == epoch))
    );
    let current = owner
        .joined_private_channel_state(TOPIC, &channel_id)
        .await
        .expect("joined")
        .current_epoch_id;
    assert_eq!(
        current, previous,
        "the newest epoch on the chain is current"
    );
}

/// C2: 受付は配布の最初の 1 page(128 件)まで。残りは背景の step が保存した cursor から 1 page ずつ進め、終端で印を消す。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_rotation_distributes_one_page_per_step_from_the_saved_cursor() {
    let store = Arc::new(MemoryStore::default());
    let docs: Arc<dyn DocsSync> = Arc::new(FaultDocs::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let keys = owner_keys();
    let owner = owner_device(&store, &docs, &blobs, &keys).await;
    let channel_id = create(&owner, TOPIC, ChannelAudienceKind::InviteOnly).await;
    let epoch = owner
        .joined_private_channel_state(TOPIC, &channel_id)
        .await
        .expect("joined")
        .current_epoch_id;
    let mut pubkeys = participant_pubkeys(130);
    pubkeys.sort();
    for pubkey in &pubkeys {
        join(&store, &channel_id, &epoch, pubkey, false).await;
    }
    let rotated = owner
        .rotate_private_channel(TOPIC, &channel_id)
        .await
        .expect("rotate");
    assert_ne!(rotated.current_epoch_id, epoch, "the new epoch is current");
    assert_eq!(store.list_direct_message_outbox().await.unwrap().len(), 128);
    let pending = store
        .list_private_channel_rotations(("", ""), 8)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].rotation_after.as_ref(), Some(&pubkeys[127]));

    assert_eq!(finish_steps(&owner).await, 1);
    assert_eq!(store.list_direct_message_outbox().await.unwrap().len(), 130);
}

/// 受付・配布の 1 step で触った行の数、積んだ outbox の行、その bytes。
async fn measure_rotation(scale: usize) -> Vec<(&'static str, usize, usize, usize)> {
    let store = Arc::new(MemoryStore::default());
    let docs: Arc<dyn DocsSync> = Arc::new(FaultDocs::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let keys = owner_keys();
    let owner_pubkey = keys.public_key_hex();
    let owner = owner_device(&store, &docs, &blobs, &keys).await;
    let channel_id = create(&owner, TOPIC, ChannelAudienceKind::FriendOnly).await;
    let state = owner
        .joined_private_channel_state(TOPIC, &channel_id)
        .await
        .expect("joined");
    // 対象の参加者は 2 page 以上で、全員 mutual。
    let targets = participant_pubkeys(256 * scale);
    for pubkey in &targets {
        mutual(&store, &owner_pubkey, pubkey).await;
        join(&store, &channel_id, &state.current_epoch_id, pubkey, false).await;
    }
    // 対象外: 退出した参加者と、他の channel の参加者。
    for index in 0..50 * scale {
        join(
            &store,
            &channel_id,
            &state.current_epoch_id,
            &format!("ff{index:062x}"),
            true,
        )
        .await;
    }
    let other = create(&owner, OTHER_TOPIC, ChannelAudienceKind::InviteOnly).await;
    for index in 0..200 * scale {
        join(
            &store,
            &other,
            "other-epoch",
            &format!("ee{index:062x}"),
            false,
        )
        .await;
    }
    // 完了した鍵更新の履歴(過去の世代の鍵の行)。
    let archived = (1..=8 * scale)
        .map(|day| PrivateChannelEpochCapability {
            epoch_id: format!("epoch-{day}-x"),
            namespace_secret_hex: hex::encode([1_u8; 32]),
        })
        .collect::<Vec<_>>();
    owner
        .persist_private_channel(&state, 1, &archived, false)
        .await
        .expect("archived epochs");
    // 完了した鍵更新ごとに参加者が redeem して残した、過去の世代の参加の行。
    for epoch in &archived {
        for pubkey in targets.iter().take(128) {
            join(&store, &channel_id, &epoch.epoch_id, pubkey, false).await;
        }
    }
    let touched = || store.private_channel_key_rows_touched();
    let outbox = || async {
        let rows = store.list_direct_message_outbox().await.unwrap();
        let mut bytes = 0;
        for row in &rows {
            bytes += blobs
                .fetch_local_blob(&row.frame_blob_hash)
                .await
                .unwrap()
                .expect("grant payload")
                .len();
        }
        (rows.len(), bytes)
    };
    let mut counts = Vec::new();
    let (rows, bytes) = outbox().await;
    let before = touched();
    owner
        .rotate_private_channel(TOPIC, &channel_id)
        .await
        .expect("rotate");
    let (after_rows, after_bytes) = outbox().await;
    counts.push((
        "accept",
        touched() - before,
        after_rows - rows,
        after_bytes - bytes,
    ));
    let before = touched();
    AppService::step_private_channel_rotations(&owner.services, &mut Default::default())
        .await
        .expect("step");
    let (step_rows, step_bytes) = outbox().await;
    counts.push((
        "step",
        touched() - before,
        step_rows - after_rows,
        step_bytes - after_bytes,
    ));
    counts
}

/// C4: 対象外の参加者・完了した鍵更新の履歴・対象の参加者の数を 10 倍にしても、受付と配布の 1 step の仕事は同じ。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_work_does_not_grow_with_participants_or_history() {
    let small = measure_rotation(1).await;
    let large = measure_rotation(10).await;
    assert_eq!(small[0].2, 128, "{small:?}");
    assert_eq!(small, large);
}

/// view(投稿・共有の前の auto rotate の判定と同じ数を読む)で触る行の数、資格喪失の数、判定で鍵を更新したか。
async fn measure_view(scale: usize) -> (usize, usize, usize) {
    let store = Arc::new(MemoryStore::default());
    let docs: Arc<dyn DocsSync> = Arc::new(FaultDocs::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let keys = owner_keys();
    let owner_pubkey = keys.public_key_hex();
    let owner = owner_device(&store, &docs, &blobs, &keys).await;
    let channel_id = create(&owner, TOPIC, ChannelAudienceKind::FriendOnly).await;
    let state = owner
        .joined_private_channel_state(TOPIC, &channel_id)
        .await
        .expect("joined");
    for pubkey in participant_pubkeys(200 * scale) {
        mutual(&store, &owner_pubkey, &pubkey).await;
        join(&store, &channel_id, &state.current_epoch_id, &pubkey, false).await;
    }
    // 資格喪失: owner からの follow だけの参加者と、関係の無い参加者。
    let stale = participant_pubkeys(2);
    store
        .upsert_follow_edge(FollowEdge {
            subject_pubkey: owner_pubkey.as_str().into(),
            target_pubkey: stale[0].as_str().into(),
            status: FollowEdgeStatus::Active,
            updated_at: 1,
            envelope_id: EnvelopeId::from("follow-one-sided"),
        })
        .await
        .expect("follow edge");
    for pubkey in stale {
        join(&store, &channel_id, &state.current_epoch_id, &pubkey, false).await;
    }
    // view の数と、投稿の前の判定(同じ数を読む)。判定の後の鍵更新の仕事は C4 が測る。
    let before = store.private_channel_key_rows_touched();
    let view = owner
        .joined_private_channel_view_for_state(&state)
        .await
        .expect("view");
    let touched = store.private_channel_key_rows_touched() - before;
    owner
        .maybe_auto_rotate_private_channel_for_owner(
            TOPIC,
            &state.channel_id,
            PrivateChannelOwnerAction::Write,
        )
        .await
        .expect("auto rotate before a write");
    assert_eq!(view.participant_count, Some(200 * scale + 3));
    assert!(view.rotation_required);
    (
        touched,
        view.stale_participant_count,
        usize::from(
            owner
                .joined_private_channel_state(TOPIC, &channel_id)
                .await
                .expect("joined")
                .current_epoch_id
                != state.current_epoch_id,
        ),
    )
}

/// C5: 参加者を 10 倍にしても、view と auto rotate の判定は同じ数の行を読み、資格喪失(mutual でない)を数え、
/// 資格喪失者がいれば投稿の前に鍵を更新する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_view_and_the_auto_rotate_decision_do_not_grow_with_participants() {
    let small = measure_view(1).await;
    assert_eq!(small.1, 2, "both non-mutual participants are stale");
    assert_eq!(small.2, 1, "the owner rotates before writing");
    assert_eq!(small, measure_view(10).await);
}
