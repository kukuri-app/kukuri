//! #1632 AC-5: 公開 blob の保持端末の検索（`POST /v1/blob-providers/search`、ADR 0063 §7）の contract test。
//!
//! loopback の Testnet と補助 index の server で、CN を使わない保持端末（共通 index の一覧の鍵だけを共有する）を
//! CN が見つけ、relay URL を持つ候補だけを返すこと（D5・D6）と、未提供・認証・同意・形式の拒否、端末ごとの満杯、
//! HTTP の切断での受付の回収、期限での打ち切りを確かめる。
//!
//! Postgres + Redis を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iroh::address_lookup::{AddrFilter, AddressLookup, EndpointData};
use iroh::{RelayUrl, SecretKey, TransportAddr};
use iroh_mainline_address_lookup::DhtAddressLookup;
use iroh_mainline_endpoint_discovery::{Announcer, Hash, ServerList, infohash_from_blake3};
use kukuri_cn_operator::SAMPLE_CONFIG;
use kukuri_cn_protocol::{
    BLOB_PROVIDER_SEARCH_PATH, BOOTSTRAP_NODES_PATH, BlobProviderCandidate,
    BlobProviderSearchResponse, BootstrapNodesResponse,
};
use kukuri_cn_user_api::BlobProviderSearch;
use kukuri_core::generate_keys;
use kukuri_transport::PublicBlobIndex;
use n0_mainline::{DhtBuilder, SigningKey, Testnet};
use reqwest::{Client, StatusCode};
use udp_addr_index::{Limits, Server};

mod support;
use support::{
    TestServer, accept_required_consents, authenticate, integration_test_admin_database_url,
};

fn dht_builder(testnet: &Testnet) -> DhtBuilder {
    let mut builder = DhtBuilder::default();
    builder
        .bootstrap(&testnet.bootstrap)
        .port(0)
        .public_ip(Ipv4Addr::LOCALHOST);
    builder
}

