//! #1910 AC-1・AC-2: 公開の record・page が手元と最初の読み先に無いとき、直接の近隣が非保持者でも、近隣に紹介して
//! もらった非近隣の通常 peer（保持者）から既存の docs の読取りで取得する。
#[cfg(feature = "iroh-integration-tests")]
use super::*;
#[cfg(feature = "iroh-integration-tests")]
use iroh::EndpointAddr;

#[cfg(feature = "iroh-integration-tests")]
fn endpoint_addr_from_ticket(ticket: &str) -> EndpointAddr {
    let (endpoint_id, addr_hint) = ticket.split_once('@').expect("ticket host");
    SeedPeer {
        endpoint_id: endpoint_id.to_string(),
        addr_hint: Some(addr_hint.to_string()),
    }
    .to_endpoint_addr()
    .expect("endpoint addr")
}

#[cfg(feature = "iroh-integration-tests")]
async fn ticket_of(stack: &TestIrohStack) -> String {
    stack
        .transport
        .export_ticket()
        .await
        .expect("ticket")
        .expect("ticket value")
}

/// 本番と同じく、アカウントの鍵から導いた docs author で書く端末(ADR 0053。保持は envelope の docs author と record の
/// 名義の一致で判定する)。`cached` なら取得した内容を remote cache に持つ(保持者と、cache の blob を取る読み手)。
#[cfg(feature = "iroh-integration-tests")]
async fn peer(root: &std::path::Path, cached: bool) -> (TestIrohStack, AppService) {
    let keys = generate_keys();
    let (stack, store, projection): (TestIrohStack, Arc<dyn Store>, Arc<dyn ProjectionStore>) =
        if cached {
            let store = Arc::new(SqliteStore::connect_memory().await.expect("sqlite store"));
            let stack = TestIrohStack::new_with_remote_cache(root, store.clone()).await;
            (stack, store.clone(), store)
        } else {
            (
                TestIrohStack::new(root).await,
                Arc::new(MemoryStore::default()),
                Arc::new(MemoryStore::default()),
            )
        };
    stack
        .docs_sync
        .use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
        .await
        .expect("docs author");
    let app = app_service_from_dependencies(
        store,
        projection,
        stack.transport.clone(),
        stack.transport.clone(),
        stack.docs_sync.clone(),
        stack.blob_service.clone(),
        keys,
    );
    app.switch_writer(1);
    (stack, app)
}

