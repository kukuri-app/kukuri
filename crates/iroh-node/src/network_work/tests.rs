use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn owner() -> Arc<NetworkWorkRuntime> {
    Arc::new(NetworkWorkRuntime::new(WorkLimits {
        scopes: 3,
        requests: 3,
        running: 1,
        ..WorkLimits::default()
    }))
}

#[tokio::test(start_paused = true)]
async fn queued_network_work_is_bounded_and_dropped_waiters_do_not_run() {
    let owner = owner();
    let deadline = Instant::now() + Duration::from_secs(30);
    let first = owner.acquire([1; 32], deadline).await.unwrap();
    let mut second = Box::pin(owner.acquire([2; 32], deadline));
    let mut third = Box::pin(owner.acquire([3; 32], deadline));
    assert!(futures_util::poll!(&mut second).is_pending());
    assert!(futures_util::poll!(&mut third).is_pending());
    assert!(matches!(
        owner.acquire([4; 32], deadline).await,
        Err(NetworkAdmissionError::Deferred)
    ));
    drop(second);
    drop(first);
    let third = third.await.unwrap();
    assert_eq!(owner.state.lock().unwrap().policy.usage().running, 1);
    assert!(third.finish());
    let state = owner.state.lock().unwrap();
    assert_eq!(state.policy.usage(), Default::default());
    assert!(state.scopes.is_empty() && state.ready.is_empty() && state.cancelled.is_empty());
}

#[tokio::test(start_paused = true)]
async fn bounded_blob_and_docs_work_share_the_display_slot_and_stop_on_close() {
    let owner = owner();
    let deadline = Instant::now() + Duration::from_secs(30);
    let display = owner.acquire([1; 32], deadline).await.unwrap();
    let mut offer = Box::pin(owner.acquire_bounded_blob([2; 32], 65_536, deadline));
    assert!(futures_util::poll!(&mut offer).is_pending());
    let mut docs_request = request(3, false);
    docs_request.protocol = WorkProtocol::Docs;
    docs_request.persistence = WorkPersistence::Ephemeral;
    docs_request.byte_limit = 1024 * 1024;
    let docs = owner
        .submit_fetch(
            docs_request,
            Box::pin(async { Ok(Some(vec![3])) }),
            Box::new(|_| Box::pin(async {})),
        )
        .unwrap();
    let mut docs = Box::pin(docs.result());
    let docs_poll = futures_util::poll!(&mut docs);
    assert!(
        docs_poll.is_pending(),
        "docs completed before slot: {docs_poll:?}"
    );
    assert!(display.finish());
    assert_eq!(docs.await.unwrap().as_deref(), Some(&vec![3]));
    let offer = offer.await.unwrap();
    assert_eq!(owner.state.lock().unwrap().policy.usage().running, 1);
    assert!(offer.finish());
    owner.close();
    assert_eq!(
        owner.state.lock().unwrap().policy.usage(),
        Default::default()
    );
}

#[tokio::test(start_paused = true)]
async fn display_admission_deadline_and_close_discard_prepared_results() {
    let owner = owner();
    let deadline = Instant::now() + Duration::from_secs(30);
    let active = owner.acquire([1; 32], deadline).await.unwrap();
    assert!(matches!(
        owner.acquire([2; 32], deadline).await,
        Err(NetworkAdmissionError::Expired)
    ));
    active.cancelled().await;
    assert!(!active.finish(), "a late result must not leave the adapter");
    let deadline = Instant::now() + Duration::from_secs(30);
    let prepared = owner.acquire([3; 32], deadline).await.unwrap();
    let mut queued = Box::pin(owner.acquire([4; 32], deadline));
    assert!(futures_util::poll!(&mut queued).is_pending());
    owner.close();
    assert!(matches!(queued.await, Err(NetworkAdmissionError::Closed)));
    assert_eq!(
        owner.state.lock().unwrap().policy.usage().running,
        1,
        "prepared work owns its slot until acknowledged"
    );
    prepared.cancelled().await;
    assert!(!prepared.finish());
    assert!(matches!(
        owner.acquire([5; 32], deadline).await,
        Err(NetworkAdmissionError::Closed)
    ));
    assert_eq!(
        owner.state.lock().unwrap().policy.usage(),
        Default::default()
    );
}

#[tokio::test(start_paused = true)]
async fn display_node_close_does_not_stop_another_nodes_work() {
    let first = owner();
    let second = owner();
    let deadline = Instant::now() + Duration::from_secs(30);
    let first_work = first.acquire([1; 32], deadline).await.unwrap();
    let second_work = second.acquire([1; 32], deadline).await.unwrap();
    first.close();
    first_work.cancelled().await;
    assert!(!first_work.finish());
    assert!(second_work.finish());
}

