use super::*;

#[tokio::test]
async fn missing_blob_does_not_penalize_a_reachable_peer() {
    let client = IrohDocsNode::memory().await.unwrap();
    let provider = IrohDocsNode::memory().await.unwrap();
    let peers = Arc::new(PeerAddrBook::new(
        client.endpoint().clone(),
        client.discovery(),
    ));
    peers
        .insert_imported_peer_addr(provider.endpoint().addr())
        .await
        .unwrap();
    for (index, mode) in [
        FetchMode::Store,
        FetchMode::Ephemeral,
        FetchMode::EphemeralBounded(1024),
    ]
    .into_iter()
    .enumerate()
    {
        let retries = Arc::new(Mutex::new(RemoteFetchRetryState::default()));
        let hash = iroh_blobs::Hash::new(format!("missing-{index}"));
        let result = fetch_bytes_with_cooldown_mode(
            &client,
            &peers,
            &retries,
            "missing test blob",
            &hash.to_string(),
            hash,
            "missing locally",
            mode,
        )
        .await
        .unwrap();
        assert_eq!(result, None);
    }
    let state = peers
        .peer_state_snapshot(provider.endpoint().id())
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    provider.shutdown().await.unwrap();
    assert_eq!(
        state.fetch_failures, 0,
        "object absence is not a peer transport failure"
    );
    assert_eq!(state.consecutive_fetch_failures, 0);
    assert_eq!(state.connection_status, PeerConnectionStatus::Connected);
    assert_eq!(
        state.fetch_misses + state.fetch_rejections,
        3,
        "one application response per mode, without retrying this endpoint's other address"
    );
}

#[tokio::test]
async fn local_blob_store_failure_does_not_penalize_the_provider() {
    let client = IrohDocsNode::memory().await.unwrap();
    let provider = IrohDocsNode::memory().await.unwrap();
    let tag = provider
        .blobs()
        .blobs()
        .add_bytes(b"available remotely".to_vec())
        .await
        .unwrap();
    let peers = Arc::new(PeerAddrBook::new(
        client.endpoint().clone(),
        client.discovery(),
    ));
    peers
        .insert_imported_peer_addr(provider.endpoint().addr())
        .await
        .unwrap();
    client.blobs().shutdown().await.unwrap();
    let retries = Arc::new(Mutex::new(RemoteFetchRetryState::default()));
    let result = fetch_bytes_with_cooldown_mode(
        &client,
        &peers,
        &retries,
        "local failure test",
        &tag.hash.to_string(),
        tag.hash,
        "local actor closed",
        FetchMode::Store,
    )
    .await;
    let state = peers
        .peer_state_snapshot(provider.endpoint().id())
        .await
        .unwrap();
    assert!(
        result.is_err(),
        "local storage failure must propagate instead of trying more peers"
    );
    assert_eq!(state.fetch_failures, 0);
    assert_eq!(state.consecutive_fetch_failures, 0);
    let _ = client.shutdown().await; // The deliberately closed local store cannot flush again.
    provider.shutdown().await.unwrap();
}

#[test]
fn queued_fetch_diagnostic_labels_are_bounded_utf8() {
    let value = "あいうえお".repeat(1_000);
    let label = bounded_fetch_log_text(&value, 128);
    assert!(label.len() <= 128);
    assert!(value.starts_with(&label));
    assert_eq!(bounded_fetch_log_text("local miss", 128), "local miss");
}

#[tokio::test]
async fn display_admission_wait_is_included_in_total_budget() {
    let node = IrohDocsNode::memory().await.unwrap();
    let peers = Arc::new(PeerAddrBook::new(node.endpoint().clone(), node.discovery()));
    let retries = Mutex::new(RemoteFetchRetryState::default());
    let permits = retries.lock().await.walk_permits();
    let occupied = permits
        .acquire_many_owned(kukuri_transport::REMOTE_FETCH_MAX_CONCURRENT_WALKS as u32)
        .await
        .unwrap();
    tokio::time::pause();
    let prepared = timeout(
        REMOTE_FETCH_TOTAL_TIMEOUT + Duration::from_secs(1),
        prepare_display_fetch(&node, &peers, &retries, iroh_blobs::Hash::new(b"waiting")),
    )
    .await;
    tokio::time::resume();
    drop(occupied);
    node.shutdown().await.unwrap();
    assert!(
        matches!(prepared, Ok(Err(_))),
        "display admission must expire within its own budget, before an outer caller timeout"
    );
}

