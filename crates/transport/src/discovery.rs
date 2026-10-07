use std::sync::Arc;
#[cfg(test)]
use std::sync::RwLock as StdRwLock;

use anyhow::Result;
use iroh::Endpoint;
use iroh::address_lookup::MemoryLookup;

use crate::config::{ConnectMode, TransportRelayConfig};

pub async fn prepare_endpoint_for_discovery(
    endpoint: &Endpoint,
    discovery: &Arc<MemoryLookup>,
    relay_config: &TransportRelayConfig,
) -> Result<()> {
    let relay_backed = relay_config.connect_mode() == ConnectMode::DirectOrRelay;
    if relay_backed {
        let endpoint = endpoint.clone();
        let discovery = Arc::clone(discovery);
        n0_future::task::spawn(async move {
            endpoint.online().await;
            discovery.add_endpoint_info(endpoint.addr());
        });
    }
    discovery.add_endpoint_info(endpoint.addr());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures_util::StreamExt;
    use iroh::SecretKey;
    use iroh::address_lookup::{AddrFilter, AddressLookup};
    use iroh_mainline_address_lookup::DhtAddressLookup;
    use n0_future::time::timeout;
    use n0_mainline::{DhtBuilder, Testnet};
    use std::time::Duration;

    use crate::config::DhtDiscoveryOptions;
    use crate::iroh::bind_endpoint_with_options;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn transport_unreachable_relay_does_not_block_endpoint_bind() {
        let relay_config = TransportRelayConfig {
            iroh_relay_urls: vec!["https://127.0.0.1:9".to_string()],
        }
        .normalized();
        let relay_urls = Arc::new(StdRwLock::new(
            relay_config.parsed_relay_urls().expect("relay urls"),
        ));

        let (endpoint, _discovery, _paths) = timeout(
            Duration::from_secs(3),
            bind_endpoint_with_options(
                std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                    std::net::Ipv4Addr::LOCALHOST,
                    0,
                )),
                &DhtDiscoveryOptions::disabled(),
                &relay_config,
                relay_urls,
                None,
            ),
        )
        .await
        .expect("endpoint bind must not wait for relay connectivity")
        .expect("bind endpoint");

        endpoint.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn transport_relay_backed_dht_lookup_publishes_relay_info() {
        let testnet = Testnet::new(5).await.expect("testnet");
        let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
            .await
            .expect("relay server");
        let relay_config = TransportRelayConfig {
            iroh_relay_urls: vec![relay_url.to_string()],
        }
        .normalized();
        let secret_key = SecretKey::from_bytes(&[7u8; 32]);
        let relay_urls = Arc::new(StdRwLock::new(
            relay_config.parsed_relay_urls().expect("relay urls"),
        ));
        let (endpoint, _discovery, _paths) = bind_endpoint_with_options(
            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::LOCALHOST,
                0,
            )),
            &DhtDiscoveryOptions::with_bootstrap(&testnet.bootstrap),
            &relay_config,
            relay_urls,
            Some(secret_key.clone()),
        )
        .await
        .expect("bind endpoint");

        let mut dht_builder = DhtBuilder::default();
        dht_builder.bootstrap(&testnet.bootstrap);
        let lookup = DhtAddressLookup::builder()
            .dht_builder(dht_builder)
            .no_publish()
            .addr_filter(AddrFilter::unfiltered())
            .build()
            .expect("dht lookup");
        timeout(Duration::from_secs(30), async {
            loop {
                if let Some(mut resolved) = lookup.resolve(endpoint.id())
                    && let Some(Ok(item)) = resolved.next().await
                    && item
                        .endpoint_info()
                        .relay_urls()
                        .any(|candidate| candidate == &relay_url)
                {
                    return;
                }
                n0_future::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("endpoint relay info never published to DHT lookup");
    }
}