/// topic の hint の gossip に、`expected` 人以上の近隣が居るまで待つ。
#[cfg(feature = "iroh-integration-tests")]
async fn wait_for_neighbors(transport: &IrohGossipTransport, topic: &str, expected: usize) {
    let hint_topic = format!("hint/{topic}");
    timeout(Duration::from_secs(30), async {
        while !transport
            .peers()
            .await
            .expect("peers")
            .topic_diagnostics
            .iter()
            .any(|diag| diag.topic == hint_topic && diag.peer_count >= expected)
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{topic}: {expected} neighbors did not become established"));
}

#[cfg(feature = "iroh-integration-tests")]
async fn display_public_channel(app: &AppService, topic: &str, channel_id: &str) {
    app.set_scope_display(crate::ScopeDisplayRequest {
        observer: format!("test-channel:{channel_id}"),
        target: crate::ScopeDisplayTarget::Timeline {
            topic: topic.into(),
            scope: TimelineScope::PublicChannel {
                channel_id: ChannelId::new(channel_id),
            },
        },
        visible: true,
    })
    .await
    .expect("open the public channel column");
}

#[cfg(feature = "iroh-integration-tests")]
async fn channel_posts(app: &AppService, topic: &str, channel_id: &str) -> Vec<PostView> {
    app.list_timeline_scoped(
        topic,
        TimelineScope::PublicChannel {
            channel_id: ChannelId::new(channel_id),
        },
        None,
        50,
    )
    .await
    .expect("channel timeline")
    .items
}

/// `read` が `Some` を返すまで 200ms ごとに読み直す（読取りは 1 要求ごとに期限 30 秒の内で終わる）。
#[cfg(feature = "iroh-integration-tests")]
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

/// Issue #1910 の fixture。A が公開スレッド（metadata と最初の投稿）・topic の投稿・Live を作り、同じ topic の H が
/// 検証して保持する。A を止めた後、R の直接の近隣は対象を持たない P だけ（R は gossip で join せず、H を知らず、
/// 台帳にも H は無い）。P は同じ topic の gossip に居て H を近隣に持つ。
///
/// 修正前: R の読み先は P だけなので、metadata の Exact・channel の投稿 page・投稿 envelope・Live の session は
/// 未取得で終わる。修正後: P の紹介で H が候補に入り、既存の docs の読取りで検証つきに取得する。H に特別な印・
/// 固定の宛先・優先の枠は無い（P と同じ `topic_read_candidates` の窓から選ぶ）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn a_reader_fetches_public_records_from_a_holder_introduced_by_a_non_holding_neighbor() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let (stack_a, app_a) = peer(&dir.path().join("author"), false).await;
    // H は取得した内容を remote cache に持ち、他の参加者へ提供する(本番と同じ構成。#1395)。R も remote cache を
    // 持ち、H が cache で保持する blob を同じ経路で取る。
    let (stack_h, app_h) = peer(&dir.path().join("holder"), true).await;
    let stack_p = TestIrohStack::new(&dir.path().join("bridge")).await;
    let (stack_r, app_r) = peer(&dir.path().join("reader"), true).await;
    let topic = "kukuri:topic:holder-discovery";

    // A・H・P は互いの ticket で gossip に合流する。P は topic の hint を購読するだけで、何も読まず保持しない。
    let (ticket_a, ticket_h, ticket_p) = (
        ticket_of(&stack_a).await,
        ticket_of(&stack_h).await,
        ticket_of(&stack_p).await,
    );
    for (stack, tickets) in [
        (&stack_a, [&ticket_h, &ticket_p]),
        (&stack_h, [&ticket_a, &ticket_p]),
        (&stack_p, [&ticket_a, &ticket_h]),
    ] {
        for ticket in tickets {
            stack
                .transport
                .import_ticket(ticket)
                .await
                .expect("import a ticket");
        }
    }
    display_topic_in(&[&app_a, &app_h], topic).await;
    let _bridge_hints = stack_p
        .transport
        .subscribe_hints(&TopicId::new(topic))
        .await
        .expect("the bridge joins the topic gossip");
    wait_for_neighbors(&stack_p.transport, topic, 2).await;
    wait_for_neighbors(&stack_h.transport, topic, 2).await;

    // A が公開スレッド・topic の投稿・Live を作り、H が検証して保持する。
    let channel = app_a
        .create_public_channel(topic, "保持者の探索", "最初の投稿", Vec::new(), Vec::new())
        .await
        .expect("create the public channel");
    let first_post_id = channel.first_post_id.clone().expect("the first post");
    display_public_channel(&app_h, topic, &channel.channel_id).await;
    let topic_post_id = app_a
        .create_post(topic, "topic の投稿", None)
        .await
        .expect("create a topic post");
    let live_id = app_a
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "保持者の Live".into(),
                description: String::new(),
                start_post: String::new(),
            },
        )
        .await
        .expect("create a live session");
    display_remote_session(&app_h, topic, &live_id, "live").await;
    poll_until("H holds the channel metadata", || async {
        app_h
            .get_public_channel(topic, &channel.channel_id)
            .await
            .expect("get the channel on H")
    })
    .await;
    poll_until("H holds the first post", || async {
        channel_posts(&app_h, topic, &channel.channel_id)
            .await
            .into_iter()
            .find(|post| post.object_id == first_post_id)
    })
    .await;
    poll_until("H holds the topic post", || async {
        app_h
            .list_timeline(topic, None, 50)
            .await
            .expect("timeline on H")
            .items
            .into_iter()
            .find(|post| post.object_id == topic_post_id)
    })
    .await;
    poll_until("H holds the live session", || async {
        app_h
            .list_live_sessions(topic)
            .await
            .expect("live sessions on H")
            .into_iter()
            .find(|session| session.session_id == live_id)
    })
    .await;

    // A を止める。
    app_a.shutdown().await;
    stack_a
        ._node
        .clone()
        .shutdown()
        .await
        .expect("shut down the author");

    // R は topic の列を開くが誰とも合流せず、試験の hook で P だけを近隣に置く（gossip の join は経ない）。
    display_topic_in(&[&app_r], topic).await;
    stack_r
        .transport
        .place_topic_neighbor_for_tests(&TopicId::new(topic), endpoint_addr_from_ticket(&ticket_p))
        .await
        .expect("place the bridge as the only neighbor");

    let found = poll_until("the channel metadata via the introduced holder", || async {
        app_r
            .get_public_channel(topic, &channel.channel_id)
            .await
            .expect("get the channel on R")
    })
    .await;
    assert_eq!(found.title, "保持者の探索");
    assert_eq!(found.first_post_id.as_deref(), Some(first_post_id.as_str()));
    // 公開スレッドを開く(channel の列の lease。R は channel の hint の近隣を持たず、topic の近隣から読む)。
    display_public_channel(&app_r, topic, &channel.channel_id).await;
    let first = poll_until("the first post via the introduced holder", || async {
        channel_posts(&app_r, topic, &channel.channel_id)
            .await
            .into_iter()
            .find(|post| post.object_id == first_post_id)
    })
    .await;
    assert_eq!(first.content, "最初の投稿");
    assert_eq!(
        first.channel_id.as_deref(),
        Some(channel.channel_id.as_str())
    );
    let topic_post = poll_until("the topic post via the introduced holder", || async {
        app_r
            .list_timeline(topic, None, 50)
            .await
            .expect("timeline on R")
            .items
            .into_iter()
            .find(|post| post.object_id == topic_post_id)
    })
    .await;
    assert_eq!(topic_post.content, "topic の投稿");
    display_remote_session(&app_r, topic, &live_id, "live").await;
    let live = poll_until("the live session via the introduced holder", || async {
        app_r
            .list_live_sessions(topic)
            .await
            .expect("live sessions on R")
            .into_iter()
            .find(|session| session.session_id == live_id)
    })
    .await;
    assert_eq!(live.title, "保持者の Live");
    // H は紹介で入った候補で、R の近隣でも台帳の peer でもない。P(要求先)は紹介に含まれない。
    let introduced = stack_r
        .transport
        .introduced_topic_peers_for_tests(&TopicId::new(topic))
        .await;
    assert!(
        introduced.contains(&endpoint_addr_from_ticket(&ticket_h).id.to_string()),
        "{introduced:?}"
    );
    assert!(
        !introduced.contains(&endpoint_addr_from_ticket(&ticket_p).id.to_string()),
        "{introduced:?}"
    );

    app_h.shutdown().await;
    app_r.shutdown().await;
}

