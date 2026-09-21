//! #1239: session(live / game)と reaction が、replica の走査なしで反映されることを固定する。
//!
//! 独立監査(PR #1267)が「直接の test が無い」とした経路(session と reaction の読み直し、hint からの依頼、
//! 同期の終わりの通知の扱い)を含む。

use super::range_reconcile::{NoScanDocsSync, is_projected};
use super::*;
use kukuri_docs_sync::{ReplicaNotice, ReplicaNoticeStream};

/// docs の通知を test から流し込む docs。entry の event は購読側へ届けない。prefix の読み出しは失敗させる。
#[derive(Clone)]
struct SilentNoScanDocsSync {
    inner: NoScanDocsSync,
    notices: tokio::sync::broadcast::Sender<ReplicaNotice>,
}

impl Default for SilentNoScanDocsSync {
    fn default() -> Self {
        Self {
            inner: NoScanDocsSync::default(),
            notices: tokio::sync::broadcast::channel(16).0,
        }
    }
}

#[async_trait]
impl DocsSync for SilentNoScanDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        self.inner
            .query_replica_with_policy(replica_id, query, policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica_id, query).await
    }

    async fn subscribe_replica(
        &self,
        _replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        Ok(Box::pin(futures_util::stream::pending()))
    }

    async fn subscribe_replica_notices(
        &self,
        _replica_id: &ReplicaId,
    ) -> Result<ReplicaNoticeStream> {
        let stream = tokio_stream::wrappers::BroadcastStream::new(self.notices.subscribe());
        Ok(Box::pin(futures_util::StreamExt::filter_map(
            stream,
            |item| async move { item.ok().map(Ok) },
        )))
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

struct Pair {
    author: AppService,
    viewer: AppService,
    viewer_store: Arc<MemoryStore>,
    transport: Arc<StaticTransport>,
}

/// 同じ docs と blob を共有する、書く側と読む側。読む側の projection は空。
fn pair(docs_sync: Arc<dyn DocsSync>) -> Pair {
    let blob_service = Arc::new(MemoryBlobService::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    // 書く側は hint を流さない(読む側が hint の個別反映で先に反映してしまわないように)。
    let app = |store: Arc<MemoryStore>, hints: Arc<dyn HintTransport>| {
        app_service_from_dependencies(
            store.clone(),
            store,
            transport.clone(),
            hints,
            docs_sync.clone(),
            blob_service.clone(),
            generate_keys(),
        )
    };
    let viewer_store = Arc::new(MemoryStore::default());
    Pair {
        author: app(
            Arc::new(MemoryStore::default()),
            Arc::new(NoopHintTransport),
        ),
        viewer: app(viewer_store.clone(), transport.clone()),
        viewer_store,
        transport,
    }
}

fn game_room_input(title: &str) -> CreateGameRoomInput {
    CreateGameRoomInput {
        title: title.into(),
        description: String::new(),
        participants: vec!["Alice".into(), "Bob".into()],
    }
}

// S-9: game room と live session の一覧は、行が無くても replica を走査しない。session の固定件数だけを
// key の一覧から反映する(prefix の読み出しを失敗させる docs で確かめる)。
#[tokio::test]
async fn session_lists_are_filled_without_a_replica_scan() {
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let pair = pair(docs_sync);
    let topic = "kukuri:topic:session-list-no-scan";
    let room_id = pair
        .author
        .create_game_room(topic, game_room_input("a room"))
        .await
        .expect("create game room");
    let session_id = pair
        .author
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "a session".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");

    let rooms = pair
        .viewer
        .list_game_rooms(topic)
        .await
        .expect("the game room list must not need a replica scan");
    assert!(rooms.iter().any(|room| room.room_id == room_id));
    let sessions = pair
        .viewer
        .list_live_sessions(topic)
        .await
        .expect("the live session list must not need a replica scan");
    assert!(
        sessions
            .iter()
            .any(|session| session.session_id == session_id)
    );
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}

// 購読タスクの追いつきは、session も反映する(一覧を取得しなくても、projection に入る)。
#[tokio::test]
async fn subscription_start_projects_the_sessions_of_the_replica() {
    let docs_sync = Arc::new(SilentNoScanDocsSync::default());
    let pair = pair(docs_sync);
    let topic = "kukuri:topic:session-catch-up-start";
    let room_id = pair
        .author
        .create_game_room(topic, game_room_input("a room"))
        .await
        .expect("create game room");
    pair.viewer
        .ensure_topic_subscription(topic)
        .await
        .expect("subscribe");
    let projection_store: &dyn ProjectionStore = pair.viewer_store.as_ref();
    timeout(Duration::from_secs(10), async {
        while !projection_store
            .list_topic_game_rooms(topic)
            .await
            .expect("rooms")
            .iter()
            .any(|row| row.room_id == room_id)
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the catch-up at start must project the game room");
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}

// 取りこぼし(`Lagged`)の後の追いつきは、窓の object の reaction も読み直す。
#[tokio::test]
async fn lagged_notice_refreshes_the_reactions_of_the_window() {
    let docs_sync = Arc::new(SilentNoScanDocsSync::default());
    let pair = pair(docs_sync.clone());
    let topic = "kukuri:topic:session-catch-up-reactions";
    let replica = topic_replica_id(topic);
    let target = pair
        .author
        .create_post(topic, "a post", None)
        .await
        .expect("create post");
    pair.viewer
        .ensure_topic_subscription(topic)
        .await
        .expect("subscribe");
    let target_id = EnvelopeId::from(target.as_str());
    let projection_store: &dyn ProjectionStore = pair.viewer_store.as_ref();
    timeout(Duration::from_secs(10), async {
        while projection_store
            .get_object_projection(&target_id)
            .await
            .expect("projection")
            .is_none()
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the post is projected at start");

    // 購読の開始後に付いた reaction。event は購読側へ届いていない。
    pair.author
        .toggle_reaction(
            topic,
            target.as_str(),
            ReactionKeyV1::Emoji {
                emoji: "👍".into()
            },
            Some(ChannelRef::Public),
        )
        .await
        .expect("toggle reaction");
    sleep(Duration::from_millis(300)).await;
    assert!(
        projection_store
            .list_reaction_cache_for_target(&replica, &target_id)
            .await
            .expect("reaction rows")
            .is_empty()
    );
    docs_sync
        .notices
        .send(ReplicaNotice::Lagged { missed: 2 })
        .expect("a subscriber is listening");
    timeout(Duration::from_secs(10), async {
        while projection_store
            .list_reaction_cache_for_target(&replica, &target_id)
            .await
            .expect("reaction rows")
            .is_empty()
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the catch-up after a lag must refresh the reactions of the window");
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}

// TR-4: docs の同期より先に hint が届いた(個別反映が 0 件)。走査はせず、追いつきを依頼する。
// 対象が後から手元に入れば、依頼した追いつきが反映する。
#[tokio::test]
async fn a_hint_that_arrives_before_the_docs_requests_a_catch_up() {
    let docs_sync = Arc::new(SilentNoScanDocsSync::default());
    let pair = pair(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:session-catch-up-hint");
    pair.viewer
        .ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(300)).await;

    pair.transport
        .publish_hint(
            &channel_hint_topic_for(topic.as_str(), None),
            GossipHint::TopicObjectsChanged {
                topic_id: topic.clone(),
                objects: vec![HintObjectRef {
                    object_id: "not-synced-yet".into(),
                    object_kind: "post".into(),
                    docs_author: None,
                }],
            },
        )
        .await
        .expect("publish hint");
    // hint の直後に、投稿が手元の docs に入った(event は購読側へ届いていない)。
    let post = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &topic_replica_id(topic.as_str()),
        &generate_keys(),
        &topic,
        super::range_reconcile::BASE_TIME,
        "arrived after the hint",
        None,
    )
    .await;
    timeout(Duration::from_secs(8), async {
        while !is_projected(pair.viewer_store.as_ref(), &post).await {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the hint miss must request a catch-up");
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}

// 同期の終わり(`SyncFinished`)は、反映するものがあるとは限らない契機。空振りで伸びた間隔をそのまま待つ
// (静かな replica では再 sync のたびに届くので、それだけで追いつきを繰り返さない)。
#[tokio::test]
async fn sync_finished_waits_for_the_interval_stretched_by_empty_runs() {
    let docs_sync = Arc::new(SilentNoScanDocsSync::default());
    let pair = pair(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:session-catch-up-sync-finished-backoff");
    pair.viewer
        .ensure_topic_subscription(topic.as_str())
        .await
        .expect("subscribe");
    sleep(Duration::from_millis(300)).await;
    // 空振りの追いつきを 2 回(約 1 秒後と、その 3 秒後)。次に追いつけるのは、さらに 6 秒後。
    for wait_ms in [1_500u64, 3_500] {
        docs_sync
            .notices
            .send(ReplicaNotice::SyncFinished)
            .expect("a subscriber is listening");
        sleep(Duration::from_millis(wait_ms)).await;
    }
    let post = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &topic_replica_id(topic.as_str()),
        &generate_keys(),
        &topic,
        super::range_reconcile::BASE_TIME,
        "synced quietly",
        None,
    )
    .await;
    docs_sync
        .notices
        .send(ReplicaNotice::SyncFinished)
        .expect("a subscriber is listening");
    sleep(Duration::from_millis(3_500)).await;
    assert!(
        !is_projected(pair.viewer_store.as_ref(), &post).await,
        "a sync-finished notice must wait for the stretched interval"
    );
    timeout(Duration::from_secs(8), async {
        while !is_projected(pair.viewer_store.as_ref(), &post).await {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("and the catch-up still runs when the interval has passed");
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}

// ここから下は、独立監査(PR #1268)の確認 test を恒久化したもの。

/// session が固定件数を超える replica で、読む量が session の総数に依存せず、新しい側が必ず入る。
async fn catch_up_with_rooms(rooms: usize) -> (usize, Vec<String>, Vec<String>) {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let pair = pair(docs_sync.clone());
    let topic = format!("kukuri:topic:session-window-{rooms}");
    let mut created = Vec::new();
    for index in 0..rooms {
        created.push(
            pair.author
                .create_game_room(
                    topic.as_str(),
                    game_room_input(format!("room {index}").as_str()),
                )
                .await
                .expect("create game room"),
        );
        // id は `game-<ms>-<owner>` なので、同じ ms に 2 件作らない。
        sleep(Duration::from_millis(2)).await;
    }
    docs_sync.reset_records_returned();
    crate::service::catch_up_sessions(
        &pair.viewer.services,
        topic.as_str(),
        &topic_replica_id(topic.as_str()),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("catch up sessions");
    let returned = docs_sync.records_returned();
    let projection_store: &dyn ProjectionStore = pair.viewer_store.as_ref();
    let projected = projection_store
        .list_topic_game_rooms(topic.as_str())
        .await
        .expect("rooms")
        .into_iter()
        .map(|row| row.room_id)
        .collect::<Vec<_>>();
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
    (returned, created, projected)
}

#[tokio::test]
async fn session_catch_up_is_bounded_and_prefers_the_new_side() {
    let (returned_small, created_small, projected_small) = catch_up_with_rooms(140).await;
    let (returned_large, created_large, projected_large) = catch_up_with_rooms(280).await;
    for (created, projected) in [
        (&created_small, &projected_small),
        (&created_large, &projected_large),
    ] {
        // 新しい側の 32 件は必ず入る。
        for room_id in created.iter().rev().take(32) {
            assert!(
                projected.contains(room_id),
                "the newest rooms must be projected"
            );
        }
        // 反映は固定件数(新しい側 32 + 古い側 32)まで。
        assert_eq!(projected.len(), 64);
    }
    // 読んだ record と key の数は、session の総数を 2 倍にしても変わらない(key の一覧の上限 128 件を超えた範囲)。
    assert_eq!(returned_small, returned_large);
}

// 監査: 一覧の追いつきは replica ごとに間隔を空ける(5 秒)。間隔のあいだは docs を読まず、過ぎれば反映する。
#[tokio::test]
async fn list_catch_up_is_spaced_per_replica() {
    let docs_sync = Arc::new(SilentNoScanDocsSync::default());
    let pair = pair(docs_sync);
    let topic = "kukuri:topic:session-list-interval";
    // TR-10: live / game の無い topic は空を返す(走査のできない docs でも失敗しない)。
    assert!(
        pair.viewer
            .list_game_rooms(topic)
            .await
            .expect("rooms")
            .is_empty()
    );
    assert!(
        pair.viewer
            .list_live_sessions(topic)
            .await
            .expect("sessions")
            .is_empty()
    );
    sleep(Duration::from_millis(300)).await;
    let room_id = pair
        .author
        .create_game_room(topic, game_room_input("late room"))
        .await
        .expect("create game room");
    // 間隔のあいだ(event は届かない docs)。
    assert!(
        pair.viewer
            .list_game_rooms(topic)
            .await
            .expect("rooms")
            .is_empty()
    );
    sleep(Duration::from_millis(5_200)).await;
    let rooms = pair.viewer.list_game_rooms(topic).await.expect("rooms");
    assert!(rooms.iter().any(|room| room.room_id == room_id));
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}

// 監査: viewer が 0 の live session があるあいだは、行があっても一覧の取得が session を反映し直す
// (以前は全件走査が担っていた。終了の event を取りこぼしても、一覧が「配信中」のまま残らない)。
// 注意: この test は head で成功するが、`needs_refresh` の分岐を外す mutation では失敗しなかった
// (購読タスクの側の反映でも Ended になる)。分岐そのものを固定する test にはなっていない。
#[tokio::test]
async fn live_list_refreshes_a_live_session_without_viewers() {
    let docs_sync = Arc::new(SilentNoScanDocsSync::default());
    let pair = pair(docs_sync);
    let topic = "kukuri:topic:live-refresh";
    let session_id = pair
        .author
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "a session".into(),
                description: String::new(),
            },
        )
        .await
        .expect("create live session");
    let sessions = pair
        .viewer
        .list_live_sessions(topic)
        .await
        .expect("sessions");
    let session = sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("the session is listed");
    assert_eq!(session.status, LiveSessionStatus::Live);
    pair.author
        .end_live_session(topic, session_id.as_str())
        .await
        .expect("end live session");
    sleep(Duration::from_millis(5_200)).await;
    let sessions = pair
        .viewer
        .list_live_sessions(topic)
        .await
        .expect("sessions");
    let session = sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("the session is still listed");
    assert_eq!(session.status, LiveSessionStatus::Ended);
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}

// 監査: 形の違う key(誰でも書ける)で prefix 全体の昇順と降順の固定件数を埋められても、
// `sessions/game/game-` の降順が新しい score game を反映する。
#[tokio::test]
async fn foreign_keys_after_the_game_prefix_do_not_hide_new_rooms() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let pair = pair(docs_sync.clone());
    let topic = "kukuri:topic:session-junk";
    let replica = topic_replica_id(topic);
    let room_id = pair
        .author
        .create_game_room(topic, game_room_input("a room"))
        .await
        .expect("create game room");
    // 昇順の側も降順の側も、形の違う key で埋める。
    for index in 0..400 {
        let side = if index % 2 == 0 { "aa" } else { "zz" };
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: format!("sessions/game/{side}-{index:04}/state"),
                    value: serde_json::json!({ "junk": index }),
                },
            )
            .await
            .expect("write a foreign key");
    }
    crate::service::catch_up_sessions(
        &pair.viewer.services,
        topic,
        &replica,
        DocFetchPolicy::LocalOnly,
    )
    .await
    .expect("catch up sessions");
    let projection_store: &dyn ProjectionStore = pair.viewer_store.as_ref();
    assert!(
        projection_store
            .list_topic_game_rooms(topic)
            .await
            .expect("rooms")
            .iter()
            .any(|row| row.room_id == room_id)
    );
    pair.author.shutdown().await;
    pair.viewer.shutdown().await;
}
