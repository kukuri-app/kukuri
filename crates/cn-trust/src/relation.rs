//! relation（pairwise cluster proximity）のドメイン型と graph-store 抽象境界（ADR 0026 §6.1）。
//!
//! relation は CN が観測から導く node-local な derived overlay であり、social graph の
//! canonical を所有・改変しない（§2.2）。出力は cluster proximity（A から見た B の近さ）で
//! あって follow 関係でも risk ラベルでもない。
//!
//! graph-store 抽象境界（§6.1 Decision）: backend（最小 = ArcadeDB / scale = neo4j）を
//! 差し替え可能にする trait を置く。クエリは **Cypher 互換**表現に落とせることを前提とし、
//! proximity の合成（feature → score）は backend 非依存の純関数 [`proximity_from_features`]
//! に置いて、ArcadeDB / neo4j / in-memory が同一の score を返すようにする（backend は
//! edge property の格納と取り出しだけを担う）。

use std::collections::BTreeMap;

use anyhow::Result;
use async_trait::async_trait;
pub use kukuri_cn_protocol::{Proximity, ProximityBasisEntry};
use serde::{Deserialize, Serialize};

/// 2 者間のアクションがあった public topic の数の feature key（#1221 R5-E で意味を置換。key は維持）。
pub const FEATURE_SHARED_TOPICS: &str = "shared_topics";
/// 2 者間のアクション（返信・repost・引用・リアクション・フォロー）の件数の feature key（#1221 R5-E で意味を置換）。
pub const FEATURE_CO_PARTICIPATION_EVENTS: &str = "co_participation_events";
/// follow projection（ADR 0013）由来 feature の key。
///
/// **seam**: CN 側に follow projection の観測実装が無いため、producer は後続
/// （プラン Assumption 4）。key だけ固定し、値が入り次第 proximity に自然合流する。
pub const FEATURE_FOLLOW_PROJECTION: &str = "follow_projection";

/// pairwise edge に格納する feature 値（解析 worker が点数化して upsert する）。
///
/// key は `FEATURE_*` 定数。BTreeMap で決定論的な順序を保つ（説明・テストの安定性）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EdgeFeatures(pub BTreeMap<String, f64>);

impl EdgeFeatures {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: &str, value: f64) -> Self {
        self.0.insert(key.to_string(), value);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// cluster 帰属（相対成分の重み付け入力, §6.1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterRef(pub String);

/// feature の重み（初期決め打ち。チューニングは ADR 0026 §4 で後続 Issue）。
fn feature_weight(feature: &str) -> f64 {
    match feature {
        FEATURE_SHARED_TOPICS => 1.0,
        FEATURE_CO_PARTICIPATION_EVENTS => 1.0,
        FEATURE_FOLLOW_PROJECTION => 1.0,
        _ => 0.5,
    }
}

/// feature 値 → proximity score の backend 非依存合成。
///
/// 各 feature を飽和変換 `value / (value + 1)`（逓減。回数が増えるほど寄与が鈍る）で
/// `[0, 1)` に正規化し、重み付き平均を score とする。すべての backend（in-memory /
/// ArcadeDB / neo4j）はこの関数で score を合成し、同一 feature には同一 score を返す。
pub fn proximity_from_features(features: &EdgeFeatures) -> Proximity {
    let mut basis = Vec::new();
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;
    for (feature, value) in &features.0 {
        let weight = feature_weight(feature);
        let normalized = if *value > 0.0 {
            value / (value + 1.0)
        } else {
            0.0
        };
        let contribution = weight * normalized;
        weighted_sum += contribution;
        total_weight += weight;
        basis.push(ProximityBasisEntry {
            feature: feature.clone(),
            value: *value,
            weight,
            contribution,
        });
    }
    let score = if total_weight > 0.0 {
        (weighted_sum / total_weight).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Proximity { score, basis }
}

/// graph-store 抽象境界（ADR 0026 §6.1 の最小 API + cluster 書き込み）。
///
/// いずれも viewer / target は pubkey。クエリは Cypher 互換表現に落とせることを前提とし、
/// ArcadeDB / neo4j 双方で同一 API を満たす。relation graph は social graph canonical とは
/// 別物の node-local overlay であり、この trait に canonical への書き込み口は存在しない
/// （`relation_does_not_mutate_social_graph_canonical` の構造的保証）。
#[async_trait]
pub trait RelationStore: Send + Sync {
    /// 2 者間のアクション等の feature を pairwise edge に格納する。
    ///
    /// feature は対称なので、実装は (from, to) と (to, from) のどちらで読んでも同じ feature が
    /// 見えることを保証する。
    async fn upsert_edge(&self, from: &str, to: &str, features: &EdgeFeatures) -> Result<()>;

    /// 成立しなくなったペアの edge を消す（#1221 R5-E）。無ければ何もしない。
    async fn remove_edge(&self, from: &str, to: &str) -> Result<()>;

    /// viewer 視点の target への近接度（根拠つき）。edge が無ければ None。
    async fn pairwise_proximity(&self, viewer: &str, target: &str) -> Result<Option<Proximity>>;

    /// viewer から複数 candidate への proximity score（`[0, 1]`）。edge の無い candidate は含めない。
    ///
    /// relation 値 R の重み `w(A,U)`（ADR 0026 §8.2）に使う。backend は一括 query で上書きしてよい。
    async fn proximity_scores(
        &self,
        viewer: &str,
        candidates: &[String],
    ) -> Result<BTreeMap<String, f64>> {
        let mut scores = BTreeMap::new();
        for candidate in candidates {
            if let Some(proximity) = self.pairwise_proximity(viewer, candidate).await? {
                scores.insert(candidate.clone(), proximity.score);
            }
        }
        Ok(scores)
    }

    /// discovery / surfacing 用の近接近傍（proximity 降順で最大 k 件）。
    async fn neighbors(&self, viewer: &str, k: usize) -> Result<Vec<String>>;

    /// cluster 帰属（相対成分の重み付け入力）。未割り当てなら None。
    async fn cluster_of(&self, pubkey: &str) -> Result<Option<ClusterRef>>;

    /// 解析 worker が算出した cluster 帰属を格納する（§6.1 の最小 API に加える書き込み口）。
    async fn set_cluster(&self, pubkey: &str, cluster: &ClusterRef) -> Result<()>;

    /// public 参加が 0 になった author の cluster 帰属を外す（#1221 R5-E）。
    async fn clear_cluster(&self, pubkey: &str) -> Result<()>;
}
