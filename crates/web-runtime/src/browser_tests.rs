//! ブラウザの IndexedDB に置いた blob が、reload の後も同じ hash・完成状態で読め、native と送受信できる
//! （#1215 W2 AC-2、ADR 0058 §5）。relay だけの経路と、QUIC over WebRTC DataChannel の custom path の両方で確かめる。
//! reload は、node と cache を止めてから、同じ account の database を開き直した新しい node で表す。
//! 中断・書込みの失敗・破損で未完了を完成と扱わず、保護した内容を残したまま非保護分を固定の窓で回収する（W2 AC-3）。
//! Web の blob-service も取得 gate の前提（local-only・ephemeral・保護と cache の境界）を保つ（W2 AC-4）。
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_blob_peer` から実行する。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context as _, Result};
use iroh::endpoint::Connection;
use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use kukuri_blob_service::{BlobService, BlobStatus, IrohBlobService};
use kukuri_core::BlobHash;
use kukuri_iroh_node::{IrohDocsNode, NodeOptions};
use kukuri_store::{
    ContentCacheStore, REMOTE_CACHE_CAPACITY_BYTES, REMOTE_CACHE_RECLAIM_STEP,
    REMOTE_CACHE_UNUSED_MS,
};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use crate::IndexedDbCache;
use crate::content_cache::test_hooks;

#[path = "../tests/support/blob_e2e.rs"]
mod blob_e2e;

wasm_bindgen_test_configure!(run_in_browser);

const MIME: &str = "application/octet-stream";
const MIB: u64 = 1024 * 1024;

struct Native {
    addr: EndpointAddr,
    relay: RelayUrl,
    blob: BlobHash,
}

/// native の相手の node（relay だけで届く宛先）と、そこに置いた blob。
async fn native() -> Result<Native> {
    let info = signaling_fixture::post("/info", "").await?;
    let mut lines = info.lines();
    let id: EndpointId = lines.next().context("id")?.parse()?;
    let relay: RelayUrl = lines.next().context("relay")?.parse()?;
    let blob = BlobHash::new(lines.next().context("blob")?.to_string());
    Ok(Native {
        addr: EndpointAddr::from_parts(id, [TransportAddr::Relay(relay.clone())]),
        relay,
        blob,
    })
}

fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn account(label: &str) -> String {
    let now = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .expect("clock");
    format!("test-{label}-{}", now.as_nanos())
}

/// database を消す。開いたままの接続があると消せずに待つので、期限で失敗にする。
async fn delete_database(account: &str) -> Result<()> {
    let request = web_sys::window()
        .context("window")?
        .indexed_db()
        .map_err(crate::idb::js_error)?
        .context("indexeddb")?
        .delete_database(&format!("kukuri-cache-v1-{account}"))
        .map_err(crate::idb::js_error)?;
    n0_future::time::timeout(Duration::from_secs(10), crate::idb::done(&request))
        .await
        .context("the database is still open")??;
    Ok(())
}

/// 1 回の起動（page の読込み）。
struct Session {
    node: Arc<IrohDocsNode>,
    cache: Arc<IndexedDbCache>,
    blobs: IrohBlobService,
    transport: Option<Arc<WebRtcTransport>>,
    _demand: Option<Connection>,
}

async fn start(native: &Native, account: &str, webrtc: bool) -> Result<Session> {
    let cache = Arc::new(IndexedDbCache::open(account).await?);
    let transport = webrtc.then(|| WebRtcTransport::new(WebRtcConfig {}));
    let node = IrohDocsNode::memory_with(NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![native.relay.to_string()],
        },
        webrtc: transport.clone(),
        ..NodeOptions::default()
    })
    .await?;
    node.install_remote_cache(cache.clone())?;
    let blobs = IrohBlobService::with_content_cache(node.clone(), cache.clone());
    node.endpoint().online().await;
    let demand = match &transport {
        Some(_) => {
            let demand = blob_e2e::demand(&node, native.addr.clone()).await?;
            node.webrtc_signaling()
                .context("webrtc")?
                .connect(native.addr.clone())
                .await?;
            blob_e2e::on_custom(&demand).await?;
            Some(demand)
        }
        None => None,
    };
    Ok(Session {
        node,
        cache,
        blobs,
        transport,
        _demand: demand,
    })
}

impl Session {
    fn received(&self) -> u64 {
        self.transport
            .as_ref()
            .map_or(0, |transport| transport.stats().received_bytes)
    }

    async fn learn(&self, native: &Native) -> Result<()> {
        let id = native.addr.id.to_string();
        self.blobs.learn_peer(&id).await?;
        self.blobs.learn_content_source(&native.blob, &id).await
    }

    /// native の blob を取得し、取得 gate を通った内容として cache へ置く。
    async fn receive(&self, native: &Native) -> Result<String> {
        self.learn(native).await?;
        let before = self.received();
        let bytes = self
            .blobs
            .fetch_blob(&native.blob)
            .await?
            .context("native blob")?;
        let line = blob_e2e::describe(&bytes, self.received() - before);
        self.blobs.put_remote_blob(bytes, MIME).await?;
        Ok(line)
    }

    /// native がこの端から `hash` を取得した結果。
    async fn send(&self, hash: &BlobHash, webrtc: bool) -> Result<String> {
        let body = format!(
            "{}\n{}\n{}",
            self.node.endpoint().id(),
            hash.as_str(),
            u8::from(webrtc)
        );
        signaling_fixture::post("/fetch", &body).await
    }

    async fn stop(self) -> Result<()> {
        self.node.shutdown().await
    }
}

async fn roundtrip(webrtc: bool) {
    let native = native().await.expect("native peer");
    let side = if webrtc {
        "browser-webrtc"
    } else {
        "browser-relay"
    };
    let account = account(side);
    let custom = if webrtc { u64::MAX } else { 0 };

    let first = start(&native, &account, webrtc).await.expect("start");
    let own = first
        .blobs
        .put_blob(blob_e2e::blob(side), MIME)
        .await
        .expect("own blob")
        .hash;
    assert_eq!(
        first.receive(&native).await.expect("receive"),
        blob_e2e::describe(&blob_e2e::blob("native"), custom)
    );
    // blob-service の内容は memory の store に残さない（W2 AC-3）。
    for hash in [&own, &native.blob] {
        assert_eq!(
            first
                .node
                .read_local_blob(hash.as_str())
                .await
                .expect("memory store"),
            None
        );
    }
    first.stop().await.expect("stop");

    // reload: memory の store は空から始まり、内容は IndexedDB からだけ読める。
    let second = start(&native, &account, webrtc).await.expect("restart");
    for (hash, side) in [(&own, side), (&native.blob, "native")] {
        assert_eq!(
            second.blobs.local_blob_status(hash).await.expect("status"),
            BlobStatus::Available
        );
        assert_eq!(
            second.blobs.fetch_local_blob(hash).await.expect("read"),
            Some(blob_e2e::blob(side))
        );
    }
    // native は reload 後の browser から `/kukuri/remote-blob/1` で取得する。
    assert_eq!(
        second.send(&own, webrtc).await.expect("send"),
        blob_e2e::describe(&blob_e2e::blob(side), custom)
    );
    second.stop().await.expect("stop");
    delete_database(&account).await.expect("delete");
}

#[wasm_bindgen_test]
async fn a_saved_blob_survives_a_reload_and_roundtrips_with_native_over_the_relay() {
    roundtrip(false).await;
}

#[wasm_bindgen_test]
async fn a_saved_blob_survives_a_reload_and_roundtrips_with_native_over_the_webrtc_path() {
    roundtrip(true).await;
}

#[wasm_bindgen_test]
async fn chunks_reservations_and_the_database_are_released() {
    let account = account("release");
    let cache = IndexedDbCache::open(&account).await.expect("open");
    let capacity = cache.remote_cache_capacity();

    // 取得を止めた（予約を drop した）分は戻る。
    let mut held = cache.empty_remote_cache_reservation();
    assert!(
        cache
            .reserve_remote_cache_bytes(&mut held, capacity)
            .await
            .expect("reserve")
    );
    let mut other = cache.empty_remote_cache_reservation();
    assert!(
        !cache
            .reserve_remote_cache_bytes(&mut other, 1)
            .await
            .expect("full")
    );
    drop(held);
    // 待つのをやめた予約の処理が後から認めた分も戻る。
    let mut abandoned = cache.empty_remote_cache_reservation();
    assert!(
        n0_future::future::now_or_never(cache.reserve_remote_cache_bytes(&mut abandoned, capacity))
            .is_none()
    );
    assert!(
        cache
            .reserve_remote_cache_bytes(&mut other, capacity)
            .await
            .expect("released")
    );
    drop(other);

    // 保存は 1 MiB の chunk に分かれ、1 回の読み出しも 1 MiB まで。
    let blob = blob_e2e::blob("chunks");
    let hash = blake3::hash(&blob).to_hex().to_string();
    assert!(
        cache
            .put_remote_content("blob", &hash, "blob", &blob)
            .await
            .expect("put")
    );
    assert_eq!(
        cache.chunk_sizes("blob", &hash).await.expect("chunks"),
        vec![MIB as usize, MIB as usize, 7]
    );
    assert!(
        cache
            .remote_content_chunk("blob", &hash, 0, MIB as usize + 1)
            .await
            .is_err()
    );
    assert_eq!(
        cache
            .remote_content_chunk("blob", &hash, MIB - 3, 10)
            .await
            .expect("read"),
        Some(blob[MIB as usize - 3..MIB as usize + 7].to_vec())
    );

    // drop で database を閉じる（閉じていなければ消せない）。
    drop(cache);
    delete_database(&account).await.expect("delete");
}

/// 書込みの中断（quota 超過と同じく transaction が確定しない）では何も残さず、1 処理の回収の後に 1 回だけ書き直す。
/// 書き直しも失敗すれば失敗を返し、保護した内容は残る。次の書込みで完成する。
#[wasm_bindgen_test]
async fn an_aborted_write_stores_nothing_and_is_retried_once_after_a_reclaim() {
    let account = account("abort");
    let cache = IndexedDbCache::start(&account, Some(64 * 1024))
        .await
        .expect("open");
    let item = |n: u8| vec![n; 1024];
    let now = now_ms();
    cache
        .put_owned_blob("own_blob:abort", &hash(&item(0)), &item(0))
        .await
        .expect("own");
    assert!(
        cache
            .put_used_at(&hash(&item(1)), &item(1), now - 2)
            .await
            .expect("oldest")
    );
    assert!(
        cache
            .put_used_at(&hash(&item(2)), &item(2), now - 1)
            .await
            .expect("older")
    );

    test_hooks::fail_writes(1);
    assert!(
        cache
            .put_remote_content("blob", &hash(&item(3)), "blob", &item(3))
            .await
            .expect("retried")
    );
    for (n, kept) in [(0, true), (1, false), (2, true), (3, true)] {
        let has = cache.has_remote_content("blob", &hash(&item(n))).await;
        assert_eq!(has.expect("has"), kept, "item {n}");
    }

    test_hooks::fail_writes(2);
    let (fourth, fourth_key) = (item(4), hash(&item(4)));
    let failed = cache.put_remote_content("blob", &fourth_key, "blob", &fourth);
    assert!(failed.await.is_err());
    let left = cache.chunk_sizes("blob", &hash(&item(4))).await;
    assert!(left.expect("chunks").is_empty());
    let protected = cache.has_remote_content("blob", &hash(&item(0))).await;
    assert!(protected.expect("protected"));
    assert!(
        cache
            .put_remote_content("blob", &hash(&item(4)), "blob", &item(4))
            .await
            .expect("resume")
    );
    let stored = cache.get_remote_content("blob", &hash(&item(4))).await;
    assert_eq!(stored.expect("read"), Some(item(4)));
    drop(cache);
    delete_database(&account).await.expect("delete");
}

/// chunk の欠けた内容は完成と扱わず、消して取り直させる。保護参照は残り、置き直した内容は保護される。
#[wasm_bindgen_test]
async fn a_blob_with_a_missing_chunk_is_not_complete_and_can_be_stored_again() {
    let account = account("corrupt");
    let capacity = 8 * MIB as i64;
    let cache = IndexedDbCache::start(&account, Some(capacity))
        .await
        .expect("open");
    let blob = blob_e2e::blob("corrupt");
    let key = hash(&blob);
    cache
        .put_owned_blob("own_blob:corrupt", &key, &blob)
        .await
        .expect("own");
    cache.drop_second_chunk(&key).await.expect("corrupt");
    let len = cache.remote_content_len("blob", &key).await;
    assert_eq!(len.expect("len"), None);
    assert!(!cache.has_remote_content("blob", &key).await.expect("has"));
    assert_eq!(
        cache.get_remote_content("blob", &key).await.expect("get"),
        None
    );
    assert!(
        cache
            .chunk_sizes("blob", &key)
            .await
            .expect("chunks")
            .is_empty()
    );

    // 保護参照を足し直さずに置き直す（消したのは内容だけで、参照は残っている）。
    let again = cache.put_remote_content("blob", &key, "blob", &blob).await;
    assert!(again.expect("again"));
    assert_eq!(
        cache.get_remote_content("blob", &key).await.expect("get"),
        Some(blob)
    );
    // 保護されているので、容量のすべての予約でも回収されない。
    let mut all = cache.empty_remote_cache_reservation();
    assert!(
        cache
            .reserve_remote_cache_bytes(&mut all, capacity as u64)
            .await
            .expect("reserve")
    );
    assert!(cache.has_remote_content("blob", &key).await.expect("kept"));
    drop(all);
    drop(cache);
    delete_database(&account).await.expect("delete");
}

/// 保存した内容と保護参照の数を増やしても、1 回の回収と読み出しが読む行は固定の窓（128 件・1 件）に収まり、
/// 保護した内容は回収されない。
#[wasm_bindgen_test]
async fn a_reclaim_step_and_a_read_stay_in_a_fixed_window_as_the_cache_grows() {
    let step = REMOTE_CACHE_RECLAIM_STEP;
    for count in [step + 22, 2 * step + 44] {
        let account = account(&format!("window-{count}"));
        let cache = IndexedDbCache::open(&account).await.expect("open");
        assert!(cache.remote_cache_capacity() <= REMOTE_CACHE_CAPACITY_BYTES as u64);
        for n in 0..count {
            let owned = format!("own-{n}");
            cache
                .put_owned_blob(&format!("own_blob:{n}"), &owned, b"own")
                .await
                .expect("own");
        }
        // 失効した非保護分（書込みの中の回収に消されないよう、保護した内容の後に置く）。
        let expired = now_ms() - REMOTE_CACHE_UNUSED_MS - 1;
        for n in 0..count {
            let key = format!("cached-{n}");
            let put = cache.put_used_at(&key, b"x", expired - n as i64);
            assert!(put.await.expect("cached"));
        }

        test_hooks::ROWS_READ.set(0);
        let read = cache.get_remote_content("blob", "own-0").await;
        assert_eq!(read.expect("read"), Some(b"own".to_vec()));
        assert_eq!(test_hooks::ROWS_READ.get(), 1);
        let mut left = count;
        while left > 0 {
            test_hooks::ROWS_READ.set(0);
            let reclaimed = cache.reclaim_remote_cache_step().await.expect("reclaim");
            assert_eq!(reclaimed, left.min(step));
            assert!(test_hooks::ROWS_READ.get() <= step);
            left -= reclaimed;
        }
        assert_eq!(cache.reclaim_remote_cache_step().await.expect("done"), 0);
        for n in [0, count - 1] {
            let kept = cache.has_remote_content("blob", &format!("own-{n}")).await;
            assert!(kept.expect("kept"));
        }
        drop(cache);
        delete_database(&account).await.expect("delete");
    }
}

/// 転送の途中で取得を待つのをやめると（転送は共有の task で続く）、検証を終えた bytes を呼出元が置くまで何も保存されない。
/// 取り直すと同じ hash で完成し、予約はすべて戻る。
#[wasm_bindgen_test]
async fn a_transfer_abandoned_midway_stores_nothing_and_a_retry_completes() {
    let native = native().await.expect("native peer");
    let account = account("interrupted");
    let session = start(&native, &account, false).await.expect("start");
    session.learn(&native).await.expect("learn");
    let reserved = || {
        let reservation = session.cache.empty_remote_cache_reservation();
        reservation.counter.load(Ordering::Acquire)
    };
    let started = test_hooks::reserved();
    let _ = n0_future::future::now_or_never(started.notified());
    tokio::select! {
        biased;
        _ = started.notified() => {}
        _ = session.blobs.fetch_blob(&native.blob) => panic!("the transfer finished before the interruption"),
    }
    // 予約は最初の受信の後に取るので、ここは転送の途中。
    assert!(reserved() > 0);
    let status = session.blobs.local_blob_status(&native.blob).await;
    assert_eq!(status.expect("status"), BlobStatus::Missing);

    let bytes = session.blobs.fetch_blob(&native.blob).await;
    let bytes = bytes.expect("retry").expect("native blob");
    assert_eq!(bytes, blob_e2e::blob("native"));
    session
        .blobs
        .put_remote_blob(bytes, MIME)
        .await
        .expect("store");
    let status = session.blobs.local_blob_status(&native.blob).await;
    assert_eq!(status.expect("status"), BlobStatus::Available);
    n0_future::time::timeout(Duration::from_secs(10), async {
        while reserved() != 0 {
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the reservations are returned");
    session.stop().await.expect("stop");
    delete_database(&account).await.expect("delete");
}

/// Web の blob-service も取得 gate の前提を保つ（W2 AC-4）。状態確認と手元の読み出しは remote から取らず、取得は
/// 一時 bytes で、呼出元が gate を確かめて置くまで保存しない。置いた remote の内容は回収の対象で、本人の書込みは
/// 保護される。
#[wasm_bindgen_test]
async fn the_blob_service_keeps_the_local_only_ephemeral_and_protection_boundaries() {
    let native = native().await.expect("native peer");
    let account = account("gates");
    let session = start(&native, &account, false).await.expect("start");
    session.learn(&native).await.expect("learn");
    let blobs = &session.blobs;
    let status =
        |hash: BlobHash| async move { blobs.local_blob_status(&hash).await.expect("status") };

    assert_eq!(status(native.blob.clone()).await, BlobStatus::Missing);
    let local = blobs.fetch_local_blob(&native.blob).await;
    assert_eq!(local.expect("local"), None);
    let fetched = blobs.fetch_blob(&native.blob).await;
    let fetched = fetched.expect("fetch").expect("native blob");
    assert_eq!(fetched, blob_e2e::blob("native"));
    let display = blobs.prepare_display_fetch(&native.blob).await;
    let displayed = display.expect("display").await.expect("display fetch");
    assert_eq!(displayed, Some(blob_e2e::blob("native")));
    assert_eq!(status(native.blob.clone()).await, BlobStatus::Missing);
    let memory = session.node.read_local_blob(native.blob.as_str()).await;
    assert_eq!(memory.expect("memory store"), None);

    blobs.put_remote_blob(fetched, MIME).await.expect("store");
    assert_eq!(status(native.blob.clone()).await, BlobStatus::Available);
    let own = blobs.put_blob(blob_e2e::blob("gates"), MIME).await;
    let own = own.expect("own").hash;
    // cache の容量をすべて予約すると、remote の内容は回収され、本人の書込みは残る。
    let mut all = session.cache.empty_remote_cache_reservation();
    let capacity = session.cache.remote_cache_capacity();
    let reserved = session.cache.reserve_remote_cache_bytes(&mut all, capacity);
    assert!(reserved.await.expect("reserve"));
    assert_eq!(status(native.blob.clone()).await, BlobStatus::Missing);
    assert_eq!(status(own).await, BlobStatus::Available);
    drop(all);
    session.stop().await.expect("stop");
    delete_database(&account).await.expect("delete");
}