fn request(id: u8, cooling_down: bool) -> FetchRequest {
    FetchRequest {
        identity: FetchIdentity {
            service: 1,
            key: format!("store:{id}"),
        },
        protocol: WorkProtocol::Blob,
        object: [id; 32],
        persistence: WorkPersistence::Store,
        byte_limit: u64::MAX,
        cooling_down,
    }
}

fn docs_request(scope: u8) -> FetchRequest {
    let mut request = request(7, false);
    request.identity = FetchIdentity {
        service: 0,
        key: format!("docs:scope-{scope}"),
    };
    request.protocol = WorkProtocol::Docs;
    request.persistence = WorkPersistence::Ephemeral;
    request.byte_limit = 1024 * 1024;
    request
}

fn finished(counter: &Arc<AtomicUsize>) -> fetch::FetchFinished {
    let counter = counter.clone();
    Box::new(move |_| {
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    })
}

#[tokio::test(start_paused = true)]
async fn ordinary_fetch_waits_for_the_same_capacity_as_display_work() {
    let owner = owner();
    let display = owner
        .acquire([1; 32], Instant::now() + Duration::from_secs(30))
        .await
        .unwrap();
    let started = Arc::new(AtomicUsize::new(0));
    let after = started.clone();
    let completed = Arc::new(AtomicUsize::new(0));
    let fetch = owner
        .submit_fetch(
            request(2, false),
            Box::pin(async move {
                after.fetch_add(1, Ordering::SeqCst);
                Ok(Some(vec![2]))
            }),
            finished(&completed),
        )
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(started.load(Ordering::SeqCst), 0);
    drop(display);
    assert_eq!(fetch.result().await.unwrap().as_deref(), Some(&vec![2]));
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(owner.fetch_count(), 0);
}

#[tokio::test]
async fn docs_reads_join_only_with_the_same_scope_identity() {
    let owner = owner();
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let first_started = started.clone();
    let first_release = release.clone();
    let first = owner
        .submit_fetch(
            docs_request(1),
            Box::pin(async move {
                first_started.fetch_add(1, Ordering::SeqCst);
                first_release.notified().await;
                Ok(Some(vec![1]))
            }),
            Box::new(|_| Box::pin(async {})),
        )
        .unwrap();
    let joined = owner
        .submit_fetch(
            docs_request(1),
            Box::pin(async { panic!("joined docs read must not run twice") }),
            Box::new(|_| Box::pin(async {})),
        )
        .unwrap();
    let other = owner
        .submit_fetch(
            docs_request(2),
            Box::pin(async { Ok(Some(vec![2])) }),
            Box::new(|_| Box::pin(async {})),
        )
        .unwrap();
    assert_eq!(owner.fetch_count(), 2);
    tokio::task::yield_now().await;
    assert_eq!(started.load(Ordering::SeqCst), 1);
    release.notify_one();
    assert_eq!(first.result().await.unwrap().as_deref(), Some(&vec![1]));
    assert_eq!(joined.result().await.unwrap().as_deref(), Some(&vec![1]));
    assert_eq!(other.result().await.unwrap().as_deref(), Some(&vec![2]));
}

#[tokio::test(start_paused = true)]
async fn fetch_waiters_are_bounded_and_join_does_not_extend_the_deadline() {
    let owner = Arc::new(NetworkWorkRuntime::new(WorkLimits {
        waiters_per_request: 2,
        ..WorkLimits::default()
    }));
    let completed = Arc::new(AtomicUsize::new(0));
    let first = owner
        .submit_fetch(
            request(1, false),
            Box::pin(std::future::pending()),
            finished(&completed),
        )
        .unwrap();
    let deadline = Instant::now() + crate::remote_fetch::REMOTE_FETCH_TOTAL_TIMEOUT;
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_secs(20)).await;
    // An existing flight wins over cooldown; a new one must not bypass it.
    let second = owner
        .submit_fetch(
            request(1, true),
            Box::pin(async { panic!("joined work must not run") }),
            finished(&completed),
        )
        .unwrap();
    assert!(matches!(
        owner.submit_fetch(
            request(1, false),
            Box::pin(std::future::pending()),
            finished(&completed)
        ),
        Err(NetworkAdmissionError::Deferred)
    ));
    assert!(matches!(
        owner.submit_fetch(
            request(2, true),
            Box::pin(std::future::pending()),
            finished(&completed)
        ),
        Err(NetworkAdmissionError::CoolingDown)
    ));
    assert_eq!(second.result().await.unwrap(), None);
    assert_eq!(Instant::now(), deadline);
    assert_eq!(first.result().await.unwrap(), None);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn panic_records_completion_and_releases_the_owned_fetch_slot() {
    let owner = owner();
    let completed = Arc::new(AtomicUsize::new(0));
    let fetch = owner
        .submit_fetch(
            request(1, false),
            Box::pin(async { panic!("injected worker panic") }),
            finished(&completed),
        )
        .unwrap();
    assert!(fetch.result().await.is_err());
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(owner.fetch_count(), 0);
    let fetch = owner
        .submit_fetch(
            request(2, false),
            Box::pin(async { Ok(Some(vec![2])) }),
            finished(&completed),
        )
        .unwrap();
    assert_eq!(fetch.result().await.unwrap().as_deref(), Some(&vec![2]));
    owner.close();
}

