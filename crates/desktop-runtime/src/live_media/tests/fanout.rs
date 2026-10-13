//! #1034 AC-2b の contract: 視聴者どうしの転送(ADR 0067 §8)。

use super::*;
use n0_future::time::Instant;
use std::sync::atomic::AtomicUsize;

use crate::media_access::{DENIED_FULL, denial};

/// 配信者が直接送るのは 4 人、視聴者が転送するのは 2 人までで、視聴者を増やしても超えない。転送する視聴者は祖先からの
/// 購読を断り、参加の問い合わせは 16 段で止まる。
#[tokio::test]
async fn live_media_fanout_keeps_upstream_bounded() {
    let swarm = swarm().await;
    swarm.start().await;
    let _pump = swarm.pump(1);
    let mut direct = Vec::new();
    for _ in 0..MAX_DIRECT_VIEWERS {
        let peer = swarm.peer().await;
        swarm.watch(&peer).await;
        swarm
            .collect_until(&peer, |chunks| !chunks.is_empty())
            .await;
        direct.push(peer);
    }
    let mut ids: Vec<_> = direct.iter().map(Peer::id).collect();
    ids.sort();

    // 満員の配信者は、資格を確かめた相手に直接の子の一覧と経路を添えて断る。
    let helper = swarm.peer().await;
    let full = swarm
        .subscribe(&helper, &swarm.broadcaster)
        .await
        .err()
        .expect("full");
    let (code, detail) = denial(&full).expect("denied");
    assert_eq!(code, DENIED_FULL);
    let mut links: FanoutLinks = serde_json::from_str(detail).expect("links");
    links.children.sort();
    assert_eq!(links.route, [swarm.broadcaster.id()]);
    assert_eq!(links.children, ids);

    // 視聴中の視聴者は 2 人まで転送し(許可に経路を添える)、3 人目は子の一覧を添えて断る。
    let forwarder = &direct[0];
    let mut held = Vec::new();
    for _ in 0..MAX_FORWARDED_VIEWERS {
        let subscription = swarm
            .subscribe(&helper, forwarder)
            .await
            .expect("forwarded");
        let links: FanoutLinks = serde_json::from_str(subscription.detail()).expect("route");
        assert_eq!(links.route, [swarm.broadcaster.id(), forwarder.id()]);
        held.push(subscription);
    }
    let full = swarm
        .subscribe(&helper, forwarder)
        .await
        .err()
        .expect("forwarder full");
    let (code, detail) = denial(&full).expect("denied");
    assert_eq!(code, DENIED_FULL);
    let links: FanoutLinks = serde_json::from_str(detail).expect("links");
    assert_eq!(links.children, [helper.id(), helper.id()]);
    // 祖先(ここでは配信者)からの購読は断る(循環の防止)。
    let ancestor = swarm
        .subscribe(&swarm.broadcaster, forwarder)
        .await
        .err()
        .expect("ancestor");
    assert_eq!(denial_code(&ancestor), Some(DENIED_ANCESTOR));
    drop(held);

    // 視聴者を増やすと配信者の子の一覧から転送を受け、配信者の子は 4 人、各視聴者の子は 2 人のまま届く。
    for _ in 0..3 {
        let peer = swarm.peer().await;
        swarm.watch(&peer).await;
        let (state, chunks) = swarm
            .collect_until(&peer, |chunks| !chunks.is_empty())
            .await;
        assert_eq!(state, LiveWatchState::Watching.code());
        assert!(chunks.iter().all(|(_, _, index)| *index == 1));
        let route = swarm.links(&peer).await.route;
        assert_eq!(route.len(), 3, "forwarded by one direct viewer");
        assert!(ids.contains(&route[1]));
    }
    assert_eq!(
        swarm.links(&swarm.broadcaster).await.children.len(),
        MAX_DIRECT_VIEWERS
    );
    for peer in &direct {
        assert!(swarm.links(peer).await.children.len() <= MAX_FORWARDED_VIEWERS);
    }
    swarm.shutdown().await;

    // 参加の問い合わせは 16 段で止まり、空きが無いと示す(どこまでも満員を返す配信者の端末)。
    let endless = super::swarm().await;
    let asked = Arc::new(AtomicUsize::new(0));
    let links = FanoutLinks {
        route: vec![endless.broadcaster.id()],
        children: vec![endless.broadcaster.id()],
    };
    let full = format!(
        "{DENIED_FULL}\n{}",
        serde_json::to_string(&links).expect("links")
    );
    let counting = Arc::clone(&asked);
    endless
        .broadcaster
        .node
        .install_media_authorizer(Some(Arc::new(move |_, _| {
            counting.fetch_add(1, Ordering::AcqRel);
            Box::pin(std::future::ready(Err(full.clone())))
        })));
    endless.watch(&endless.viewer).await;
    let (state, _) = endless.collect_until(&endless.viewer, |_| false).await;
    assert_eq!(state, LiveWatchState::Full.code());
    assert_eq!(asked.load(Ordering::Acquire), MAX_JOIN_STEPS);
    endless.shutdown().await;
}

