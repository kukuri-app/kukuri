//! ブラウザの IndexedDB に置いた blob が、reload の後も同じ hash・完成状態で読め、native と送受信できる
//! （#1215 W2 AC-2、ADR 0058 §5）。relay だけの経路と、QUIC over WebRTC DataChannel の custom path の両方で確かめる。
//! reload は、node と cache を止めてから、同じ account の database を開き直した新しい node で表す。
//! 中断・書込みの失敗・破損で未完了を完成と扱わず、保護した内容を残したまま非保護分を固定の窓で回収する（W2 AC-3）。
//! Web の blob-service も取得 gate の前提（local-only・ephemeral・保護と cache の境界）を保つ（W2 AC-4）。
//! docs の自分の record は IndexedDB に保護され、browser↔native の有界な key の一覧・record の読み出しが namespace の
//! 同期を始めずに成り立ち、読んだ record は手元に保持される（#1216 W3 AC-2、ADR 0058 §7）。
//! reload の後は空の docs から、保存した自分の record を必要な key だけ有界に読んで手元と相手へ出し、閉じた replica は
//! memory store に残らない（W3 AC-3）。保存の失敗・破損・失効した capability は、偽の成功・公開の replica への読替え
//! にならない（W3 AC-4）。
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer` から実行する。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context as _, Result};
use iroh::endpoint::Connection;
use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use kukuri_blob_service::{BlobService, BlobStatus, IrohBlobService};
use kukuri_core::{BlobHash, KukuriKeys, ReplicaId};
use kukuri_docs_sync::{DocFetchPolicy, DocsSync, IrohDocsSync};
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

#[path = "../tests/support/storage_e2e.rs"]
mod storage_e2e;

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
    docs: IrohDocsSync,
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
    let docs = IrohDocsSync::with_content_cache(node.clone(), cache.clone());
    node.endpoint().online().await;
    let demand = match &transport {
        Some(_) => {
            // 需要の接続を受けて、ブラウザの node が交渉を始める（#1422 AC-2）。
            let demand = storage_e2e::demand(&node, native.addr.clone()).await?;
            storage_e2e::on_custom(&demand).await?;
            Some(demand)
        }
        None => None,
    };
    Ok(Session {
        node,
        cache,
        blobs,
        docs,
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
        let line = storage_e2e::describe(&bytes, self.received() - before);
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

    /// native がこの端の `replica` を読んだ結果。
    async fn read_back(&self, replica: &ReplicaId, webrtc: bool) -> Result<String> {
        let body = format!(
            "{}\n{}\n{}",
            self.node.endpoint().id(),
            replica.as_str(),
            u8::from(webrtc)
        );
        signaling_fixture::post("/docs-read", &body).await
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
        .put_blob(storage_e2e::blob(side), MIME)
        .await
        .expect("own blob")
        .hash;
    assert_eq!(
        first.receive(&native).await.expect("receive"),
        storage_e2e::describe(&storage_e2e::blob("native"), custom)
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
            Some(storage_e2e::blob(side))
        );
    }
    // native は reload 後の browser から `/kukuri/remote-blob/1` で取得する。
    assert_eq!(
        second.send(&own, webrtc).await.expect("send"),
        storage_e2e::describe(&storage_e2e::blob(side), custom)
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
    let blob = storage_e2e::blob("chunks");
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
    let blob = storage_e2e::blob("corrupt");
    let key = hash(&blob);
    cache
        .put_owned_blob("own_blob:corrupt", &key, &blob)
        .await
        .expect("own");
    cache.drop_chunk("blob", &key, 1).await.expect("corrupt");
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
    assert_eq!(bytes, storage_e2e::blob("native"));
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
    assert_eq!(fetched, storage_e2e::blob("native"));
    let display = blobs.prepare_display_fetch(&native.blob).await;
    let displayed = display.expect("display").await.expect("display fetch");
    assert_eq!(displayed, Some(storage_e2e::blob("native")));
    assert_eq!(status(native.blob.clone()).await, BlobStatus::Missing);
    let memory = session.node.read_local_blob(native.blob.as_str()).await;
    assert_eq!(memory.expect("memory store"), None);

    blobs.put_remote_blob(fetched, MIME).await.expect("store");
    assert_eq!(status(native.blob.clone()).await, BlobStatus::Available);
    let own = blobs.put_blob(storage_e2e::blob("gates"), MIME).await;
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

/// `side` の replica の最新の 2 件を読んだときの 1 行（`storage_e2e::read_newest` の形）。
fn newest(side: &str, newest: usize, author: &str) -> String {
    let prefix = storage_e2e::PREFIX;
    let value = format!("{side}-{newest}");
    let bytes = value.len() + storage_e2e::PADDING;
    format!(
        "{prefix}{newest:04},{prefix}{:04} reached_limit=true value={value} bytes={bytes} author={author} namespace=false",
        newest - 1
    )
}

async fn docs_roundtrip(webrtc: bool) {
    let native = native().await.expect("native peer");
    let account = account(if webrtc { "docs-webrtc" } else { "docs-relay" });
    let session = start(&native, &account, webrtc).await.expect("start");
    let keys = KukuriKeys::generate();
    let author = session
        .docs
        .use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
        .await
        .expect("docs author");
    let replica = storage_e2e::replica("browser");
    let prefix = storage_e2e::PREFIX;

    // 自分の record は IndexedDB に保護され、native が有界な reader で読める。
    storage_e2e::write_records(&session.docs, &replica, "browser", 1..=3)
        .await
        .expect("write");
    let key = format!("{prefix}0003");
    let own = session
        .cache
        .get_remote_records(replica.as_str(), &key, Some(&author), 1, false)
        .await;
    assert_eq!(own.expect("own record").len(), 1);
    let via = format!(" via_custom={webrtc}");
    let read = session
        .read_back(&replica, webrtc)
        .await
        .expect("read back");
    assert_eq!(read, newest("browser", 3, &author) + &via);

    // 履歴を増やしても、同じ要求の読み出しは同じ上限（2 件と 1 件の record）に収まり、手元の一覧も読む行は上限 + 1 まで。
    storage_e2e::write_records(&session.docs, &replica, "browser", 4..=30)
        .await
        .expect("write more");
    let read = session
        .read_back(&replica, webrtc)
        .await
        .expect("read back");
    assert_eq!(read, newest("browser", 30, &author) + &via);
    test_hooks::ROWS_READ.set(0);
    let held = session
        .cache
        .remote_record_keys(replica.as_str(), prefix, true, None, 2, false)
        .await;
    let (held, more) = held.expect("held keys");
    assert_eq!(held.len(), 2);
    assert!(more);
    assert!(test_hooks::ROWS_READ.get() <= 3);

    // native の replica を読むと、確かめた record を IndexedDB に保持し、namespace を作らずに手元で読める。
    let before = session.received();
    let native_replica = storage_e2e::replica("native");
    let read = storage_e2e::read_newest(&session.docs, native.addr.clone(), &native_replica)
        .await
        .expect("read native");
    let native_author = read
        .split(" author=")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .expect("native author")
        .to_owned();
    assert_eq!(read, newest("native", 3, &native_author));
    let received = session.received() - before;
    assert_eq!(received >= storage_e2e::PADDING as u64, webrtc);
    let local = session
        .docs
        .query_local_source(&native_replica, &key, Some(&native_author), 1)
        .await
        .expect("held record");
    assert_eq!(local.len(), 1);
    assert!(String::from_utf8_lossy(&local[0].value).starts_with("native-3."));
    let namespace = session.docs.has_local_replica(&native_replica).await;
    assert!(!namespace.expect("namespace"));
    let offline = session
        .docs
        .query_replica_by_author(
            &native_replica,
            &native_author,
            &key,
            DocFetchPolicy::LocalOnly,
        )
        .await;
    assert!(offline.expect("local only").is_none());

    // 自分の record は保護されているので、容量のすべての予約でも回収されない。
    let mut all = session.cache.empty_remote_cache_reservation();
    let capacity = session.cache.remote_cache_capacity();
    let reserved = session.cache.reserve_remote_cache_bytes(&mut all, capacity);
    assert!(reserved.await.expect("reserve"));
    let own = session
        .cache
        .get_remote_records(replica.as_str(), &key, Some(&author), 1, false)
        .await;
    assert_eq!(own.expect("own record").len(), 1);
    drop(all);
    session.stop().await.expect("stop");
    delete_database(&account).await.expect("delete");
}

#[wasm_bindgen_test]
async fn own_records_are_protected_and_read_both_ways_with_native_over_the_relay() {
    docs_roundtrip(false).await;
}

#[wasm_bindgen_test]
async fn own_records_are_protected_and_read_both_ways_with_native_over_the_webrtc_path() {
    docs_roundtrip(true).await;
}

/// reload の後、空の docs から自分の record を必要な key だけ読む（W3 AC-3）。同じ account 由来の docs author で、
/// 手元の exact・docs author 指定・key の一覧が相手（peer）なしで読め、native へも同じ record（同じ hash）を出す。
/// 読む行は要求の上限に収まり、namespace を作らない。同じ更新を書き直しても hash（更新の ID・時刻）は変わらない。
#[wasm_bindgen_test]
async fn own_records_are_restored_from_the_cache_after_a_reload_without_peers() {
    let native = native().await.expect("native peer");
    let account = account("restore");
    let keys = KukuriKeys::generate();
    let seed = keys.derive_docs_author_seed();
    let replica = ReplicaId::new(format!("author::{}", keys.public_key_hex()));
    let newest_key = storage_e2e::key(30);
    let local_only = DocFetchPolicy::LocalOnly;

    let first = start(&native, &account, false).await.expect("start");
    let author = first
        .docs
        .use_account_docs_author(&seed, &keys.public_key_hex())
        .await
        .expect("docs author");
    storage_e2e::write_records(&first.docs, &replica, "own", 1..=30)
        .await
        .expect("write");
    let written = first
        .docs
        .query_replica_by_author(&replica, &author, &newest_key, local_only)
        .await
        .expect("read")
        .expect("written record");
    first.stop().await.expect("stop");

    // reload: memory の docs は空から始まる。
    let second = start(&native, &account, false).await.expect("restart");
    let again = second
        .docs
        .use_account_docs_author(&seed, &keys.public_key_hex())
        .await
        .expect("docs author");
    assert_eq!(again, author);
    test_hooks::ROWS_READ.set(0);
    let by_author = second
        .docs
        .query_replica_by_author(&replica, &author, &newest_key, local_only)
        .await
        .expect("by author")
        .expect("restored record");
    assert_eq!(by_author.content_hash, written.content_hash);
    assert_eq!(by_author.value, written.value);
    assert_eq!(test_hooks::ROWS_READ.get(), 1);
    let exact = second
        .docs
        .query_replica_exact_bounded(&replica, &newest_key, 8, local_only)
        .await
        .expect("exact");
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].content_hash, written.content_hash);
    test_hooks::ROWS_READ.set(0);
    let query = kukuri_docs_sync::DocKeyQuery {
        prefix: storage_e2e::PREFIX.into(),
        order: kukuri_docs_sync::DocKeyOrder::Descending,
        limit: 2,
    };
    let page = second
        .docs
        .query_replica_keys(&replica, query)
        .await
        .expect("keys");
    let page_keys: Vec<_> = page.entries.iter().map(|entry| entry.key.clone()).collect();
    assert_eq!(page_keys, [storage_e2e::key(30), storage_e2e::key(29)]);
    assert!(page.reached_limit);
    assert!(test_hooks::ROWS_READ.get() <= 3);
    // 保存していない key は未取得（空）で、偽の完成を返さない。
    let missing = second
        .docs
        .query_replica_by_author(&replica, &author, &storage_e2e::key(31), local_only)
        .await
        .expect("missing");
    assert!(missing.is_none());
    let namespace = second.docs.has_local_replica(&replica).await;
    assert!(!namespace.expect("namespace"));

    // native は reload 後の browser から、保持分として同じ record を読む。
    let read = second.read_back(&replica, false).await.expect("read back");
    assert_eq!(read, newest("own", 30, &author) + " via_custom=false");

    // 同じ更新を書き直しても、hash（値の中の更新の ID・時刻）は変わらない。
    storage_e2e::write_records(&second.docs, &replica, "own", 30..=30)
        .await
        .expect("re-send");
    let resent = second
        .docs
        .query_replica_by_author(&replica, &author, &newest_key, local_only)
        .await
        .expect("read")
        .expect("re-sent record");
    assert_eq!(resent.content_hash, written.content_hash);
    second.stop().await.expect("stop");
    delete_database(&account).await.expect("delete");
}

/// 閉じた replica は drop され、その内容は memory store の GC で消える。自分の record は保存 trait から読める（W3 AC-3）。
#[wasm_bindgen_test]
async fn a_closed_replica_leaves_the_memory_store_and_stays_readable() {
    let native = native().await.expect("native peer");
    let account = account("memory");
    let session = start(&native, &account, false).await.expect("start");
    let keys = KukuriKeys::generate();
    let author = session
        .docs
        .use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
        .await
        .expect("docs author");
    let replica = storage_e2e::replica("memory");
    let key = storage_e2e::key(1);
    storage_e2e::write_records(&session.docs, &replica, "memory", 1..=1)
        .await
        .expect("write");
    let local_only = DocFetchPolicy::LocalOnly;
    let read = session
        .docs
        .query_replica_by_author(&replica, &author, &key, local_only)
        .await
        .expect("read")
        .expect("record");
    let in_memory = session.node.read_local_blob(&read.content_hash).await;
    assert!(in_memory.expect("memory store").is_some());

    session.docs.close_replica(&replica).await.expect("close");
    n0_future::time::timeout(Duration::from_secs(30), async {
        while session
            .node
            .read_local_blob(&read.content_hash)
            .await
            .expect("memory store")
            .is_some()
        {
            n0_future::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the content leaves the memory store");
    let namespace = session.docs.has_local_replica(&replica).await;
    assert!(!namespace.expect("namespace"));
    let held = session
        .docs
        .query_replica_by_author(&replica, &author, &key, local_only)
        .await
        .expect("held")
        .expect("held record");
    assert_eq!(held.content_hash, read.content_hash);
    session.stop().await.expect("stop");
    delete_database(&account).await.expect("delete");
}

/// 保存の失敗・破損・失効した capability を明示する（W3 AC-4）。保存 trait へ書けなければ書込みは失敗で、record は
/// 保護されない。chunk の欠けた record は未取得として扱う。capability を外した private の replica は error で、公開の
/// replica として読み替えない。
#[wasm_bindgen_test]
async fn own_record_failures_are_explicit_without_a_public_fallback() {
    let native = native().await.expect("native peer");
    let account = account("failures");
    let session = start(&native, &account, false).await.expect("start");
    let keys = KukuriKeys::generate();
    let author = session
        .docs
        .use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
        .await
        .expect("docs author");
    let topic = storage_e2e::replica("failures");
    let local_only = DocFetchPolicy::LocalOnly;

    // 保存 trait への書込みが 2 回とも中断すると（quota 超過と同じ）、書込み全体が失敗する。
    test_hooks::fail_writes(2);
    let failed = storage_e2e::write_records(&session.docs, &topic, "failed", 1..=1).await;
    assert!(failed.is_err());
    let protected = session
        .cache
        .get_remote_records(topic.as_str(), &storage_e2e::key(1), Some(&author), 1, true)
        .await;
    assert!(protected.expect("own records").is_empty());

    // chunk の欠けた record は、namespace を閉じた後の読み出しで未取得になる。
    storage_e2e::write_records(&session.docs, &topic, "corrupt", 2..=2)
        .await
        .expect("write");
    session.docs.close_replica(&topic).await.expect("close");
    let cache_key = format!("{}\0{}\0{author}", topic.as_str(), storage_e2e::key(2));
    let corrupt = session.cache.drop_chunk("record", &cache_key, 0).await;
    corrupt.expect("corrupt");
    let read = session
        .docs
        .query_replica_by_author(&topic, &author, &storage_e2e::key(2), local_only)
        .await;
    assert!(read.expect("read").is_none());

    // capability を外した private の replica は、手元の読み出しが error になる。
    let channel = ReplicaId::new("channel::web-failures");
    let secret = hex::encode([7u8; 32]);
    let docs = &session.docs;
    docs.register_private_replica_secret(&channel, &secret)
        .await
        .expect("register");
    storage_e2e::write_records(docs, &channel, "private", 1..=1)
        .await
        .expect("write");
    docs.remove_private_replica_secret(&channel)
        .await
        .expect("revoke");
    let key = storage_e2e::key(1);
    let by_author = docs.query_replica_by_author(&channel, &author, &key, local_only);
    let error = by_author.await.expect_err("revoked capability").to_string();
    assert!(error.contains("capability is not registered"), "{error}");
    let exact = docs.query_replica_exact_bounded(&channel, &key, 8, local_only);
    assert!(exact.await.is_err());
    session.stop().await.expect("stop");
    delete_database(&account).await.expect("delete");
}
