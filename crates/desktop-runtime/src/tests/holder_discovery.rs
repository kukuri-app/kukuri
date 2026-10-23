//! #1910 AC-3: desktop と CLI が共用する runtime（本番の stack。SQLite の account store と remote cache）で、R が近隣の
//! 紹介で見つけた保持者 H（CLI の保持 daemon と同じ `set_topic_retention`）から公開 record を既存の検証つき読取りで取得し、
//! 取得した内容を既存の cache に置いて、別の通常 peer X へ再提供する。種別ごとの独自の探索は無く、入口は
//! `topic_read_candidates` の 1 つ。
use super::*;
use kukuri_transport::SeedPeer;

const TOPIC: &str = "kukuri:topic:holder-discovery-runtime";

async fn runtime(dir: &std::path::Path, name: &str) -> Arc<DesktopRuntime> {
    fs::create_dir_all(dir.join(name)).expect("runtime dir");
    Arc::new(
        DesktopRuntime::new(dir.join(name).join("kukuri.db"))
            .await
            .expect("runtime"),
    )
}

async fn ticket_of(runtime: &DesktopRuntime) -> String {
    runtime
        .local_peer_ticket()
        .await
        .expect("ticket")
        .expect("ticket value")
}

fn endpoint_addr_from_ticket(ticket: &str) -> iroh::EndpointAddr {
    let (endpoint_id, addr_hint) = ticket.split_once('@').expect("ticket host");
    SeedPeer {
        endpoint_id: endpoint_id.to_string(),
        addr_hint: Some(addr_hint.to_string()),
    }
    .to_endpoint_addr()
    .expect("endpoint addr")
}

