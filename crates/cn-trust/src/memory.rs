//! in-memory `RelationStore`（テスト / 単体検証用）。
//!
//! ArcadeDB 実装（`cn-indexer`）と同じ contract テストを両実装に対して実行するための
//! 参照実装。proximity の合成は backend 非依存の [`proximity_from_features`] を使う。

use std::collections::BTreeMap;
use std::sync::RwLock;

use anyhow::Result;
use async_trait::async_trait;

use crate::relation::{
    ClusterRef, EdgeFeatures, Proximity, RelationStore, proximity_from_features,
};

/// 対称 edge の正規化 key（辞書順で (小, 大)）。
fn edge_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// in-memory relation graph。
#[derive(Debug, Default)]
pub struct MemoryRelationStore {
    edges: RwLock<BTreeMap<(String, String), EdgeFeatures>>,
    clusters: RwLock<BTreeMap<String, ClusterRef>>,
}

impl MemoryRelationStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RelationStore for MemoryRelationStore {
    async fn upsert_edge(&self, from: &str, to: &str, features: &EdgeFeatures) -> Result<()> {
        let mut edges = self.edges.write().expect("edges lock poisoned");
        edges.insert(edge_key(from, to), features.clone());
        Ok(())
    }

    async fn remove_edge(&self, from: &str, to: &str) -> Result<()> {
        let mut edges = self.edges.write().expect("edges lock poisoned");
        edges.remove(&edge_key(from, to));
        Ok(())
    }

    async fn pairwise_proximity(&self, viewer: &str, target: &str) -> Result<Option<Proximity>> {
        let edges = self.edges.read().expect("edges lock poisoned");
        Ok(edges
            .get(&edge_key(viewer, target))
            .map(proximity_from_features))
    }

    async fn neighbors(&self, viewer: &str, k: usize) -> Result<Vec<String>> {
        let edges = self.edges.read().expect("edges lock poisoned");
        let mut scored: Vec<(f64, String)> = edges
            .iter()
            .filter_map(|((a, b), features)| {
                let other = if a == viewer {
                    Some(b)
                } else if b == viewer {
                    Some(a)
                } else {
                    None
                }?;
                Some((proximity_from_features(features).score, other.clone()))
            })
            .collect();
        // proximity 降順・同点は pubkey 辞書順（決定論的な並び）。
        scored.sort_by(|(sa, pa), (sb, pb)| {
            sb.partial_cmp(sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| pa.cmp(pb))
        });
        Ok(scored.into_iter().take(k).map(|(_, p)| p).collect())
    }

    async fn cluster_of(&self, pubkey: &str) -> Result<Option<ClusterRef>> {
        let clusters = self.clusters.read().expect("clusters lock poisoned");
        Ok(clusters.get(pubkey).cloned())
    }

    async fn set_cluster(&self, pubkey: &str, cluster: &ClusterRef) -> Result<()> {
        let mut clusters = self.clusters.write().expect("clusters lock poisoned");
        clusters.insert(pubkey.to_string(), cluster.clone());
        Ok(())
    }

    async fn clear_cluster(&self, pubkey: &str) -> Result<()> {
        let mut clusters = self.clusters.write().expect("clusters lock poisoned");
        clusters.remove(pubkey);
        Ok(())
    }
}