/// 住所 record（`addr`）を公開し、`hash` を `index` で告知した保持端末。値を落とすと公開と告知が止まる。
async fn holder(
    testnet: &Testnet,
    index: &PublicBlobIndex,
    addr: TransportAddr,
    hash: &Hash,
) -> Result<(DhtAddressLookup, Announcer)> {
    let secret = SecretKey::generate();
    let dht = dht_builder(testnet).build()?;
    let lookup = DhtAddressLookup::builder()
        .dht(dht.clone())
        .secret_key(secret.clone())
        .addr_filter(AddrFilter::unfiltered())
        .build()?;
    lookup.publish(&EndpointData::new(vec![addr]));
    let announcer = Announcer::new(secret, dht.clone(), index.connect(dht).await);
    tokio::time::timeout(Duration::from_secs(60), async {
        // 補助 index が record を持つまで、告知は失敗する。
        while announcer
            .announce(infohash_from_blake3(hash).into())
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("the holder did not announce in time")?;
    Ok((lookup, announcer))
}

async fn search(
    client: &Client,
    base_url: &str,
    token: Option<&str>,
    hash: &str,
    budget_ms: u64,
) -> Result<reqwest::Response> {
    let request = client
        .post(format!("{base_url}{BLOB_PROVIDER_SEARCH_PATH}"))
        .json(&serde_json::json!({ "hash": hash, "budget_ms": budget_ms }));
    let request = match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };
    Ok(request.send().await?)
}

async fn consented_token(client: &Client, base_url: &str, endpoint_id: &str) -> Result<String> {
    let (token, _) = authenticate(client, base_url, &generate_keys(), endpoint_id, None).await?;
    accept_required_consents(client, base_url, &token).await?;
    Ok(token)
}

/// bootstrap の応答で、この node 自身が検索を提供すると示すか。
async fn offers_search(client: &Client, base_url: &str, token: &str) -> Result<bool> {
    client
        .get(format!("{base_url}{BOOTSTRAP_NODES_PATH}"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json::<BootstrapNodesResponse>()
        .await?
        .nodes
        .into_iter()
        .find(|node| node.base_url == base_url)
        .map(|node| node.resolved_urls.public_blob_search)
        .context("the bootstrap response must list the node itself")
}

async fn code(response: reqwest::Response) -> Result<String> {
    let body = response.json::<serde_json::Value>().await?;
    Ok(body["code"].as_str().unwrap_or_default().to_string())
}

/// 共通 index の一覧の鍵だけを共有する保持端末（CN を使わない）を見つけ、relay URL を持つ候補だけを返す。
#[tokio::test]
async fn a_holder_with_a_relay_url_is_found_through_the_common_index() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping blob provider search test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let testnet = Testnet::new(3).await?;
    let index_dht = dht_builder(&testnet).build()?;
    let index_addr = SocketAddrV4::new(
        Ipv4Addr::LOCALHOST,
        index_dht.info().await?.local_addr().port(),
    );
    let _index_server = Server::new(Limits::for_tests()).attach(index_dht).await?;
    // kukuri の鍵で署名した補助 index の一覧。CN と保持端末は、この鍵だけを設定として共有する。
    let list_key = SigningKey::from_bytes(&[0x16; 32]);
    dht_builder(&testnet)
        .build()?
        .put_mutable(ServerList::new(vec![index_addr])?.sign(&list_key, 1)?, None)
        .await?;
    let index = PublicBlobIndex::ListKey(list_key.verifying_key().to_bytes());

    let hash = Hash::from_bytes([0x32; 32]);
    let relay_url: RelayUrl = "https://relay.kukuri.test/".parse()?;
    let direct_addr = "127.0.0.1:4433".parse()?;
    let (_relay_lookup, relay_holder) = holder(
        &testnet,
        &index,
        TransportAddr::Relay(relay_url.clone()),
        &hash,
    )
    .await?;
    let (_direct_lookup, direct_holder) =
        holder(&testnet, &index, TransportAddr::Ip(direct_addr), &hash).await?;
    // 直接の address だけの住所 record も DHT から引ける。候補にしないのは D5 による。
    let reader = DhtAddressLookup::builder()
        .dht(dht_builder(&testnet).build()?)
        .no_publish()
        .build()?;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(Ok(item)) = reader
                .resolve(direct_holder.id())
                .expect("the DHT lookup resolves")
                .next()
                .await
                && item
                    .endpoint_info()
                    .ip_addrs()
                    .any(|addr| *addr == direct_addr)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("the direct-only record was not published in time")?;

    let blob_provider_search = Arc::new(BlobProviderSearch::start(&dht_builder(&testnet), index)?);
    let server = TestServer::spawn_with_state(
        &admin_database_url,
        "cn_blob_provider_search",
        SAMPLE_CONFIG,
        |state| state.with_blob_provider_search(blob_provider_search),
    )
    .await?;
    let client = Client::new();
    let token = consented_token(&client, &server.base_url, "peer-a").await?;
    assert!(offers_search(&client, &server.base_url, &token).await?);

    let hash = hash.to_hex();
    let found = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let response = search(&client, &server.base_url, Some(&token), &hash, 10_000)
                .await?
                .error_for_status()?
                .json::<BlobProviderSearchResponse>()
                .await?;
            if !response.candidates.is_empty() {
                return anyhow::Ok(response);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .context("the holder was not found in time")??;
    assert_eq!(
        found.candidates,
        [BlobProviderCandidate {
            endpoint_id: relay_holder.id().to_string(),
            relay_urls: vec![relay_url.to_string()],
            direct_addrs: Vec::new(),
        }]
    );
    server.shutdown().await
}

/// 未提供の node は 404。認証・同意・形式の不正は拒否する。端末ごとの同時数を超えた要求は 429（Retry-After）で、
/// HTTP を切断した検索の受付は、照会の時間切れを待たずに戻る。期限で打ち切った応答は `partial`。
#[tokio::test]
async fn requests_are_rejected_bounded_and_released() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping blob provider search test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let client = Client::new();
    let hash = Hash::from_bytes([0x48; 32]);

    let plain = TestServer::spawn(&admin_database_url, "cn_blob_provider_search_off").await?;
    let token = consented_token(&client, &plain.base_url, "peer-a").await?;
    let response = search(
        &client,
        &plain.base_url,
        Some(&token),
        &hash.to_hex(),
        1_000,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(code(response).await?, "BLOB_PROVIDER_SEARCH_NOT_CONFIGURED");
    assert!(!offers_search(&client, &plain.base_url, &token).await?);
    plain.shutdown().await?;

    // 応答しない補助 index。同じ保持端末の照会は 1 件ずつ、それぞれ時間切れ（2 秒）まで待つ。
    let testnet = Testnet::new(2).await?;
    let silent = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let silent_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, silent.local_addr()?.port());
    let holder = dht_builder(&testnet).build()?;
    holder
        .announce_peer(infohash_from_blake3(&hash).into(), None)
        .await?;
    let blob_provider_search = Arc::new(BlobProviderSearch::start(
        &dht_builder(&testnet),
        PublicBlobIndex::Servers(vec![silent_addr]),
    )?);
    let server = TestServer::spawn_with_state(
        &admin_database_url,
        "cn_blob_provider_search_bounds",
        SAMPLE_CONFIG,
        |state| state.with_blob_provider_search(blob_provider_search),
    )
    .await?;
    let base_url = server.base_url.clone();
    let hash = hash.to_hex().to_string();

    let anonymous = search(&client, &base_url, None, &hash, 1_000).await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(code(anonymous).await?, "AUTH_REQUIRED");
    let (unconsented, _) =
        authenticate(&client, &base_url, &generate_keys(), "peer-b", None).await?;
    let response = search(&client, &base_url, Some(&unconsented), &hash, 1_000).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(code(response).await?, "CONSENT_REQUIRED");
    let token = consented_token(&client, &base_url, "peer-a").await?;
    for (hash, budget_ms) in [("zz", 1_000), (hash.as_str(), 0)] {
        let response = search(&client, &base_url, Some(&token), hash, budget_ms).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(code(response).await?, "INVALID_BLOB_PROVIDER_SEARCH");
    }

    // 照会が補助 index へ届くまで待つ（補助 index の client が組み上がった）。
    let probe = tokio::spawn({
        let (client, base_url, token, hash) = (
            client.clone(),
            base_url.clone(),
            token.clone(),
            hash.clone(),
        );
        async move { search(&client, &base_url, Some(&token), &hash, 100).await }
    });
    let mut packet = [0; 2_048];
    tokio::time::timeout(Duration::from_secs(20), silent.recv_from(&mut packet))
        .await
        .context("the search did not query the index")??;
    assert_eq!(probe.await??.status(), StatusCode::OK);

    // 同じ端末の 9 件のうち、8 件は補助 index の照会を待ち、1 件は 429。
    let mut pending = (0..9)
        .map(|_| {
            let (client, base_url, token, hash) = (
                client.clone(),
                base_url.clone(),
                token.clone(),
                hash.clone(),
            );
            tokio::spawn(
                async move { search(&client, &base_url, Some(&token), &hash, 10_000).await },
            )
        })
        .collect::<Vec<_>>();
    let (busy, index, _) = tokio::time::timeout(
        Duration::from_secs(20),
        futures_util::future::select_all(pending.iter_mut()),
    )
    .await
    .context("no request was refused")?;
    let busy = busy??;
    pending.remove(index);
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(pending.iter().all(|task| !task.is_finished()));
    assert_eq!(
        busy.headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("5")
    );
    assert_eq!(code(busy).await?, "BLOB_PROVIDER_SEARCH_BUSY");

    // 8 件を切断すると、照会の時間切れ（2 秒ごとに 1 件）を待たずに 8 件とも受付が戻る。新しい 8 件は期限で打ち切る。
    for task in &pending {
        task.abort();
    }
    let released = Instant::now();
    loop {
        let responses = futures_util::future::join_all(
            (0..8).map(|_| search(&client, &base_url, Some(&token), &hash, 300)),
        )
        .await;
        let mut admitted = Vec::new();
        for response in responses {
            let response = response?;
            if response.status() == StatusCode::OK {
                admitted.push(response.json::<BlobProviderSearchResponse>().await?);
            }
        }
        if admitted.len() == 8 {
            assert!(
                admitted
                    .iter()
                    .all(|response| response.partial && response.candidates.is_empty())
            );
            break;
        }
        assert!(
            released.elapsed() < Duration::from_secs(1),
            "the disconnected searches still hold the device's slots"
        );
    }
    drop(holder);
    server.shutdown().await
}
