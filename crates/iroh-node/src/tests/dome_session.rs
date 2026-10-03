//! #1527: 所有者の端末の Dome host への P2P の session 経路(実 iroh の 2 node)。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use kukuri_core::DOME_SESSION_REQUEST_MAX_BYTES;

use crate::{DomeHostUnreachable, DomeSessionHandler, IrohDocsNode};

fn echo(calls: Arc<AtomicUsize>) -> DomeSessionHandler {
    Arc::new(move |request| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { [b"echo:".as_slice(), &request].concat() })
    })
}

fn unreachable(result: Result<Vec<u8>>) -> bool {
    result
        .err()
        .is_some_and(|error| error.downcast_ref::<DomeHostUnreachable>().is_some())
}

/// participant は host の endpoint id だけで接続し、要求ごとに応答を受け取る。受け口の無い host・上限を超える要求と
/// 応答は、接続できない失敗になり、host の受け口は呼ばれない。
#[tokio::test]
async fn a_participant_reaches_only_an_installed_host_handler_within_the_limits() -> Result<()> {
    let host = IrohDocsNode::memory().await?;
    let participant = IrohDocsNode::memory().await?;
    participant
        .discovery()
        .add_endpoint_info(host.endpoint().addr());
    let host_id = host.endpoint().id().to_string();

    assert!(
        unreachable(participant.dome_session_request(&host_id, b"join", 1024).await),
        "a host without the handler does not answer"
    );

    let calls = Arc::new(AtomicUsize::new(0));
    host.install_dome_session_handler(echo(calls.clone()));
    for request in [b"join".as_slice(), b"move"] {
        let response = participant
            .dome_session_request(&host_id, request, 1024)
            .await?;
        assert_eq!(response, [b"echo:".as_slice(), request].concat());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let oversized = vec![0; DOME_SESSION_REQUEST_MAX_BYTES + 1];
    assert!(unreachable(
        participant
            .dome_session_request(&host_id, &oversized, 1024)
            .await
    ));
    assert!(unreachable(
        participant.dome_session_request(&host_id, b"move", 4).await
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 3, "the oversized request never reached the handler");

    participant.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}