/// 転送する視聴者が中身を変えた frame は画面へ渡さずに捨て、その親から離れて参加し直す(5 秒に 1 回まで)。
#[tokio::test]
async fn live_media_drops_frames_without_broadcaster_signature() {
    let swarm = swarm().await;
    swarm.start().await;
    swarm.push(1, true).await;
    let mut tamperer = swarm.fake_parent(usize::MAX).await;
    tamperer.video.push(99, true);
    swarm.watch(&swarm.viewer).await;
    // 配信者は満員(子は改ざんする視聴者だけ)なので、参加し直しても同じ親に付き、また離れる。
    let (state, chunks) = swarm
        .collect_until(&swarm.viewer, |_| tamperer.granted() >= 3)
        .await;
    assert_eq!(state, LiveWatchState::Watching.code());
    assert!(
        chunks.is_empty(),
        "a changed frame never reaches the screen"
    );
    tamperer.assert_rejoined_at_most_every(REJOIN_INTERVAL);

    // 配信者に空きができれば、次の参加し直しで配信者から直接受ける。
    tamperer.slots.pop();
    let _pump = swarm.pump(7);
    let (state, chunks) = swarm
        .collect_until(&swarm.viewer, |chunks| !chunks.is_empty())
        .await;
    assert_eq!(state, LiveWatchState::Watching.code());
    assert!(!chunks.is_empty());
    assert!(chunks.iter().all(|(_, _, index)| *index == 7), "{chunks:?}");
    assert_eq!(
        swarm.links(&swarm.viewer).await.route,
        [swarm.broadcaster.id(), swarm.viewer.id()]
    );
    swarm.shutdown().await;
}

