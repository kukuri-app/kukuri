//! #1840 AC-4: 公開スレッドで始めた Live。そのスレッドの Live の一覧(公開 channel の scope)に出て、topic の公開の Live の
//! 一覧には出ない。一覧の行は配信中の印を持ち、片方を終えても残りが配信中なら印は残る。公開の Live なので、topic の
//! 資格で視聴でき、どこへでも紹介できる。Live の変化は topic の hint で届くので、一覧だけを開いた端末にも届く。
use super::public_channels::{TOPIC, channel_ref, device};
use super::*;
use kukuri_core::SpatialContextV1;
use kukuri_docs_sync::MemoryDocsSync;
use kukuri_store::PublicChannelOrder;

fn live_input(title: &str) -> CreateLiveSessionInput {
    CreateLiveSessionInput {
        title: title.into(),
        description: String::new(),
        start_post: format!("{title}を始めます"),
    }
}

/// 一覧の列だけを開く(topic の lease だけを取り、topic の hint を受ける)。
async fn open_list(app: &AppService) {
    app.set_scope_display(crate::ScopeDisplayRequest {
        observer: "test-public-threads".into(),
        target: crate::ScopeDisplayTarget::Timeline {
            topic: TOPIC.into(),
            scope: TimelineScope::Public,
        },
        visible: true,
    })
    .await
    .expect("open the public thread list");
}

async fn thread_lives(app: &AppService, channel_id: &str) -> Vec<LiveSessionView> {
    app.list_live_sessions_scoped(TOPIC, channel_ref(channel_id).timeline_scope())
        .await
        .expect("the lives of the thread")
}

/// 一覧の行の配信中の印。行が無ければ `None`。
async fn marked(app: &AppService, channel_id: &str) -> Option<bool> {
    app.list_public_channels(TOPIC, PublicChannelOrder::Created, None, 20)
        .await
        .expect("list the public threads")
        .items
        .into_iter()
        .find(|row| row.channel_id == channel_id)
        .map(|row| row.live_now)
}

/// `done` が満たされるまで取り直す(期限 10 秒。hint と背景の読み込みを待つ)。
async fn until<T>(mut read: impl AsyncFnMut() -> T, done: impl Fn(&T) -> bool) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let value = read().await;
        if done(&value) || tokio::time::Instant::now() >= deadline {
            return value;
        }
        sleep(Duration::from_millis(20)).await;
    }
}

/// session の ID の順に並べた状態。
fn by_id(mut statuses: Vec<(String, LiveSessionStatus)>) -> Vec<(String, LiveSessionStatus)> {
    statuses.sort_by(|a, b| a.0.cmp(&b.0));
    statuses
}

fn statuses(lives: &[LiveSessionView]) -> Vec<(String, LiveSessionStatus)> {
    by_id(
        lives
            .iter()
            .map(|live| (live.session_id.clone(), live.status.clone()))
            .collect(),
    )
}

#[tokio::test]
async fn lives_started_in_a_public_thread_are_listed_and_marked_until_the_last_one_ends() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("02".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);
    open_list(&a).await;
    open_list(&b).await;
    let thread = a
        .create_public_channel(TOPIC, "配信の場所", "")
        .await
        .expect("create the public thread");
    let id = thread.channel_id.as_str();
    assert_eq!(
        until(async || marked(&b, id).await, Option::is_some).await,
        Some(false)
    );

    // A と、スレッドに何も書いていない B が、同じスレッドで Live を始める。
    let first = a
        .create_live_session_in_channel(TOPIC, channel_ref(id), live_input("Aの配信"), None)
        .await
        .expect("A starts a live in the thread");
    let second = b
        .create_live_session_in_channel(TOPIC, channel_ref(id), live_input("Bの配信"), None)
        .await
        .expect("B starts a live in the thread");
    let both_live = by_id(vec![
        (first.clone(), LiveSessionStatus::Live),
        (second.clone(), LiveSessionStatus::Live),
    ]);
    for app in [&a, &b] {
        let lives = until(
            async || thread_lives(app, id).await,
            |lives| lives.len() == 2,
        )
        .await;
        assert_eq!(statuses(&lives), both_live);
        assert!(
            lives
                .iter()
                .all(|live| live.channel_id.as_deref() == Some(id)
                    && live.audience_label == "Public thread"),
            "{lives:?}"
        );
        assert_eq!(
            until(async || marked(app, id).await, |mark| *mark == Some(true)).await,
            Some(true)
        );
        // topic の公開の Live の一覧には出ない。
        assert!(
            app.list_live_sessions(TOPIC)
                .await
                .expect("the public lives of the topic")
                .is_empty()
        );
    }

    // 公開の Live なので、始めたスレッドにも topic の公開の場所にも紹介できる。紹介しても Live は 1 件のまま。
    let live_ref = LiveRefV1 {
        session_id: second.clone(),
        topic_id: TopicId::new(TOPIC),
        channel_id: Some(ChannelId::new(id)),
    };
    for place in [channel_ref(id), ChannelRef::Public] {
        a.create_post_with_attachments_in_channel(
            TOPIC,
            place,
            "見に来て",
            None,
            Vec::new(),
            Vec::new(),
            Some(live_ref.clone()),
        )
        .await
        .expect("introduce the live");
    }
    assert_eq!(thread_lives(&a, id).await.len(), 2);

    // B が終えても、A の Live が配信中なので印は残る。
    b.end_live_session(TOPIC, &second)
        .await
        .expect("B ends the live");
    let ended_second = by_id(vec![
        (first.clone(), LiveSessionStatus::Live),
        (second.clone(), LiveSessionStatus::Ended),
    ]);
    let lives = until(
        async || thread_lives(&a, id).await,
        |lives| statuses(lives) == ended_second,
    )
    .await;
    assert_eq!(statuses(&lives), ended_second);
    assert_eq!(marked(&a, id).await, Some(true));

    // 最後の Live を終えると、どちらの端末でも印が消える。開始 post はスレッドに残る。
    a.end_live_session(TOPIC, &first)
        .await
        .expect("A ends the live");
    for app in [&a, &b] {
        assert_eq!(
            until(async || marked(app, id).await, |mark| *mark == Some(false)).await,
            Some(false)
        );
    }
    let posts = a
        .list_timeline_scoped(TOPIC, channel_ref(id).timeline_scope(), None, 50)
        .await
        .expect("the thread timeline")
        .items;
    for session in [&first, &second] {
        assert!(
            posts.iter().any(|post| post
                .live
                .as_ref()
                .is_some_and(|live| live.session_id == *session)),
            "the start post of {session} stays in the thread: {posts:?}"
        );
    }
    a.shutdown().await;
    b.shutdown().await;
}

