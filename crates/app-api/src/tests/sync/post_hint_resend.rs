//! #2030: 投稿の応答は hint の送信を待たない。hint は間を空けて 3 回送り、account の停止で残りをやめる。

use super::*;

const TOPIC: &str = "kukuri:topic:post-hint-resend";

fn app_with_hints() -> (AppService, Arc<TrackingHintTransport>) {
    let store = Arc::new(MemoryStore::default());
    let hints = Arc::new(TrackingHintTransport::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    (app, hints)
}

#[tokio::test(start_paused = true)]
async fn a_post_returns_before_its_hint_is_sent_three_times() {
    let (app, hints) = app_with_hints();

    let start = tokio::time::Instant::now();
    app.create_post(TOPIC, "post", None).await.expect("post");
    assert!(start.elapsed() < Duration::from_millis(250));

    // 投稿の後の時刻と、それまでに送った数。
    for (at, sent) in [(1, 1), (249, 1), (251, 2), (749, 2), (751, 3), (60_000, 3)] {
        tokio::time::sleep_until(start + Duration::from_millis(at)).await;
        assert_eq!(
            hints.published_count.load(Ordering::SeqCst),
            sent,
            "{at} ms"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_the_remaining_sends_of_a_post_hint() {
    let (app, hints) = app_with_hints();

    app.create_post(TOPIC, "post", None).await.expect("post");
    tokio::time::sleep(Duration::from_millis(1)).await;
    app.shutdown().await;
    let sent = hints.published_count.load(Ordering::SeqCst);

    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(hints.published_count.load(Ordering::SeqCst), sent);
}