#[tokio::test(start_paused = true)]
async fn dropping_queued_fetch_does_not_hold_the_lock_while_dropping_its_node() {
    struct CloseOnDrop(std::sync::Weak<NetworkWorkRuntime>);
    impl Drop for CloseOnDrop {
        fn drop(&mut self) {
            if let Some(owner) = self.0.upgrade() {
                owner.close();
            }
        }
    }
    let owner = owner();
    let display = owner
        .acquire([1; 32], Instant::now() + Duration::from_secs(30))
        .await
        .unwrap();
    let guard = CloseOnDrop(Arc::downgrade(&owner));
    let completed = Arc::new(AtomicUsize::new(0));
    let fetch = owner
        .submit_fetch(
            request(2, false),
            Box::pin(async move {
                let _guard = guard;
                panic!("queued work must never run");
            }),
            finished(&completed),
        )
        .unwrap();
    drop(fetch);
    display.cancelled().await;
    assert!(!display.finish());
    assert_eq!(
        completed.load(Ordering::SeqCst),
        0,
        "queue cancellation is not a failed network attempt"
    );
    assert_eq!(owner.fetch_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn node_close_stops_running_fetches_and_discards_queued_work() {
    struct Stopped(Arc<AtomicUsize>);
    impl Drop for Stopped {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let owner = owner();
    let completed = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(AtomicUsize::new(0));
    let after = stopped.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let running = owner
        .submit_fetch(
            request(1, false),
            Box::pin(async move {
                let _stopped = Stopped(after);
                let _ = started.send(());
                std::future::pending().await
            }),
            finished(&completed),
        )
        .unwrap();
    ready.await.unwrap();
    let queued = owner
        .submit_fetch(
            request(2, false),
            Box::pin(async { panic!("queued I/O after close") }),
            finished(&completed),
        )
        .unwrap();
    owner.close();
    assert_eq!(running.result().await.unwrap(), None);
    assert_eq!(queued.result().await.unwrap(), None);
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(owner.fetch_count(), 0);
}

#[tokio::test]
async fn retired_service_identity_cannot_be_reused_before_old_flight_is_removed() {
    let owner = Arc::new(NetworkWorkRuntime::default());
    let old_service = Arc::new(kukuri_transport::RemoteFetchRetryState::default());
    let old_id = old_service.instance_id();
    let (dropped, old_dropped) = tokio::sync::oneshot::channel();
    let (resume, resumed) = tokio::sync::oneshot::channel();
    let callback: fetch::FetchFinished = Box::new(move |_| {
        Box::pin(async move {
            // Only this callback owns the original retry ledger. Hold the flight at
            // the retirement boundary after releasing that last service reference.
            drop(old_service);
            dropped.send(()).unwrap();
            resumed.await.unwrap();
        })
    });
    let mut original = request(1, false);
    original.identity.service = old_id;
    let first = owner
        .submit_fetch(original, Box::pin(async { Ok(Some(vec![1])) }), callback)
        .unwrap();
    old_dropped.await.unwrap();
    assert_eq!(
        owner.fetch_count(),
        1,
        "old identity remains registered during completion"
    );
    let new_service = kukuri_transport::RemoteFetchRetryState::default();
    assert_ne!(new_service.instance_id(), old_id);
    let mut replacement = request(1, false);
    replacement.identity.service = new_service.instance_id();
    let completed = Arc::new(AtomicUsize::new(0));
    let second = owner
        .submit_fetch(
            replacement,
            Box::pin(async { Ok(Some(vec![2])) }),
            finished(&completed),
        )
        .unwrap();
    assert_eq!(
        owner.fetch_count(),
        2,
        "replacement must create its own flight"
    );
    resume.send(()).unwrap();
    assert_eq!(first.result().await.unwrap().as_deref(), Some(&vec![1]));
    assert_eq!(second.result().await.unwrap().as_deref(), Some(&vec![2]));
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    owner.close();
}
