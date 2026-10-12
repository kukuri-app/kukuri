//! ADR 0067: media の受け口(実 iroh の 2 node)。送る側の認可が許した broadcast だけが届き、拒否・取消しでは届かない。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use moq_net::{Timestamp, origin, track};

use crate::{IrohDocsNode, MediaAuthorizer, MediaDenied, MediaGrant};

const PATH: &str = "media-test";

/// `allow` の要求だけを許し、`allowed` が偽になったら評価し直しで取り消す。
fn authorizer(publish: origin::Consumer, allowed: Arc<AtomicBool>) -> MediaAuthorizer {
    Arc::new(move |_remote, request| {
        let publish = publish.clone();
        let allowed = allowed.clone();
        Box::pin(async move {
            if request != b"allow" {
                return Err("access_denied".to_string());
            }
            Ok(MediaGrant {
                publish,
                revalidate: Arc::new(move || {
                    let allowed = allowed.clone();
                    Box::pin(async move { allowed.load(Ordering::SeqCst) })
                }),
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
    let _origin_driver = n0_future::task::AbortOnDropHandle::new(n0_future::task::spawn(
        async move {
            moq_net::time::run(origin_driver).await;
        },
    ));
    let broadcast = origin.publish(PATH, origin::Route::default())?;
    let mut track = broadcast.create_track("frames", None)?;
    let allowed = Arc::new(AtomicBool::new(true));
    publisher.install_media_authorizer(Some(authorizer(origin.consume(), allowed.clone())));

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

    // 取消し: 次の評価し直し(5 秒ごと)で送る側が閉じる。
    allowed.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(15), subscription.closed())
        .await
        .expect("the publisher closes the revoked session");

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
    assert!(result.is_err(), "no session without an installed authorizer");
    listener.shutdown().await?;
    publisher.shutdown().await?;
    Ok(())
}