// 公開スレッドの Live は topic の資格で視聴する(private channel の参加を求めない)。非表示の主催者の Live は、一覧の行の
// 印に数えない。
#[tokio::test]
async fn a_thread_live_is_watched_with_the_topic_access_and_hidden_hosts_are_not_marked() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("02".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);
    open_list(&a).await;
    open_list(&b).await;
    let thread = a
        .create_public_channel(TOPIC, "配信の場所", "")
        .await
        .expect("create the public thread");
    let id = thread.channel_id.as_str();
    let session = b
        .create_live_session_in_channel(
            TOPIC,
            channel_ref(id),
            live_input("Bの配信"),
            Some("1".repeat(64)),
        )
        .await
        .expect("B starts a live with media");

    let topic_context = SpatialContextV1::Topic {
        topic_id: TopicId::new(TOPIC),
    };
    let (_, _, context, owner) = until(
        async || {
            a.live_media_source(TOPIC, &session)
                .await
                .expect("the media source")
        },
        Option::is_some,
    )
    .await
    .expect("A sees the media of B's live");
    assert_eq!(context, topic_context);
    assert_eq!(owner.as_str(), b.current_author_pubkey());
    a.build_dome_access_proof(context, owner)
        .await
        .expect("A builds the proof with the topic access");
    let (scope, _, _) = b
        .live_media_scope(TOPIC, &session)
        .await
        .expect("the media scope")
        .expect("B hosts the live");
    assert_eq!(scope, topic_context);

    assert_eq!(
        until(async || marked(&a, id).await, |mark| *mark == Some(true)).await,
        Some(true)
    );
    a.mute_author(b.current_author_pubkey().as_str())
        .await
        .expect("mute B");
    assert_eq!(marked(&a, id).await, Some(false));
    a.shutdown().await;
    b.shutdown().await;
}

// 公開スレッドの Live に別の端末が参加すると、一覧の列だけを開いた配信者の端末でも視聴者に数える(presence は topic の
// hint で届き、受け取る側が Live の行の channel に結び付ける)。退出すると数えない。
#[tokio::test]
async fn a_viewer_of_a_thread_live_is_counted_on_the_host_that_only_has_the_list_open() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("02".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);
    open_list(&a).await;
    open_list(&b).await;
    let thread = b
        .create_public_channel(TOPIC, "配信の場所", "")
        .await
        .expect("create the public thread");
    let id = thread.channel_id.as_str();
    let session = b
        .create_live_session_in_channel(TOPIC, channel_ref(id), live_input("Bの配信"), None)
        .await
        .expect("B starts a live in the thread");
    until(
        async || {
            a.get_live_session(TOPIC, &session)
                .await
                .expect("the live on A")
        },
        Option::is_some,
    )
    .await;
    a.join_live_session(TOPIC, &session)
        .await
        .expect("A joins the live");
    let viewers = |lives: &[LiveSessionView]| {
        lives
            .iter()
            .find(|live| live.session_id == session)
            .map(|live| live.viewer_count)
    };
    for app in [&a, &b] {
        let lives = until(
            async || thread_lives(app, id).await,
            |lives| viewers(lives) == Some(1),
        )
        .await;
        assert_eq!(viewers(&lives), Some(1), "{lives:?}");
    }
    a.leave_live_session(TOPIC, &session)
        .await
        .expect("A leaves the live");
    let lives = until(
        async || thread_lives(&b, id).await,
        |lives| viewers(lives) == Some(0),
    )
    .await;
    assert_eq!(viewers(&lives), Some(0));
    a.shutdown().await;
    b.shutdown().await;
}
