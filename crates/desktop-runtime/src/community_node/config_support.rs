use super::*;

pub(crate) fn load_community_node_config_from_file(
    db_path: &Path,
) -> Result<Option<CommunityNodeConfig>> {
    let path = community_node_config_path(db_path);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read community-node config `{}`", path.display()))?;
    let config = serde_json::from_str::<CommunityNodeConfig>(&raw)
        .with_context(|| format!("failed to parse community-node config `{}`", path.display()))?;
    Ok(Some(normalize_community_node_config(config)?))
}

pub(crate) fn save_community_node_config(
    db_path: &Path,
    config: &CommunityNodeConfig,
) -> Result<()> {
    let path = community_node_config_path(db_path);
    let normalized = normalize_community_node_config(config.clone())?;
    let json = serde_json::to_vec_pretty(&normalized).with_context(|| {
        format!(
            "failed to encode community-node config `{}`",
            path.display()
        )
    })?;
    fs::write(&path, json)
        .with_context(|| format!("failed to write community-node config `{}`", path.display()))
}

pub(crate) fn normalize_community_node_config(
    config: CommunityNodeConfig,
) -> Result<CommunityNodeConfig> {
    let mut deduped = std::collections::BTreeMap::<String, CommunityNodeNodeConfig>::new();
    for node in config.nodes {
        let base_url = normalize_http_url(node.base_url.as_str())?;
        let incoming_resolved_urls = match node.resolved_urls {
            Some(resolved) => Some(CommunityNodeResolvedUrls::new(
                resolved.public_base_url,
                resolved.connectivity_urls,
                resolved.seed_peers,
            )?),
            None => None,
        };
        let existing = deduped.get(&base_url);
        // 同じ node の重複指定では、どれか 1 つでも採用 OFF なら OFF を維持する(外部送信を増やさない側)。
        let content_advisory_enabled = node.content_advisory_enabled
            && existing.is_none_or(|existing| existing.content_advisory_enabled);
        let resolved_urls = if let Some(existing) = existing {
            merge_community_node_resolved_urls(
                existing.resolved_urls.clone(),
                incoming_resolved_urls,
            )?
        } else {
            incoming_resolved_urls
        };
        deduped.insert(
            base_url.clone(),
            CommunityNodeNodeConfig {
                content_advisory_enabled,
                base_url,
                resolved_urls,
            },
        );
    }
    Ok(CommunityNodeConfig {
        trust_node_priority: normalize_trust_node_priority(
            config.trust_node_priority.as_slice(),
            deduped.keys().cloned().collect::<Vec<_>>().as_slice(),
        ),
        nodes: deduped.into_values().collect(),
    })
}

pub(crate) fn merge_community_node_resolved_urls(
    current: Option<CommunityNodeResolvedUrls>,
    incoming: Option<CommunityNodeResolvedUrls>,
) -> Result<Option<CommunityNodeResolvedUrls>> {
    match (current, incoming) {
        (None, None) => Ok(None),
        (Some(resolved), None) | (None, Some(resolved)) => Ok(Some(resolved)),
        (Some(current), Some(incoming)) => {
            let public_base_url = incoming.public_base_url;
            let connectivity_urls = current
                .connectivity_urls
                .into_iter()
                .chain(incoming.connectivity_urls)
                .collect();
            let mut seed_peers_by_endpoint = std::collections::BTreeMap::new();
            for seed_peer in current.seed_peers {
                seed_peers_by_endpoint.insert(seed_peer.endpoint_id.clone(), seed_peer);
            }
            for seed_peer in incoming.seed_peers {
                seed_peers_by_endpoint.insert(seed_peer.endpoint_id.clone(), seed_peer);
            }
            let seed_peers = seed_peers_by_endpoint.into_values().collect();
            Ok(Some(CommunityNodeResolvedUrls::new(
                public_base_url,
                connectivity_urls,
                seed_peers,
            )?))
        }
    }
}

pub(crate) fn refresh_community_node_resolved_urls(
    current: Option<CommunityNodeResolvedUrls>,
    incoming: CommunityNodeResolvedUrls,
) -> Result<CommunityNodeResolvedUrls> {
    let public_base_url = incoming.public_base_url;
    let connectivity_urls = current
        .map(|current| current.connectivity_urls)
        .unwrap_or_default()
        .into_iter()
        .chain(incoming.connectivity_urls)
        .collect();
    CommunityNodeResolvedUrls::new(public_base_url, connectivity_urls, incoming.seed_peers)
}

pub(crate) fn seed_peer_from_community_node(seed_peer: &CommunityNodeSeedPeer) -> Option<SeedPeer> {
    let endpoint_id = seed_peer.endpoint_id.trim();
    if endpoint_id.is_empty() {
        return None;
    }
    Some(SeedPeer {
        endpoint_id: endpoint_id.to_string(),
        addr_hint: seed_peer.addr_hint.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_resolved_urls_replaces_cached_addr_hint_with_incoming_endpoint() {
        let current = CommunityNodeResolvedUrls::new(
            "https://api.example.com",
            vec!["https://relay.example.com".to_string()],
            vec![
                CommunityNodeSeedPeer::new("peer-a", Some("172.20.80.1:40123".to_string()))
                    .expect("seed peer"),
            ],
        )
        .expect("current urls");
        let incoming = CommunityNodeResolvedUrls::new(
            "https://api.example.com",
            vec!["https://relay.example.com".to_string()],
            vec![CommunityNodeSeedPeer::new("peer-a", None).expect("seed peer")],
        )
        .expect("incoming urls");

        let merged = merge_community_node_resolved_urls(Some(current), Some(incoming))
            .expect("merged urls")
            .expect("resolved urls");

        assert_eq!(merged.seed_peers.len(), 1);
        assert_eq!(merged.seed_peers[0].endpoint_id, "peer-a");
        assert!(merged.seed_peers[0].addr_hint.is_none());
    }

    #[test]
    fn refresh_resolved_urls_replaces_seed_peer_snapshot() {
        let current = CommunityNodeResolvedUrls::new(
            "https://api.example.com",
            vec!["https://relay-a.example.com".to_string()],
            vec![CommunityNodeSeedPeer::new("peer-a", None).expect("seed peer")],
        )
        .expect("current urls");
        let incoming = CommunityNodeResolvedUrls::new(
            "https://api.example.com",
            vec!["https://relay-b.example.com".to_string()],
            vec![CommunityNodeSeedPeer::new("peer-b", None).expect("seed peer")],
        )
        .expect("incoming urls");

        let refreshed =
            refresh_community_node_resolved_urls(Some(current), incoming).expect("refreshed urls");

        assert_eq!(
            refreshed.connectivity_urls,
            vec![
                "https://relay-a.example.com".to_string(),
                "https://relay-b.example.com".to_string()
            ]
        );
        assert_eq!(
            refreshed.seed_peers,
            vec![CommunityNodeSeedPeer::new("peer-b", None).expect("seed peer")]
        );
    }
}
