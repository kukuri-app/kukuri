//! ブラウザの IndexedDB に置いた blob が、reload の後も同じ hash・完成状態で読め、native と送受信できる
//! （#1215 W2 AC-2、ADR 0058 §5）。relay だけの経路と、QUIC over WebRTC DataChannel の custom path の両方で確かめる。
//! reload は、node と cache を止めてから、同じ account の database を開き直した新しい node で表す。
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_blob_peer` から実行する。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use iroh::endpoint::Connection;
use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use kukuri_blob_service::{BlobService, BlobStatus, IrohBlobService};
use kukuri_core::BlobHash;
use kukuri_iroh_node::{IrohDocsNode, NodeOptions};
use kukuri_store::{ContentCacheStore, REMOTE_CACHE_CAPACITY_BYTES};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use crate::IndexedDbCache;

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
    let blobs = IrohBlobService::with_content_cache(node.clone(), cache);
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

    /// native の blob を取得し、取得 gate を通った内容として cache へ置く。
    async fn receive(&self, native: &Native) -> Result<String> {
        let id = native.addr.id.to_string();
        self.blobs.learn_peer(&id).await?;
        self.blobs.learn_content_source(&native.blob, &id).await?;
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
    let capacity = REMOTE_CACHE_CAPACITY_BYTES as u64;

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