/// 親が 5 秒止まったら、また親を失ったら、配信者から参加し直す(5 秒に 1 回まで)。自分の子は付いたまま移る。
#[tokio::test]
async fn live_media_rejoins_after_parent_loss_within_rate_limit() {
    // 5 秒の停止: 転送する親 S が接続と公開を残したまま止まると、V は 5 秒後に(空いた)配信者へ参加し直す。V が先に
    // 子への公開を取り下げるので、V の子 C は参加し直さずに待ち、付いたまま移る。
    let swarm = swarm().await;
    swarm.start().await;
    swarm.push(1, true).await;
    let mut hung = swarm.fake_parent(1).await;
    let sending = hung.pump(2);
    let child = swarm.peer().await;
    for peer in [&swarm.viewer, &child] {
        swarm.watch(peer).await;
        swarm.collect_until(peer, |chunks| !chunks.is_empty()).await;
    }
    let route = [&swarm.broadcaster, &hung.peer, &swarm.viewer, &child].map(Peer::id);
    assert_eq!(swarm.links(&child).await.route, route);
    drop(sending);
    let stopped = Instant::now();
    let relay = swarm.fanout(&swarm.viewer).await;
    while relay.announced.load(Ordering::Acquire) {
        assert!(
            stopped.elapsed() < STALL - Duration::from_secs(1),
            "the viewer withdraws from its children before it gives up on the parent"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(relay);
    hung.slots.pop();
    let _pump = swarm.pump(3);
    let moved = assert_child_stays(&swarm, &child, &route).await;
    assert!(
        moved + Duration::from_millis(250) >= stopped + STALL,
        "rejoined {:?} after the parent stopped",
        moved - stopped
    );
    swarm.shutdown().await;

    // 親の喪失: 配信者 B の子は 4 人(3 枠は転送しない端末)で、F が V を、V が C を転送している。F が抜けると、V は
    // 空いた B へ参加し直し、C は V に付いたまま届き続ける。
    let swarm = super::swarm().await;
    swarm.start().await;
    let _pump = swarm.pump(1);
    let filler = swarm.peer().await;
    let mut held = Vec::new();
    for _ in 1..MAX_DIRECT_VIEWERS {
        held.push(
            swarm
                .subscribe(&filler, &swarm.broadcaster)
                .await
                .expect("slot"),
        );
    }
    let (parent, child) = (swarm.peer().await, swarm.peer().await);
    for (peer, fill) in [(&parent, false), (&swarm.viewer, true), (&child, false)] {
        swarm.watch(peer).await;
        swarm.collect_until(peer, |chunks| !chunks.is_empty()).await;
        // F の残りの枠を埋め、C が V へ回るようにする。
        if fill {
            held.push(swarm.subscribe(&filler, &parent).await.expect("slot"));
        }
    }
    let route = [&swarm.broadcaster, &parent, &swarm.viewer, &child].map(Peer::id);
    assert_eq!(swarm.links(&child).await.route, route);
    parent.live.stop_watching(&swarm.session_id).await;
    assert_child_stays(&swarm, &child, &route).await;
    drop(held);
    swarm.shutdown().await;
}

/// 視聴者 V が配信者へ参加し直した後も、V の子 `child` は経路を変えずに(参加し直さずに)届き続ける。V が参加し直した
/// のを見た時刻を返す。
async fn assert_child_stays(swarm: &Swarm, child: &Peer, route: &[String]) -> Instant {
    let rejoined = [swarm.broadcaster.id(), swarm.viewer.id()];
    let mut moved = None;
    for _ in 0..80 {
        if swarm.links(&swarm.viewer).await.route == rejoined {
            moved = Some(Instant::now());
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let moved = moved.expect("the viewer rejoined the broadcaster");
    swarm.next(child).await;
    let (state, chunks) = swarm
        .collect_until(child, |chunks| !chunks.is_empty())
        .await;
    assert_eq!(state, LiveWatchState::Watching.code());
    assert!(
        !chunks.is_empty(),
        "the child keeps receiving after its parent moved"
    );
    assert_eq!(
        swarm.links(child).await.route,
        route,
        "the child stayed attached instead of rejoining"
    );
    moved
}

/// 転送する視聴者のふりをする端末: 配信者の枠を 4 つとも取り(配信者からは受けない)、子へは自前の broadcast を
/// 公開する。子は `limit` 人まで(超えたら子の一覧を添えて満員と断る)。許可した時刻を記録する。
struct FakeParent {
    peer: Peer,
    grants: Arc<StdMutex<Vec<Instant>>>,
    video: Arc<FakeVideo>,
    slots: Vec<MediaSubscription>,
    _broadcast: moq_net::broadcast::Producer,
    _fanout: Arc<Fanout>,
}

/// 配信者の鍵で署名した keyframe を置く映像の track。
struct FakeVideo {
    signer: LiveMediaSigner,
    catalog: moq_mux::catalog::Producer,
    track: StdMutex<moq_mux::container::Producer<Container, hang::catalog::VideoConfig>>,
}

impl Swarm {
    async fn fake_parent(&self, limit: usize) -> FakeParent {
        let peer = self.peer().await;
        let fanout = Fanout::new(TOPIC.into(), 0, Vec::new());
        let mut broadcast = fanout
            .origin
            .create_broadcast(live_media_broadcast_path(&self.session_id))
            .expect("broadcast");
        let catalog =
            moq_mux::catalog::Producer::new(&mut broadcast, moq_mux::catalog::Config::default())
                .expect("catalog");
        let track = broadcast
            .create_track(
                LIVE_VIDEO_TRACK,
                catalog.track_info(hang::catalog::PRIORITY.video),
            )
            .expect("track");
        let track = catalog
            .video(
                track,
                Container::Legacy(moq_mux::container::Kind::Video),
                video_config(LiveVideoSource::Camera.limits()),
            )
            .expect("video");
        broadcast
            .announce(origin::Route::default())
            .expect("announce");
        let grants = Arc::<StdMutex<Vec<Instant>>>::default();
        let (granted, publish) = (Arc::clone(&grants), fanout.origin.consume());
        let route = vec![self.broadcaster.id(), peer.id()];
        let children = StdMutex::new(Vec::new());
        peer.node
            .install_media_authorizer(Some(Arc::new(move |remote, _| {
                let mut children = children.lock().expect("children");
                let answer = if children.len() < limit {
                    children.push(remote.to_string());
                    granted.lock().expect("grants").push(Instant::now());
                    Ok(MediaGrant {
                        publish: publish.consume(),
                        revalidate: Arc::new(|| Box::pin(std::future::ready(true))),
                        detail: serde_json::to_string(&FanoutLinks {
                            route: route.clone(),
                            children: Vec::new(),
                        })
                        .expect("links"),
                    })
                } else {
                    let links = FanoutLinks {
                        route: route.clone(),
                        children: children.clone(),
                    };
                    Err(format!(
                        "{DENIED_FULL}\n{}",
                        serde_json::to_string(&links).expect("links")
                    ))
                };
                Box::pin(std::future::ready(answer))
            })));
        let mut slots = Vec::new();
        for _ in 0..MAX_DIRECT_VIEWERS {
            slots.push(
                self.subscribe(&peer, &self.broadcaster)
                    .await
                    .expect("slot"),
            );
        }
        let signer = self
            .broadcaster
            .app
            .live_media_signer(&self.session_id)
            .expect("signer");
        FakeParent {
            peer,
            grants,
            video: Arc::new(FakeVideo {
                signer,
                catalog,
                track: StdMutex::new(track),
            }),
            slots,
            _broadcast: broadcast,
            _fanout: fanout,
        }
    }
}

impl FakeVideo {
    /// 1 byte 目を `index` にした keyframe を置く。`tamper` なら署名の後で中身を変える。
    fn push(&self, index: u8, tamper: bool) {
        let timestamp_us = micros(self.catalog.clock().now());
        let mut payload = self
            .signer
            .sign(LIVE_VIDEO_TRACK, timestamp_us, &[index, 0, 0, 0]);
        if tamper {
            *payload.last_mut().expect("payload") ^= 1;
        }
        self.track
            .lock()
            .expect("track")
            .write(moq_mux::container::Frame {
                timestamp: Timestamp::from_micros(timestamp_us).expect("timestamp"),
                duration: None,
                payload: payload.into(),
                keyframe: true,
            })
            .expect("write");
    }
}

impl FakeParent {
    /// 100 ms ごとに keyframe を置き続ける(落とすと止まる。接続・公開・track は残る)。
    fn pump(&self, index: u8) -> AbortOnDropHandle<()> {
        let video = Arc::clone(&self.video);
        AbortOnDropHandle::new(n0_future::task::spawn(async move {
            loop {
                video.push(index, false);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }))
    }

    fn granted(&self) -> usize {
        self.grants.lock().expect("grants").len()
    }

    /// 参加し直しの間隔(許可の時刻の差)が `interval` を下回らない(timer の揺れを 50 ms まで許す)。
    fn assert_rejoined_at_most_every(&self, interval: Duration) {
        let grants = self.grants.lock().expect("grants");
        for pair in grants.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                gap + Duration::from_millis(50) >= interval,
                "rejoined after {gap:?}"
            );
        }
    }
}