/// AC-2: R の直接の近隣は非保持の 4 台（P1〜P4。1 窓の上限と同じ数）で、保持者 H は後から topic に現れる。
///
/// 最初の要求は上限（1 窓 4 台・30 秒）の内に未取得で終わり、全ネットワークでの不存在とは断定しない（台帳と窓は
/// 残る）。H が現れると、定期の紹介で近隣の P が H を紹介し、正規の再試行（読み直し）で取得する。窓は近隣∪台帳を
/// 回るので、同じ 4 台だけを永久に試さない。履歴（channel の投稿）を 10 倍にしても、1 要求の読み先の窓は 4 台のままで、
/// 読取りは紹介の要求を増やさない（要求数は間隔で決まる）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn a_holder_that_appears_later_is_reached_through_periodic_introductions() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let (stack_a, app_a) = peer(&dir.path().join("author"), false).await;
    let (stack_h, app_h) = peer(&dir.path().join("holder"), true).await;
    let mut bridges = Vec::new();
    for index in 0..4 {
        bridges.push(TestIrohStack::new(&dir.path().join(format!("bridge-{index}"))).await);
    }
    let (stack_r, app_r) = peer(&dir.path().join("reader"), true).await;
    let topic = "kukuri:topic:holder-discovery-later";
    let topic_id = TopicId::new(topic);

    // A と P1〜P4 は gossip で合流する(P は購読するだけで保持しない)。
    let ticket_a = ticket_of(&stack_a).await;
    let mut bridge_tickets = Vec::new();
    for bridge in &bridges {
        bridge_tickets.push(ticket_of(bridge).await);
    }
    for ticket in &bridge_tickets {
        stack_a
            .transport
            .import_ticket(ticket)
            .await
            .expect("A imports a bridge");
    }
    for bridge in &bridges {
        bridge
            .transport
            .import_ticket(&ticket_a)
            .await
            .expect("a bridge imports A");
    }
    display_topic_in(&[&app_a], topic).await;
    let mut bridge_hints = Vec::new();
    for bridge in &bridges {
        bridge_hints.push(
            bridge
                .transport
                .subscribe_hints(&topic_id)
                .await
                .expect("a bridge joins the topic gossip"),
        );
    }
    for bridge in &bridges {
        wait_for_neighbors(&bridge.transport, topic, 1).await;
    }

    // A が公開スレッドと、履歴として channel の投稿を 20 件(最初の投稿の 10 倍以上)、topic の投稿を作る。
    let channel = app_a
        .create_public_channel(
            topic,
            "後から現れる保持者",
            "最初の投稿",
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("create the public channel");
    let first_post_id = channel.first_post_id.clone().expect("the first post");
    let channel_ref = ChannelRef::PublicChannel {
        channel_id: ChannelId::new(&channel.channel_id),
    };
    for index in 0..20 {
        app_a
            .create_post_with_attachments_in_channel(
                topic,
                channel_ref.clone(),
                &format!("履歴 {index}"),
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .await
            .expect("a channel post");
    }
    let topic_post_id = app_a
        .create_post(topic, "topic の投稿", None)
        .await
        .expect("create a topic post");

    // H は gossip で合流して保持し、その後いったん topic の gossip を抜ける(R の最初の要求の時点では、どの P の近隣でも
    // ない)。
    stack_h
        .transport
        .import_ticket(&ticket_a)
        .await
        .expect("H imports A");
    stack_h
        .transport
        .import_ticket(&bridge_tickets[0])
        .await
        .expect("H imports a bridge");
    display_topic_in(&[&app_h], topic).await;
    display_public_channel(&app_h, topic, &channel.channel_id).await;
    poll_until("H holds the channel metadata", || async {
        app_h
            .get_public_channel(topic, &channel.channel_id)
            .await
            .expect("get the channel on H")
    })
    .await;
    poll_until("H holds the channel history", || async {
        let posts = channel_posts(&app_h, topic, &channel.channel_id).await;
        (posts.len() >= 21).then_some(posts.len())
    })
    .await;
    poll_until("H holds the topic post", || async {
        app_h
            .list_timeline(topic, None, 50)
            .await
            .expect("timeline on H")
            .items
            .into_iter()
            .find(|post| post.object_id == topic_post_id)
    })
    .await;

    // A を止め、H は gossip を抜ける(lease は残す)。P1〜P4 の近隣が互いだけになるのを待つ。
    app_a.shutdown().await;
    stack_a
        ._node
        .clone()
        .shutdown()
        .await
        .expect("shut down the author");
    app_h
        .set_topic_gossip_enabled(topic, false)
        .await
        .expect("stop the topic gossip of H");
    app_h
        .set_channel_gossip_enabled(topic, &channel.channel_id, false)
        .await
        .expect("stop the channel gossip of H");
    let hint_topic = format!("hint/{topic}");
    timeout(Duration::from_secs(30), async {
        loop {
            let mut settled = true;
            for bridge in &bridges {
                let count = bridge
                    .transport
                    .peers()
                    .await
                    .expect("peers")
                    .topic_diagnostics
                    .iter()
                    .filter(|diag| diag.topic == hint_topic)
                    .map(|diag| diag.peer_count)
                    .sum::<usize>();
                settled &= count <= bridges.len() - 1;
            }
            if settled {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the bridges forget A and H");

    // R は列を開き、P1〜P4 だけを近隣に置く。定期の紹介は 1 秒ごと。
    display_topic_in(&[&app_r], topic).await;
    display_public_channel(&app_r, topic, &channel.channel_id).await;
    stack_r
        .transport
        .set_introduction_interval_for_tests(&topic_id, Duration::from_secs(1))
        .await
        .expect("interval");
    for ticket in &bridge_tickets {
        stack_r
            .transport
            .place_topic_neighbor_for_tests(&topic_id, endpoint_addr_from_ticket(ticket))
            .await
            .expect("place a bridge as a neighbor");
    }
    let holder_id = endpoint_addr_from_ticket(&ticket_of(&stack_h).await)
        .id
        .to_string();

    // 最初の要求は、上限の内に未取得で終わる(H はまだ誰の近隣でもない)。
    let started = n0_future::time::Instant::now();
    assert!(
        app_r
            .get_public_channel(topic, &channel.channel_id)
            .await
            .expect("get the channel on R")
            .is_none()
    );
    assert!(
        started.elapsed() < Duration::from_secs(35),
        "{:?}",
        started.elapsed()
    );
    assert!(
        !stack_r
            .transport
            .introduced_topic_peers_for_tests(&topic_id)
            .await
            .contains(&holder_id)
    );

    // H が topic の gossip に現れる(P1 と合流する)。
    app_h
        .set_topic_gossip_enabled(topic, true)
        .await
        .expect("resume the topic gossip of H");
    app_h
        .set_channel_gossip_enabled(topic, &channel.channel_id, true)
        .await
        .expect("resume the channel gossip of H");
    wait_for_neighbors(&stack_h.transport, topic, 1).await;

    // 正規の再試行で、紹介された H から取得する。
    let found = poll_until("the channel metadata after the holder appears", || async {
        app_r
            .get_public_channel(topic, &channel.channel_id)
            .await
            .expect("get the channel on R")
    })
    .await;
    assert_eq!(found.first_post_id.as_deref(), Some(first_post_id.as_str()));
    poll_until("the channel history via the holder", || async {
        let posts = channel_posts(&app_r, topic, &channel.channel_id).await;
        (posts.len() >= 21).then_some(posts.len())
    })
    .await;
    poll_until("the topic post via the holder", || async {
        app_r
            .list_timeline(topic, None, 50)
            .await
            .expect("timeline on R")
            .items
            .into_iter()
            .find(|post| post.object_id == topic_post_id)
    })
    .await;
    assert!(
        stack_r
            .transport
            .introduced_topic_peers_for_tests(&topic_id)
            .await
            .contains(&holder_id)
    );

    // 履歴が 10 倍でも、1 要求の読み先の窓は 4 台のままで、読取りは紹介の要求を増やさない。
    stack_r
        .transport
        .set_introduction_interval_for_tests(&topic_id, Duration::from_secs(60))
        .await
        .expect("interval");
    let requests_before = stack_r
        .transport
        .introduction_request_count_for_tests(&topic_id)
        .await;
    let served = |bridges: &[TestIrohStack]| {
        bridges
            .iter()
            .map(|bridge| {
                bridge
                    .transport
                    .topic_peer_introductions()
                    .served_count_for_tests()
            })
            .sum::<u64>()
    };
    let served_before = served(&bridges);
    for _ in 0..10 {
        let posts = channel_posts(&app_r, topic, &channel.channel_id).await;
        assert!(posts.len() >= 21);
        app_r
            .list_timeline(topic, None, 50)
            .await
            .expect("timeline on R");
    }
    assert!(
        stack_r
            .transport
            .topic_read_candidates(&topic_id)
            .await
            .expect("candidates")
            .len()
            <= 4
    );
    assert_eq!(
        stack_r
            .transport
            .introduction_request_count_for_tests(&topic_id)
            .await,
        requests_before
    );
    assert_eq!(served(&bridges), served_before);

    app_h.shutdown().await;
    app_r.shutdown().await;
}