#[tokio::test]
async fn display_admission_is_shared_across_services_using_one_node() {
    let node = IrohDocsNode::memory().await.unwrap();
    let peers = Arc::new(PeerAddrBook::new(node.endpoint().clone(), node.discovery()));
    let mut prepared = Vec::new();
    for _ in 0..kukuri_transport::work_admission::WorkLimits::default().running {
        // Different services have different legacy retry ledgers. Their
        // combined display work must nevertheless share the node budget.
        let retries = Mutex::new(RemoteFetchRetryState::default());
        prepared.push(
            prepare_display_fetch(
                &node,
                &peers,
                &retries,
                iroh_blobs::Hash::new(b"shared-node"),
            )
            .await
            .unwrap(),
        );
    }
    let retries = Mutex::new(RemoteFetchRetryState::default());
    tokio::time::pause();
    let next = timeout(
        REMOTE_FETCH_TOTAL_TIMEOUT + Duration::from_secs(1),
        prepare_display_fetch(&node, &peers, &retries, iroh_blobs::Hash::new(b"next")),
    )
    .await;
    tokio::time::resume();
    assert!(
        matches!(next, Ok(Err(_))),
        "per-service ledgers must not multiply display slots"
    );
    drop(prepared);
    let next = prepare_display_fetch(
        &node,
        &peers,
        &retries,
        iroh_blobs::Hash::new(b"new-demand"),
    )
    .await
    .unwrap();
    drop(next);
    node.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn total_budget_cancels_a_fetch_that_never_completes() {
    let result = within_remote_fetch_budget(async {
        tokio::time::sleep(REMOTE_FETCH_TOTAL_TIMEOUT + Duration::from_secs(1)).await;
        1_u8
    })
    .await;
    assert_eq!(result, None);
}

use std::sync::atomic::{AtomicUsize, Ordering};

use kukuri_transport::REMOTE_FETCH_MAX_CONCURRENT_WALKS;

struct FetchFixture {
    retries: Arc<Mutex<RemoteFetchRetryState>>,
    admission: Arc<NetworkWorkRuntime>,
}

impl std::ops::Deref for FetchFixture {
    type Target = Arc<Mutex<RemoteFetchRetryState>>;
    fn deref(&self) -> &Self::Target {
        &self.retries
    }
}

fn retries() -> FetchFixture {
    FetchFixture {
        retries: Arc::new(Mutex::new(RemoteFetchRetryState::default())),
        admission: Arc::new(NetworkWorkRuntime::default()),
    }
}

async fn run_single_flight<F>(
    fixture: &FetchFixture,
    retry_key: &str,
    flight_key: &str,
    subject: &str,
    hash_text: &str,
    walk: F,
) -> Option<SharedRemoteFetchResult>
where
    F: Future<Output = Result<Option<Vec<u8>>>> + Send + 'static,
{
    let mode = if let Some(limit) = flight_key
        .strip_prefix("bounded:")
        .and_then(|key| key.split(':').next())
        .and_then(|limit| limit.parse().ok())
    {
        FetchMode::EphemeralBounded(limit)
    } else if flight_key.starts_with("ephemeral:") {
        FetchMode::Ephemeral
    } else {
        FetchMode::Store
    };
    super::run_single_flight(
        &fixture.admission,
        &fixture.retries,
        retry_key,
        flight_key,
        subject,
        hash_text,
        mode,
        walk,
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn display_fetch_does_not_continue_io_after_caller_timeout() {
    let writes = Arc::new(AtomicUsize::new(0));
    let after = writes.clone();
    let walk = async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        after.fetch_add(1, Ordering::SeqCst);
        Ok(Some(vec![1_u8]))
    };
    let result = timeout(Duration::from_secs(1), run_display_fetch(walk)).await;
    assert!(result.is_err());
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        writes.load(Ordering::SeqCst),
        0,
        "a closed display must not cause later I/O"
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_display_waiter_never_starts_a_later_walk() {
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let waiting = {
        let permits = permits.clone();
        let started = started.clone();
        run_display_fetch(async move {
            let _permit = permits.acquire().await?;
            started.fetch_add(1, Ordering::SeqCst);
            Ok(Some(vec![1]))
        })
    };
    assert!(timeout(Duration::from_secs(1), waiting).await.is_err());
    permits.add_permits(1);
    tokio::time::advance(REMOTE_FETCH_TOTAL_TIMEOUT).await;
    tokio::task::yield_now().await;
    assert_eq!(started.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn displayed_transfer_can_finish_after_the_old_projection_timeout() {
    let bytes = run_display_fetch(async {
        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok(Some(vec![1]))
    })
    .await
    .unwrap();
    assert_eq!(bytes, Some(vec![1]));
}

/// 応答しない peer を模した走査。開始回数だけを数え、総予算まで完了しない。
fn stalled_walk(
    started: &Arc<AtomicUsize>,
) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send + 'static {
    let started = Arc::clone(started);
    async move {
        started.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
        Ok(None)
    }
}

// #1207 TR-10: 呼び出し側が外側の timeout で待つのをやめても、失敗のクールダウンが残る。
#[tokio::test(start_paused = true)]
async fn dropped_caller_still_records_the_failure_cooldown() {
    let retries = retries();
    let started = Arc::new(AtomicUsize::new(0));

    let abandoned = timeout(
        Duration::from_secs(2),
        run_single_flight(
            &retries,
            "hash-a",
            "store:hash-a",
            "blob",
            "hash-a",
            stalled_walk(&started),
        ),
    )
    .await;
    assert!(
        abandoned.is_err(),
        "the caller gives up before the walk ends"
    );

    // 走査の総予算が尽きるまで進める。走査 task は呼び出し側と無関係に終わる。
    tokio::time::sleep(REMOTE_FETCH_TOTAL_TIMEOUT).await;
    tokio::task::yield_now().await;
    assert_eq!(retries.admission.fetch_count(), 0);

    let next = run_single_flight(
        &retries,
        "hash-a",
        "store:hash-a",
        "blob",
        "hash-a",
        stalled_walk(&started),
    )
    .await;
    assert!(next.is_none(), "the next call must be inside the cooldown");
    assert_eq!(started.load(Ordering::SeqCst), 1);
}

// #1207 TR-10 / TR-11: 待つのをやめた直後の再要求は、実行中の走査へ合流し新しい走査を始めない。
#[tokio::test(start_paused = true)]
async fn repeated_callers_join_the_walk_in_flight() {
    let retries = retries();
    let started = Arc::new(AtomicUsize::new(0));

    for _ in 0..5 {
        let attempt = timeout(
            Duration::from_secs(3),
            run_single_flight(
                &retries,
                "hash-a",
                "store:hash-a",
                "blob",
                "hash-a",
                stalled_walk(&started),
            ),
        )
        .await;
        assert!(attempt.is_err());
    }
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(retries.admission.fetch_count(), 1);
}

// #1207 TR-11: 合流した全呼び出しが同じ結果を受け取り、成功後は予約もクールダウンも残らない。
#[tokio::test(start_paused = true)]
async fn joined_callers_share_one_result() {
    let retries = retries();
    let started = Arc::new(AtomicUsize::new(0));
    let walk = |started: &Arc<AtomicUsize>| {
        let started = Arc::clone(started);
        async move {
            started.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(Some(vec![1_u8, 2, 3]))
        }
    };

    let (first, second) = tokio::join!(
        run_single_flight(
            &retries,
            "hash-a",
            "store:hash-a",
            "blob",
            "hash-a",
            walk(&started)
        ),
        run_single_flight(
            &retries,
            "hash-a",
            "store:hash-a",
            "blob",
            "hash-a",
            walk(&started)
        ),
    );
    for result in [first, second] {
        let bytes = result.expect("not cooling down").expect("walk succeeded");
        assert_eq!(bytes.as_deref(), Some(&vec![1_u8, 2, 3]));
    }
    assert_eq!(started.load(Ordering::SeqCst), 1);
    let state = retries.lock().await;
    assert_eq!(retries.admission.fetch_count(), 0);
    assert_eq!(state.cooldown_len(), 0);
}

// #1207 INVAR-2: 永続取得と一時取得は合流しない(一時取得の bytes を保存経路へ混ぜない)。
#[tokio::test(start_paused = true)]
async fn store_and_ephemeral_walks_do_not_join() {
    let retries = retries();
    let started = Arc::new(AtomicUsize::new(0));
    for flight_key in ["store:hash-a", "ephemeral:hash-a"] {
        let attempt = timeout(
            Duration::from_secs(1),
            run_single_flight(
                &retries,
                "hash-a",
                flight_key,
                "blob",
                "hash-a",
                stalled_walk(&started),
            ),
        )
        .await;
        assert!(attempt.is_err());
    }
    assert_eq!(started.load(Ordering::SeqCst), 2);
}

// #1207 INVAR-3 / #1221 NW-4: 同時実行を制限し、待機需要が消えた分は後から起動しない。
#[tokio::test(start_paused = true)]
async fn concurrent_walks_are_bounded() {
    let retries = retries();
    let started = Arc::new(AtomicUsize::new(0));
    let total = REMOTE_FETCH_MAX_CONCURRENT_WALKS + 3;
    for index in 0..total {
        let key = format!("hash-{index}");
        let flight_key = format!("store:{key}");
        let attempt = timeout(
            Duration::from_millis(10),
            run_single_flight(
                &retries,
                &key,
                &flight_key,
                "blob",
                &key,
                stalled_walk(&started),
            ),
        )
        .await;
        assert!(attempt.is_err());
    }
    assert_eq!(
        started.load(Ordering::SeqCst),
        REMOTE_FETCH_MAX_CONCURRENT_WALKS
    );

    tokio::time::sleep(REMOTE_FETCH_TOTAL_TIMEOUT).await;
    tokio::task::yield_now().await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        REMOTE_FETCH_MAX_CONCURRENT_WALKS,
        "cancelled queued callers must not start later I/O"
    );
}

// 合流した側にも、大きさ超過を同じ型で返す(CN scan が型で判定する)。
#[tokio::test(start_paused = true)]
async fn shared_error_keeps_the_too_large_type() {
    let retries = retries();
    let result = run_single_flight(
        &retries,
        "bounded:8:hash-a",
        "bounded:8:hash-a",
        "blob",
        "hash-a",
        async { Err(BlobTooLarge { limit: 8 }.into()) },
    )
    .await
    .expect("not cooling down");
    let error = result.expect_err("walk failed");
    assert!(error.downcast_ref::<BlobTooLarge>().is_some());
}
