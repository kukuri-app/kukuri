//! ADR 0067: media の受け口(実 iroh の 2 node)。送る側の認可が許した broadcast だけが届き、拒否・取消しでは届かない。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use iroh::endpoint::TransportAddrUsage;
use kukuri_transport::TransportRelayConfig;
use moq_net::{Timestamp, origin, track};
use tokio::sync::{Notify, watch};

use crate::dome_session::direct_first_candidates;
use crate::media::MediaRevalidate;
use crate::{
    IrohDocsNode, MediaAuthorizer, MediaDenied, MediaGrant, NodeOptions, media_origin_config,
};

const PATH: &str = "media-test";

/// `allow` の要求だけを許し、渡された評価で継続の可否を確認する。
fn authorizer(publish: origin::Consumer, revalidate: MediaRevalidate) -> MediaAuthorizer {
    Arc::new(move |_remote, request| {
        let publish = publish.clone();
        let revalidate = revalidate.clone();
        Box::pin(async move {
            if request != b"allow" {
                return Err("access_denied".to_string());
            }
            Ok(MediaGrant {
                publish,
                revalidate,
                detail: String::new(),
            })
        })
    })
}

async fn read_one_frame(track: &mut track::Subscriber) -> Result<Vec<u8>> {
    let mut group = tokio::time::timeout(Duration::from_secs(10), track.recv_group())
        .await??
        .expect("a group");
    let frame = tokio::time::timeout(Duration::from_secs(10), group.read_frame())
        .await??
        .expect("a frame");
    Ok(frame.payload.to_vec())
}

#[tokio::test]
async fn media_reaches_only_an_authorized_subscriber_until_access_is_revoked() -> Result<()> {
    let publisher = IrohDocsNode::memory().await?;
    let listener = IrohDocsNode::memory().await?;
    listener
        .discovery()
        .add_endpoint_info(publisher.endpoint().addr());
    let publisher_id = publisher.endpoint().id().to_string();

    let (origin, origin_driver) = origin::Producer::new(origin::Config::default());
    let _origin_driver =
        n0_future::task::AbortOnDropHandle::new(n0_future::task::spawn(async move {
            moq_net::time::run(origin_driver).await;
        }));
    let broadcast = origin.publish(PATH, origin::Route::default())?;
    let mut track = broadcast.create_track("frames", None)?;
    let (allowed, verdict) = watch::channel(None);
    let checking = Arc::new(Notify::new());
    let revalidate: MediaRevalidate = {
        let checking = checking.clone();
        Arc::new(move || {
            let mut verdict = verdict.clone();
            let checking = checking.clone();
            Box::pin(async move {
                checking.notify_one();
                verdict
                    .wait_for(Option::is_some)
                    .await
                    .is_ok_and(|value| *value == Some(true))
            })
        })
    };
    publisher.install_media_authorizer(Some(authorizer(origin.consume(), revalidate)));

    // 拒否: 理由の code だけが返り、session は始まらない。
    let denied = listener
        .media_subscribe(&publisher_id, b"deny")
        .await
        .err()
        .expect("the request is denied");
    let denied = denied
        .downcast_ref::<MediaDenied>()
        .expect("a denial, not a connection failure");
    assert_eq!(denied.0, "access_denied");

    // 許可: 公開された broadcast の frame が届く。
    let subscription = listener.media_subscribe(&publisher_id, b"allow").await?;
    let consumer = subscription.broadcast(PATH).await?;
    let mut frames = consumer
        .track("frames")?
        .subscribe(track::Subscription::default())
        .await?;
    track.write_frame(Timestamp::from_micros(20_000)?, b"first".as_slice())?;
    assert_eq!(read_one_frame(&mut frames).await?, b"first");

    // 再確認(5 秒ごと)の結果を待つ間も、認可済みの搬送を進める。
    tokio::time::timeout(Duration::from_secs(10), checking.notified()).await?;
    track.write_frame(Timestamp::from_micros(40_000)?, b"pending".as_slice())?;
    assert_eq!(read_one_frame(&mut frames).await?, b"pending");

    // 拒否が確定したら閉じ、それ以後に書いた frame は届かない。
    allowed.send(Some(false))?;
    tokio::time::timeout(Duration::from_secs(15), subscription.closed())
        .await
        .expect("the publisher closes the revoked session");
    track.write_frame(Timestamp::from_micros(60_000)?, b"revoked".as_slice())?;
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(1), frames.recv_group()).await,
        Ok(Ok(Some(_)))
    ));

    publisher.install_media_authorizer(None);
    listener.shutdown().await?;
    publisher.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_publisher_without_an_authorizer_serves_nothing() -> Result<()> {
    let publisher = IrohDocsNode::memory().await?;
    let listener = IrohDocsNode::memory().await?;
    listener
        .discovery()
        .add_endpoint_info(publisher.endpoint().addr());
    let result = listener
        .media_subscribe(&publisher.endpoint().id().to_string(), b"allow")
        .await;
    assert!(
        result.is_err(),
        "no session without an installed authorizer"
    );
    listener.shutdown().await?;
    publisher.shutdown().await?;
    Ok(())
}

