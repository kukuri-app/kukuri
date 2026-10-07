use super::*;

#[derive(Clone, Debug)]
struct RelayFallbackLookup {
    relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
}
impl RelayFallbackLookup {
    fn new(relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>) -> Self {
        Self { relay_urls }
    }
}
impl AddressLookup for RelayFallbackLookup {
    fn resolve(
        &self,
        endpoint_id: EndpointId,
    ) -> Option<n0_future::boxed::BoxStream<Result<AddressLookupItem, iroh::address_lookup::Error>>>
    {
        let relay_urls = self
            .relay_urls
            .read()
            .expect("relay fallback lookup poisoned")
            .clone();
        if relay_urls.is_empty() {
            return None;
        }
        let endpoint_info = EndpointInfo::from(endpoint_addr_with_relays(endpoint_id, &relay_urls));
        Some(Box::pin(futures_util::stream::once(async move {
            Ok(AddressLookupItem::new(
                endpoint_info,
                "community-relay-fallback",
                None,
            ))
        })))
    }
}
fn relay_backed_transport_config_for_platform(
    is_windows: bool,
    relay_urls: &[RelayUrl],
) -> Option<QuicTransportConfig> {
    if !is_windows || relay_urls.is_empty() {
        return None;
    }
    Some(
        QuicTransportConfig::builder()
            .enable_segmentation_offload(false)
            .initial_mtu(1200)
            .min_mtu(1200)
            .mtu_discovery_config(None::<MtuDiscoveryConfig>)
            .send_observed_address_reports(false)
            .receive_observed_address_reports(false)
            .build(),
    )
}

fn relay_backed_windows_transport_config(relay_urls: &[RelayUrl]) -> Option<QuicTransportConfig> {
    relay_backed_transport_config_for_platform(cfg!(target_os = "windows"), relay_urls)
}
pub fn build_endpoint_builder(
    builder: EndpointBuilder,
    discovery: &Arc<MemoryLookup>,
    dht_options: Option<&DhtDiscoveryOptions>,
    relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
) -> Result<EndpointBuilder> {
    let mut builder = builder.address_lookup(discovery.clone());
    let relay_urls_snapshot = relay_urls
        .read()
        .expect("relay transport config poisoned")
        .clone();
    if let Some(transport_config) = relay_backed_windows_transport_config(&relay_urls_snapshot) {
        builder = builder.transport_config(transport_config);
    }
    builder = builder.address_lookup(RelayFallbackLookup::new(relay_urls));
    // DHT は UDP を使うので native だけ（ADR 0056 §3）。
    #[cfg(target_family = "wasm")]
    let _ = dht_options;
    #[cfg(not(target_family = "wasm"))]
    if let Some(dht_options) = dht_options.filter(|options| options.enabled) {
        let mut dht_builder = DhtAddressLookup::builder().addr_filter(AddrFilter::unfiltered());
        if let Some(dht) = &dht_options.dht {
            dht_builder = dht_builder.dht(dht.clone());
        } else if let Some(builder_override) = dht_options.resolved_dht_builder() {
            dht_builder = dht_builder.dht_builder(builder_override);
        }
        builder = builder.address_lookup(dht_builder);
    }
    Ok(builder)
}

pub async fn sync_endpoint_relay_config(
    endpoint: &Endpoint,
    current: &[RelayUrl],
    next: &[RelayUrl],
) -> Result<()> {
    let current = current.iter().cloned().collect::<BTreeSet<_>>();
    let next = next.iter().cloned().collect::<BTreeSet<_>>();
    for relay_url in current.difference(&next) {
        endpoint.remove_relay(relay_url).await;
    }
    for relay_url in next.difference(&current) {
        endpoint
            .insert_relay(
                relay_url.clone(),
                Arc::new(RelayConfig::from(relay_url.clone())),
            )
            .await;
    }
    Ok(())
}

impl IrohGossipTransport {
    pub async fn update_relay_config(&self, relay_config: TransportRelayConfig) -> Result<()> {
        let relay_config = relay_config.normalized();
        let relay_urls = relay_config.parsed_relay_urls()?;
        *self.connect_mode.lock().await = relay_config.connect_mode();
        *self
            .relay_urls
            .write()
            .expect("transport relay urls poisoned") = relay_urls;
        *self.last_error.lock().await = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_backed_transport_config_requires_windows_and_relay_urls() {
        let relay_url = "https://relay.example.com"
            .parse::<RelayUrl>()
            .expect("relay url");

        assert!(
            relay_backed_transport_config_for_platform(false, std::slice::from_ref(&relay_url))
                .is_none()
        );
        assert!(relay_backed_transport_config_for_platform(true, &[]).is_none());
        assert!(relay_backed_transport_config_for_platform(true, &[relay_url]).is_some());
    }
}