/// `read` が `Some` を返すまで 200ms ごとに読み直す。
async fn poll_until<T, F, Fut>(what: &str, mut read: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    timeout(Duration::from_secs(90), async {
        loop {
            if let Some(value) = read().await {
                return value;
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}: not found within the deadline"))
}

async fn channel_on(
    runtime: &DesktopRuntime,
    channel_id: &str,
) -> Option<kukuri_app_api::PublicChannelView> {
    runtime
        .get_public_channel(GetPublicChannelRequest {
            topic: TOPIC.into(),
            channel_id: channel_id.into(),
        })
        .await
        .expect("get the public channel")
}

async fn post_on(runtime: &DesktopRuntime, post_id: &str) -> Option<String> {
    runtime
        .list_timeline(ListTimelineRequest {
            topic: TOPIC.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(50),
        })
        .await
        .expect("timeline")
        .items
        .into_iter()
        .find(|item| item.object_id == post_id)
        .map(|item| item.content)
}

/// 読み先が `only` だけになるよう、gossip で合流せずに試験の hook で近隣を置く(#1910 の fixture)。
async fn place_only_neighbor(runtime: &DesktopRuntime, ticket: &str) {
    let transport = runtime.iroh_stack.transport.current().await;
    transport
        .place_topic_neighbor_for_tests(
            &kukuri_core::TopicId::new(TOPIC),
            endpoint_addr_from_ticket(ticket),
        )
        .await
        .expect("place the neighbor");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_fetches_from_an_introduced_retaining_daemon_and_reprovides_it() {
    let dir = tempdir().expect("tempdir");
    let (a, h, p, r, x) = (
        runtime(dir.path(), "author").await,
        runtime(dir.path(), "holder").await,
        runtime(dir.path(), "bridge").await,
        runtime(dir.path(), "reader").await,
        runtime(dir.path(), "another").await,
    );
    // 新形式の writer(公開スレッドは bucket の writer が要る)。
    for runtime in [&a, &h, &p, &r, &x] {
        runtime.app_service.switch_writer(1);
    }
    let (ticket_a, ticket_h, ticket_p, ticket_r) = (
        ticket_of(&a).await,
        ticket_of(&h).await,
        ticket_of(&p).await,
        ticket_of(&r).await,
    );
    // A・H・P は互いの ticket で gossip に合流する。H は CLI の保持 daemon と同じく、列なしに topic を保持する。P は
    // 列を開くだけで、何も保持しない(本番の構成では列の読込みで取得した内容は cache に入るので、P は購読だけにする)。
    for (runtime, tickets) in [
        (&a, [&ticket_h, &ticket_p]),
        (&h, [&ticket_a, &ticket_p]),
        (&p, [&ticket_a, &ticket_h]),
    ] {
        for ticket in tickets {
            runtime
                .import_peer_ticket(ImportPeerTicketRequest {
                    ticket: (*ticket).clone(),
                })
                .await
                .expect("import a ticket");
        }
    }
    h.set_topic_retention(SetTopicRetentionRequest {
        topic: TOPIC.into(),
        retained: true,
    })
    .await
    .expect("H retains the topic");
    let p_transport = p.iroh_stack.transport.current().await;
    let _p_hints = kukuri_transport::HintTransport::subscribe_hints(
        p_transport.as_ref(),
        &kukuri_core::TopicId::new(TOPIC),
    )
    .await
    .expect("P joins the topic gossip");
    open_topic_column(&a, TOPIC, TimelineScope::Public)
        .await
        .expect("A opens the topic");

    // A が公開スレッドと topic の投稿を作り、H が保持する。
    let channel = a
        .create_public_channel(CreatePublicChannelRequest {
            topic: TOPIC.into(),
            title: "紹介で届くスレッド".into(),
            first_post: "最初の投稿".into(),
            attachments: Vec::new(),
            content_labels: Vec::new(),
        })
        .await
        .expect("A creates the public channel");
    let post_id = a
        .create_post(CreatePostRequest {
            topic: TOPIC.into(),
            content: "topic の投稿".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: Vec::new(),
            content_labels: Vec::new(),
            live: None,
        })
        .await
        .expect("A posts");
    // P の近隣に A と H が揃う(H は保持の lease で topic の gossip に居る)。
    timeout(Duration::from_secs(30), async {
        loop {
            let count = kukuri_transport::Transport::peers(p_transport.as_ref())
                .await
                .expect("peers of P")
                .topic_diagnostics
                .iter()
                .filter(|diag| diag.topic == format!("hint/{TOPIC}"))
                .map(|diag| diag.peer_count)
                .sum::<usize>();
            if count >= 2 {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("P has A and H as neighbors");
    poll_until("H holds the channel metadata", || {
        channel_on(&h, &channel.channel_id)
    })
    .await;
    poll_until("H holds the topic post", || post_on(&h, &post_id)).await;
    a.shutdown().await;

    // R は A を知らず、H とも合流しない。読み先は P だけで、P の紹介で H が候補に入る。
    open_topic_column(&r, TOPIC, TimelineScope::Public)
        .await
        .expect("R opens the topic");
    place_only_neighbor(&r, &ticket_p).await;
    let found = poll_until("R reads the metadata from the introduced H", || {
        channel_on(&r, &channel.channel_id)
    })
    .await;
    assert_eq!(found.title, "紹介で届くスレッド");
    assert_eq!(
        poll_until("R reads the topic post from the introduced H", || post_on(
            &r, &post_id
        ))
        .await,
        "topic の投稿"
    );
    let introduced = r
        .iroh_stack
        .transport
        .current()
        .await
        .introduced_topic_peers_for_tests(&kukuri_core::TopicId::new(TOPIC))
        .await;
    assert!(
        introduced.contains(&endpoint_addr_from_ticket(&ticket_h).id.to_string()),
        "{introduced:?}"
    );

    // H を止める。X の読み先は R だけで、R が取得して cache に置いた内容を再提供する。
    h.shutdown().await;
    open_topic_column(&x, TOPIC, TimelineScope::Public)
        .await
        .expect("X opens the topic");
    place_only_neighbor(&x, &ticket_r).await;
    assert_eq!(
        poll_until("X reads the metadata from R", || channel_on(
            &x,
            &channel.channel_id
        ))
        .await
        .title,
        "紹介で届くスレッド"
    );
    assert_eq!(
        poll_until("X reads the topic post from R", || post_on(&x, &post_id)).await,
        "topic の投稿"
    );

    for runtime in [&p, &r, &x] {
        runtime.shutdown().await;
    }
}