/// Relay Fallback の試験(relay だけで届く。ADR 0067 §5): 聞き手に IP の transport が無く Direct P2P が成り立たない
/// とき、media は relay を通って届く。接続の候補は、relay URL を含まない Direct の候補を先に置く。
#[tokio::test]
async fn metaverse_audio_relay_fallback_keeps_direct_candidates_relay_free() -> Result<()> {
    let (relay, _relay_server) = kukuri_webrtc_transport::signaling_fixture::spawn_relay().await?;
    let options = |browser_like| NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        browser_like,
        ..NodeOptions::default()
    };
    let publisher = IrohDocsNode::memory_with(options(false)).await?;
    let listener = IrohDocsNode::memory_with(options(true)).await?;
    publisher.endpoint().online().await;
    listener.endpoint().online().await;
    let publisher_addr = publisher.endpoint().addr();
    let candidates = direct_first_candidates(
        publisher_addr.id,
        Some(&publisher_addr),
        &listener.relay_urls().await,
    );
    assert!(
        candidates[0].relay_urls().next().is_none(),
        "the direct candidate has no relay URL"
    );
    assert!(candidates[0].ip_addrs().next().is_some());
    assert!(candidates[1].relay_urls().next().is_some());
    listener.discovery().add_endpoint_info(publisher_addr);

    let (origin, origin_driver) = origin::Producer::new(origin::Config::default());
    let _origin_driver =
        n0_future::task::AbortOnDropHandle::new(n0_future::task::spawn(async move {
            moq_net::time::run(origin_driver).await;
        }));
    let broadcast = origin.publish(PATH, origin::Route::default())?;
    let mut track = broadcast.create_track("frames", None)?;
    publisher.install_media_authorizer(Some(authorizer(
        origin.consume(),
        Arc::new(|| Box::pin(async { true })),
    )));

    let subscription = listener
        .media_subscribe(&publisher.endpoint().id().to_string(), b"allow")
        .await?;
    let consumer = subscription.broadcast(PATH).await?;
    let mut frames = consumer
        .track("frames")?
        .subscribe(track::Subscription::default())
        .await?;
    track.write_frame(Timestamp::from_micros(20_000)?, b"relayed".as_slice())?;
    assert_eq!(read_one_frame(&mut frames).await?, b"relayed");
    let info = listener
        .endpoint()
        .remote_info(publisher.endpoint().id())
        .await
        .expect("the publisher is known");
    let active: Vec<bool> = info
        .addrs()
        .filter(|addr| matches!(addr.usage(), TransportAddrUsage::Active))
        .map(|addr| addr.addr().is_relay())
        .collect();
    assert!(
        !active.is_empty() && active.iter().all(|relay| *relay),
        "the media path is the relay"
    );

    drop(subscription);
    publisher.install_media_authorizer(None);
    listener.shutdown().await?;
    publisher.shutdown().await?;
    Ok(())
}

/// INVAR-3: media の origin は、読み書きの無い group を cache から外し、保持を 10 秒以下にする(ADR 0067 §2)。
#[tokio::test(start_paused = true)]
async fn media_origin_keeps_at_most_ten_seconds_of_media() -> Result<()> {
    let config = media_origin_config();
    let pool = config.pool.clone();
    let (origin, origin_driver) = origin::Producer::new(config);
    let _origin_driver =
        n0_future::task::AbortOnDropHandle::new(n0_future::task::spawn(async move {
            moq_net::time::run(origin_driver).await;
        }));
    let broadcast = origin.publish(PATH, origin::Route::default())?;
    let mut track = broadcast.create_track("frames", None)?;
    // 20 ms ごとに 100 bytes の group を 60 秒書く。cache に残るのは、最初の 1 秒分の使用量の 10 倍まで。
    let mut first_second = 0;
    for index in 1..=3_000_u64 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        track.write_frame(Timestamp::from_micros(index * 20_000)?, vec![0_u8; 100])?;
        if index == 50 {
            first_second = pool.used();
        }
    }
    assert!(
        pool.used() <= 10 * first_second,
        "{} bytes cached ({first_second} bytes per second)",
        pool.used()
    );
    Ok(())
}
