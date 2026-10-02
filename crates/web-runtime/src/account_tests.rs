//! 鍵・設定・最小状態・projection・peer の接続候補が IndexedDB の transaction の確定で保存され、reload の後に同じ値で
//! 戻ること（#1217 W4 AC-2、ADR 0059 §1〜§3）。reload は、保存先を閉じてから同じ database を開き直した新しい接続で表す。
//! - 失敗（quota・拒否・破損・schema の版・中断）を区別して返し、別の identity・鍵で隠さない。
//! - account ごとの endpoint 秘密鍵を vault に置き、reload の後も同じ EndpointId で Endpoint を作り、その EndpointId で
//!   作った private channel の鍵更新が保留にならない（W6 #1219 の担当端末）。
//! - cache の回収は、鍵・private channel の行・account 同期の版・端末だけの行を消さない。
//! - projection は store の parity の scenario で MemoryStore と同じ結果になり、sqlite の実測値を満たす。
//!
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-web-runtime web_storage_peer` から実行する。

use std::path::PathBuf;
use std::sync::Arc;

use js_sys::Uint8Array;
use kukuri_app_api::{AppService, PrivateChannelControllerPending, ServiceHandles};
use kukuri_blob_service::IrohBlobService;
use kukuri_core::{
    ChannelAudienceKind, CreatePrivateChannelInput, EnvelopeId, KukuriKeys, PayloadRef, ReplicaId,
    TopicId,
};
use kukuri_desktop_runtime::{ClientStorage, load_endpoint_secret, save_endpoint_secret};
use kukuri_docs_sync::IrohDocsSync;
use kukuri_iroh_node::{IrohDocsNode, NodeOptions};
use kukuri_store::{
    AccountSyncRow, AccountSyncStore, ContentCacheStore, LEARNED_RETENTION_MS, ObjectProjectionRow,
    ObjectProjectionStore, PeerCandidateStore, PrivateChannelEpochRow, PrivateChannelKeyStore,
    PrivateChannelRow,
};
use kukuri_transport::{FakeNetwork, FakeTransport};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;
use web_sys::DomException;

use crate::idb::{self, Mode, StorageFailure, js_error};
use crate::rows::{Txn, key, test_hooks, text};
use crate::{BrowserStorage, IndexedDbCache};

const KEYRING: &str = "org.kukuri.desktop";
const TOPIC: &str = "kukuri:topic:web-account";

/// 試験ごとに別の account の ID（公開鍵の hex の先頭 16 文字と同じ形）。
fn account_id() -> String {
    let now = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{:016x}", now as u64)
}

fn db_path(account: &str) -> PathBuf {
    PathBuf::from(format!("/kukuri/accounts/{account}/kukuri.db"))
}

fn failure(error: &anyhow::Error) -> Option<StorageFailure> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref().copied())
}

#[wasm_bindgen_test]
async fn keys_and_settings_survive_a_reload_in_the_account_vault() {
    let account = account_id();
    let secret = format!("db:{}", db_path(&account).display());
    let setting = format!("/kukuri/accounts/{account}/kukuri.subscriptions.json");
    let keys = KukuriKeys::generate();
    let storage = BrowserStorage::open().await.expect("open");
    storage
        .set(KEYRING, &secret, keys.export_secret_hex().as_bytes())
        .await
        .expect("account key");
    storage
        .set("file", &setting, b"{\"topics\":[]}")
        .await
        .expect("setting");
    storage
        .set("file", "/kukuri/accounts.json", b"{\"accounts\":[]}")
        .await
        .expect("device setting");
    drop(storage);

    let storage = BrowserStorage::open().await.expect("reopen");
    let restored = storage
        .get(KEYRING, &secret)
        .await
        .expect("read")
        .expect("key");
    assert_eq!(
        KukuriKeys::parse(std::str::from_utf8(&restored).unwrap())
            .unwrap()
            .public_key_hex(),
        keys.public_key_hex(),
        "the same account key after a reload"
    );
    assert_eq!(
        storage.get("file", &setting).await.unwrap().as_deref(),
        Some(b"{\"topics\":[]}".as_slice())
    );
    assert_eq!(
        storage
            .get("file", "/kukuri/accounts.json")
            .await
            .unwrap()
            .as_deref(),
        Some(b"{\"accounts\":[]}".as_slice())
    );
    // account の値は device ではなくその account の vault にある。
    let vault = idb::open(&format!("kukuri-vault-v1-{account}"), 1, |_| Ok(()))
        .await
        .expect("vault");
    let tx = Txn::begin(&vault, &["secrets"], Mode::Read).unwrap();
    let raw = idb::done(
        &crate::rows::store(&tx, "secrets")
            .unwrap()
            .get(&key(&[text(KEYRING), text(&secret)]))
            .unwrap(),
    )
    .await
    .unwrap();
    let sealed = Uint8Array::new(&raw).to_vec();
    assert!(
        !String::from_utf8_lossy(&sealed).contains(&keys.export_secret_hex()),
        "the secret is wrapped, not stored in the clear"
    );
    vault.close();

    storage.delete(KEYRING, &secret).await.expect("delete");
    assert!(storage.get(KEYRING, &secret).await.unwrap().is_none());
}

#[wasm_bindgen_test]
async fn storage_failures_are_reported_by_kind_without_a_new_identity() {
    let account = account_id();
    let path = db_path(&account);
    let storage = BrowserStorage::open().await.expect("open");
    save_endpoint_secret(&storage, &path, &[7; 32])
        .await
        .expect("endpoint secret");

    // 中断（部分的な保存）: 書けなかった値は残らず、前の値のまま。
    let setting = format!("/kukuri/accounts/{account}/kukuri.subscriptions.json");
    storage
        .set("file", &setting, b"before")
        .await
        .expect("setting");
    test_hooks::fail_writes(1);
    let error = storage
        .set("file", &setting, b"after")
        .await
        .expect_err("aborted write");
    assert_eq!(failure(&error), Some(StorageFailure::Interrupted));
    assert_eq!(
        storage.get("file", &setting).await.unwrap().as_deref(),
        Some(b"before".as_slice())
    );

    // 破損: 包みを壊した値は読めない失敗になり、無い（新しく作る）とは扱わない。
    let vault = idb::open(&format!("kukuri-vault-v1-{account}"), 1, |_| Ok(()))
        .await
        .expect("vault");
    let tx = Txn::begin(&vault, &["secrets"], Mode::Write).unwrap();
    let secrets = crate::rows::store(&tx, "secrets").unwrap();
    let request = secrets.open_cursor().map_err(js_error).expect("cursor");
    while let Some(cursor) = idb::next(&request).await.unwrap() {
        cursor
            .update(&Uint8Array::from([0u8; 40].as_slice()))
            .unwrap();
        cursor.continue_().unwrap();
    }
    tx.commit().await.unwrap();
    vault.close();
    let error = load_endpoint_secret(&storage, &path)
        .await
        .expect_err("corrupt secret");
    assert_eq!(failure(&error), Some(StorageFailure::Corrupt));

    // schema の版: この版より新しい database は開かずに失敗する。
    let newer = account_id();
    idb::open(&format!("kukuri-vault-v1-{newer}"), 2, |_| Ok(()))
        .await
        .expect("newer vault")
        .close();
    let error = load_endpoint_secret(&storage, &db_path(&newer))
        .await
        .expect_err("newer schema");
    assert_eq!(failure(&error), Some(StorageFailure::Upgrade));

    // quota・拒否は DOMException の名前で区別する。
    for (name, expected) in [
        ("QuotaExceededError", StorageFailure::Quota),
        ("SecurityError", StorageFailure::Denied),
    ] {
        let exception = DomException::new_with_message_and_name("test", name).unwrap();
        assert_eq!(failure(&js_error(JsValue::from(exception))), Some(expected));
    }
}

/// 同じ account の 1 回の起動（page の読込み）の service。
async fn app(
    cache: &Arc<IndexedDbCache>,
    node: &Arc<IrohDocsNode>,
    keys: &KukuriKeys,
) -> anyhow::Result<AppService> {
    node.install_remote_cache(cache.clone())?;
    let transport = Arc::new(FakeTransport::new(
        node.endpoint().id().to_string(),
        FakeNetwork::default(),
    ));
    let docs = Arc::new(IrohDocsSync::with_content_cache(
        node.clone(),
        cache.clone(),
    ));
    docs.use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
        .await?;
    let blobs = Arc::new(IrohBlobService::with_content_cache(
        node.clone(),
        cache.clone(),
    ));
    Ok(AppService::from_handles(ServiceHandles::new(
        cache.clone(),
        cache.clone(),
        transport.clone(),
        transport,
        docs,
        blobs,
        keys.clone(),
    )))
}

/// ブラウザの node（IP の transport が無いので、試験の native の相手の relay を使う）。
async fn node(secret: Option<[u8; 32]>) -> Arc<IrohDocsNode> {
    let relay = crate::browser_tests::native()
        .await
        .expect("native peer")
        .relay;
    IrohDocsNode::memory_with(NodeOptions {
        secret_key: secret.map(|secret| iroh::SecretKey::from_bytes(&secret)),
        relay_config: kukuri_transport::TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        ..NodeOptions::default()
    })
    .await
    .expect("node")
}

#[wasm_bindgen_test]
async fn the_endpoint_id_survives_a_reload_and_its_channel_rotates() {
    let keys = KukuriKeys::generate();
    let account = keys.public_key_hex();
    let path = db_path(&account[..16]);
    let storage = BrowserStorage::open().await.expect("open");
    assert_eq!(load_endpoint_secret(&storage, &path).await.unwrap(), None);
    let first = node(None).await;
    save_endpoint_secret(&storage, &path, &first.endpoint().secret_key().to_bytes())
        .await
        .expect("save endpoint secret");
    let endpoint_id = first.endpoint().id();
    let cache = Arc::new(IndexedDbCache::open(&account).await.expect("cache"));
    let service = app(&cache, &first, &keys).await.expect("first app");
    let _ = service.list_timeline(TOPIC, None, 20).await;
    let channel = service
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(TOPIC),
            label: "reload".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    drop(service);
    first.shutdown().await.expect("shutdown");
    drop((cache, storage));

    // reload: 保存した秘密鍵で同じ EndpointId の Endpoint を作る。
    let storage = BrowserStorage::open().await.expect("reopen");
    let secret = load_endpoint_secret(&storage, &path)
        .await
        .expect("read endpoint secret")
        .expect("saved endpoint secret");
    let cache = Arc::new(IndexedDbCache::open(&account).await.expect("cache"));

    // 対照: 保存していない鍵の Endpoint（reload で鍵を失った場合）では、担当を失って保留になる。
    let other = node(None).await;
    assert_ne!(other.endpoint().id(), endpoint_id);
    let service = app(&cache, &other, &keys).await.expect("other app");
    service
        .restore_joined_private_channels()
        .await
        .expect("restore");
    let error = service
        .rotate_private_channel(TOPIC, &channel.channel_id)
        .await
        .expect_err("another endpoint waits");
    assert!(
        error
            .downcast_ref::<PrivateChannelControllerPending>()
            .is_some()
    );
    drop(service);
    other.shutdown().await.expect("shutdown");

    let second = node(Some(secret)).await;
    assert_eq!(
        second.endpoint().id(),
        endpoint_id,
        "the same EndpointId after a reload"
    );
    let service = app(&cache, &second, &keys).await.expect("second app");
    service
        .restore_joined_private_channels()
        .await
        .expect("restore");
    let rotated = service
        .rotate_private_channel(TOPIC, &channel.channel_id)
        .await
        .expect("the controlling device rotates after a reload");
    assert_ne!(rotated.current_epoch_id, channel.current_epoch_id);
    drop(service);
    second.shutdown().await.expect("shutdown");
}

#[wasm_bindgen_test]
async fn account_rows_match_memory_in_every_parity_scenario() {
    kukuri_store::parity::check_backend(async || {
        IndexedDbCache::open(&account_id()).await.expect("cache")
    })
    .await;
}

fn projection(id: &str, created_at: i64) -> ObjectProjectionRow {
    ObjectProjectionRow {
        object_id: EnvelopeId::from(id),
        topic_id: TOPIC.into(),
        channel_id: "public".into(),
        author_pubkey: "a".repeat(64),
        created_at,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::InlineText {
            text: "x".repeat(400),
        },
        content: Some("x".repeat(400)),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new(format!("topic::{TOPIC}")),
        source_key: format!("objects/{id}"),
        source_envelope_id: EnvelopeId::from(id),
        source_blob_hash: None,
        source_docs_author: None,
        derived_at: created_at,
        projection_version: kukuri_store::VERIFIED_OBJECT_PROJECTION_VERSION,
    }
}

fn channel_row(joined: bool) -> PrivateChannelRow {
    PrivateChannelRow {
        channel_key: format!("{TOPIC}::channel-1"),
        topic_id: TOPIC.into(),
        channel_id: "channel-1".into(),
        label: "kept".into(),
        creator_pubkey: "a".repeat(64),
        owner_pubkey: "a".repeat(64),
        joined_via_pubkey: None,
        audience_kind: "invite_only".into(),
        current_epoch_id: "epoch-1".into(),
        controller: None,
        joined,
        updated_at: 1,
        op_id: "op-1".into(),
    }
}

fn epoch_row(epoch: &str) -> PrivateChannelEpochRow {
    PrivateChannelEpochRow {
        channel_id: "channel-1".into(),
        epoch_id: epoch.into(),
        started_at: 1,
        receive_key_id: format!("receive-{epoch}"),
        updated_at: 1,
        sealed_secret: vec![1, 2, 3],
        rotation_from: None,
        rotation_after: None,
    }
}

/// 参加の行の無い channel の鍵の行だけを足せる。同じ行は足さない（native の store と同じ意味。#1218 AC-4c）。
#[wasm_bindgen_test]
async fn a_key_row_without_a_channel_is_added_once() {
    let cache = IndexedDbCache::start(&account_id(), None)
        .await
        .expect("cache");
    assert!(
        cache
            .put_private_channel_epoch(&epoch_row("epoch-9"))
            .await
            .unwrap()
    );
    let mut again = epoch_row("epoch-9");
    again.sealed_secret = vec![1];
    assert!(!cache.put_private_channel_epoch(&again).await.unwrap());
    assert_eq!(
        cache
            .get_private_channel_epoch("channel-1", "epoch-9")
            .await
            .unwrap(),
        Some(epoch_row("epoch-9"))
    );
    assert_eq!(
        cache.get_private_channel_by_id("channel-1").await.unwrap(),
        None
    );
}

#[wasm_bindgen_test]
async fn a_cache_reclaim_keeps_keys_versions_and_device_rows() {
    let account = account_id();
    // remote の投稿の行 1 件と blob 1 件が収まらない容量。
    let cache = IndexedDbCache::start(&account, Some(4 * 1024))
        .await
        .expect("cache");
    cache
        .put_private_channel(&channel_row(true), &[epoch_row("epoch-1")])
        .await
        .expect("private channel");
    let adopted = AccountSyncRow {
        key: "profile".into(),
        op_id: "op".into(),
        updated_at: 5,
        value: Some("{}".into()),
    };
    assert!(cache.adopt_account_sync_row(&adopted).await.unwrap());
    cache
        .put_object_projection(projection("own-post", 1))
        .await
        .expect("own post");
    cache
        .put_remote_object_projection(projection("remote-post", 2))
        .await
        .expect("remote post");
    assert!(
        cache
            .get_object_projection(&EnvelopeId::from("remote-post"))
            .await
            .unwrap()
            .is_some()
    );
    // 新しい remote の内容を置くと、古い非保護の行（remote の投稿）から回収する。
    assert!(
        cache
            .put_remote_content("blob", "big", "blob", &[0; 3 * 1024])
            .await
            .unwrap()
    );
    assert!(
        cache
            .get_object_projection(&EnvelopeId::from("remote-post"))
            .await
            .unwrap()
            .is_none(),
        "the remote post is reclaimed"
    );
    cache.reclaim_remote_cache_step().await.expect("reclaim");
    assert!(
        cache
            .get_object_projection(&EnvelopeId::from("own-post"))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        cache
            .get_private_channel(&channel_row(true).channel_key)
            .await
            .unwrap(),
        Some(channel_row(true))
    );
    assert_eq!(
        cache
            .get_private_channel_epoch("channel-1", "epoch-1")
            .await
            .unwrap(),
        Some(epoch_row("epoch-1"))
    );
    assert_eq!(
        cache.get_account_sync_row("profile").await.unwrap(),
        Some(adopted)
    );
    drop(cache);

    // 退会の tombstone も reload の後に残る。
    let cache = IndexedDbCache::open(&account).await.expect("reopen");
    cache
        .put_private_channel(&channel_row(false), &[])
        .await
        .unwrap();
    drop(cache);
    let cache = IndexedDbCache::open(&account).await.expect("reopen");
    assert_eq!(
        cache
            .get_private_channel(&channel_row(false).channel_key)
            .await
            .unwrap(),
        Some(channel_row(false))
    );
}

#[wasm_bindgen_test]
async fn an_aborted_private_channel_write_leaves_nothing() {
    let cache = IndexedDbCache::open(&account_id()).await.expect("cache");
    test_hooks::fail_writes(1);
    let error = cache
        .put_private_channel(
            &channel_row(true),
            &[epoch_row("epoch-1"), epoch_row("epoch-2")],
        )
        .await
        .expect_err("aborted");
    assert_eq!(failure(&error), Some(StorageFailure::Interrupted));
    assert!(
        cache
            .get_private_channel(&channel_row(true).channel_key)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        cache
            .get_private_channel_epoch("channel-1", "epoch-1")
            .await
            .unwrap()
            .is_none()
    );
}

#[wasm_bindgen_test]
async fn peer_candidates_survive_a_reload_within_the_native_limits() {
    let account = account_id();
    let cache = IndexedDbCache::open(&account).await.expect("cache");
    let day = 24 * 60 * 60 * 1000;
    let now = LEARNED_RETENTION_MS + 10 * day;
    cache
        .put_peer_candidate("docs", "learned", "expired", b"old", 1)
        .await
        .unwrap();
    for (index, id) in ["peer-a", "peer-b", "peer-c"].into_iter().enumerate() {
        cache
            .put_peer_candidate("docs", "learned", id, id.as_bytes(), now + index as i64)
            .await
            .unwrap();
    }
    cache
        .put_peer_candidate("docs", "imported", "ticket", b"ticket", 1)
        .await
        .unwrap();
    cache
        .replace_seed_candidates("docs", vec![("seed".into(), b"seed".to_vec())], 1)
        .await
        .unwrap();
    drop(cache);

    let cache = IndexedDbCache::open(&account).await.expect("reopen");
    let window = cache
        .peer_candidate_window("docs", "learned", None, 64, now + 5)
        .await
        .unwrap();
    assert_eq!(
        window
            .iter()
            .map(|(id, _, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["peer-a", "peer-b", "peer-c"],
        "learned candidates survive; the expired one is gone"
    );
    assert_eq!(
        cache
            .peer_candidate_by_id("docs", "learned", "peer-b", now)
            .await
            .unwrap(),
        Some(b"peer-b".to_vec())
    );
    // 折り返し: 末尾の後は先頭から。
    let wrapped = cache
        .peer_candidate_window(
            "docs",
            "learned",
            Some((now + 1, "peer-b".into())),
            2,
            now + 5,
        )
        .await
        .unwrap();
    assert_eq!(
        wrapped
            .iter()
            .map(|(id, _, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["peer-c", "peer-a"]
    );
    assert_eq!(
        cache
            .imported_peer_candidate_window("docs", None, 4)
            .await
            .unwrap(),
        vec![("ticket".to_string(), b"ticket".to_vec())]
    );
    assert_eq!(
        cache
            .peer_candidate_window("docs", "seed", None, 4, now)
            .await
            .unwrap()
            .len(),
        1
    );
    // 学習した候補の容量の上限を超えたら、古いものから消す。
    for index in 0..20 {
        cache
            .put_peer_candidate_bounded(
                ("docs", "learned", &format!("bulk-{index:02}")),
                b"address",
                now + 100 + index,
                1_000,
            )
            .await
            .unwrap();
    }
    assert!(
        cache
            .peer_candidate_by_id("docs", "learned", "peer-a", now)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        cache
            .peer_candidate_by_id("docs", "learned", "bulk-19", now)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        cache
            .imported_peer_candidate_window("docs", None, 4)
            .await
            .unwrap()
            .len(),
        1,
        "explicit tickets are never evicted"
    );
}
