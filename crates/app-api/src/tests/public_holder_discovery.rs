//! #1910 AC-1: 公開の record・page が手元と最初の読み先に無いとき、直接の近隣が非保持者でも、近隣に紹介して
//! もらった非近隣の通常 peer（保持者）から既存の docs の読取りで取得する。
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
    let stack_a = TestIrohStack::new(&dir.path().join("author")).await;
    // H は取得した内容を remote cache に持ち、他の参加者へ提供する(本番と同じ構成。#1395)。
    let store_h = Arc::new(
        SqliteStore::connect_memory()
            .await
            .expect("sqlite store for the holder"),
    );
    let stack_h =
        TestIrohStack::new_with_remote_cache(&dir.path().join("holder"), store_h.clone()).await;
    let stack_p = TestIrohStack::new(&dir.path().join("bridge")).await;
    // R も remote cache を持つ(本番と同じ構成。H が cache で保持する blob を同じ経路で取る)。
    let store_r = Arc::new(
        SqliteStore::connect_memory()
            .await
            .expect("sqlite store for the reader"),
    );
    let stack_r =
        TestIrohStack::new_with_remote_cache(&dir.path().join("reader"), store_r.clone()).await;
    // 本番と同じく、アカウントの鍵から導いた docs author で書く(ADR 0053。保持は envelope の docs author と
    // record の名義の一致で判定する)。
    let keys_a = generate_keys();
    stack_a
        .docs_sync
        .use_account_docs_author(&keys_a.derive_docs_author_seed(), &keys_a.public_key_hex())
        .await
        .expect("docs author of the author");
    let app_a = app_service_from_dependencies(
        Arc::new(MemoryStore::default()),
        Arc::new(MemoryStore::default()),
        stack_a.transport.clone(),
        stack_a.transport.clone(),
        stack_a.docs_sync.clone(),
        stack_a.blob_service.clone(),
        keys_a,
    );
    let keys_h = generate_keys();
    stack_h
        .docs_sync
        .use_account_docs_author(&keys_h.derive_docs_author_seed(), &keys_h.public_key_hex())
        .await
        .expect("docs author of the holder");
    let app_h = app_service_from_dependencies(
        store_h.clone(),
        store_h,
        stack_h.transport.clone(),
        stack_h.transport.clone(),
        stack_h.docs_sync.clone(),
        stack_h.blob_service.clone(),
        keys_h,
    );
    let keys_r = generate_keys();
    stack_r
        .docs_sync
        .use_account_docs_author(&keys_r.derive_docs_author_seed(), &keys_r.public_key_hex())
        .await
        .expect("docs author of the reader");
    let app_r = app_service_from_dependencies(
        store_r.clone(),
        store_r,
        stack_r.transport.clone(),
        stack_r.transport.clone(),
        stack_r.docs_sync.clone(),
        stack_r.blob_service.clone(),
        keys_r,
    );
    for app in [&app_a, &app_h, &app_r] {
        app.switch_writer(1);
    }
    let topic = "kukuri:topic:holder-discovery";

    // A・H・P は互いの ticket で gossip に合流する。P は topic の hint を購読するだけで、何も読まず保持しない。
    let ticket = |stack: &TestIrohStack| {
        let transport = stack.transport.clone();
        async move {
            transport
                .export_ticket()
                .await
                .expect("ticket")
                .expect("ticket value")
        }
    };
    let (ticket_a, ticket_h, ticket_p) = (
        ticket(&stack_a).await,
        ticket(&stack_h).await,
        ticket(&stack_p).await,
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
