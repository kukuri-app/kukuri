//! relation graph の ArcadeDB 実装（ADR 0026 §6.1, #415）。
//!
//! `cn-trust` の `RelationStore` trait（graph-store 抽象境界）を、index 投影と同じ ArcadeDB
//! インスタンスに相乗りする graph で実装する。クエリは **Cypher**（ArcadeDB の opencypher
//! サポート）で書き、scale path の neo4j（Cypher 互換）へ backend を差し替えても同じクエリ形が
//! 使えることを担保する。schema 定義（型 / index 作成）のみ ArcadeDB SQL（`IF NOT EXISTS` が
//! 必要なため。neo4j では相当する constraint 作成に置き換える）。
//!
//! - vertex `RelationUser { pubkey, cluster }`: relation の観測対象 user（社会グラフの canonical
//!   ではなく node-local overlay。この graph への書き込みは canonical に一切触れない）。
//! - edge `InteractsWith { features_json }`: pairwise の feature（`shared_topics` 等）。feature は
//!   JSON 文字列 1 property に格納する（neo4j も map property を持たないため、この表現は
//!   backend 間で可搬）。proximity の合成は backend 非依存の
//!   [`proximity_from_features`] で行い、in-memory 実装と同一の score を返す。
//! - edge は正規化ペア（辞書順 (小, 大)）で 1 本だけ張り、読みは無向 match（`-[r]-`）で
//!   双方向から同じ feature が見える（対称性 contract）。
//! - #1221 R5-E で観測を投稿の共起から 2 者間のアクションへ置き換えた。旧定義の型（`TrustUser` / `RelatesTo`）は
//!   意味が違うため読まず、`ensure_schema` が型ごと削除する。新しい型の edge・cluster は解析が差分で作り直す。

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use kukuri_cn_trust::{
    ClusterRef, EdgeFeatures, Proximity, RelationStore, proximity_from_features,
};

use crate::arcadedb::ArcadeDbClient;
use crate::config::ArcadeDbConfig;

/// relation graph の vertex type 名。
const USER_TYPE: &str = "RelationUser";
/// relation graph の edge type 名。
const EDGE_TYPE: &str = "InteractsWith";
/// 旧定義（投稿の共起、#1221 R5-E より前）の型。edge 型を先に消す。
const LEGACY_TYPES: [&str; 2] = ["RelatesTo", "TrustUser"];
/// `proximity_scores` の 1 query あたりの candidate 数。
const PROXIMITY_SCORES_CHUNK: usize = 200;

/// ArcadeDB 上の relation graph。
pub struct ArcadeDbRelationGraph {
    client: ArcadeDbClient,
}

impl ArcadeDbRelationGraph {
    pub fn new(config: ArcadeDbConfig) -> Result<Self> {
        Ok(Self {
            client: ArcadeDbClient::new(config)?,
        })
    }

    /// relation graph の schema（vertex / edge type + unique index）を冪等に用意する。
    pub async fn ensure_schema(&self) -> Result<()> {
        for legacy in LEGACY_TYPES {
            self.client
                .command("sql", &format!("DROP TYPE {legacy} IF EXISTS UNSAFE"))
                .await?;
        }
        self.client
            .command(
                "sql",
                &format!("CREATE VERTEX TYPE {USER_TYPE} IF NOT EXISTS"),
            )
            .await?;
        // `IF NOT EXISTS` は property 名の直後・データ型の前に置く（ArcadeDB SQL 文法。
        // 型の後ろに置くと 26.x で CommandSQLParsingException になる）。
        for (name, data_type) in [("pubkey", "STRING"), ("cluster", "STRING")] {
            self.client
                .command(
                    "sql",
                    &format!("CREATE PROPERTY {USER_TYPE}.{name} IF NOT EXISTS {data_type}"),
                )
                .await?;
        }
        self.client
            .command(
                "sql",
                &format!("CREATE INDEX IF NOT EXISTS ON {USER_TYPE} (pubkey) UNIQUE"),
            )
            .await?;
        self.client
            .command(
                "sql",
                &format!("CREATE EDGE TYPE {EDGE_TYPE} IF NOT EXISTS"),
            )
            .await?;
        self.client
            .command(
                "sql",
                &format!("CREATE PROPERTY {EDGE_TYPE}.features_json IF NOT EXISTS STRING"),
            )
            .await?;
        Ok(())
    }
}

/// 正規化ペア（辞書順）。foundation の feature は対称なので edge は 1 本に正規化する。
fn normalized_pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Cypher の応答行から文字列カラムを読む。
fn string_column(row: &Value, column: &str) -> Option<String> {
    row.get(column).and_then(Value::as_str).map(str::to_string)
}

/// 応答の `result` 配列。
fn result_rows(value: &Value) -> Vec<Value> {
    value
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `features_json` 文字列（edge property）を `EdgeFeatures` に戻す。
fn features_from_json(raw: &str) -> Result<EdgeFeatures> {
    serde_json::from_str(raw).context("failed to decode RelatesTo.features_json")
}

#[async_trait]
impl RelationStore for ArcadeDbRelationGraph {
    async fn upsert_edge(&self, from: &str, to: &str, features: &EdgeFeatures) -> Result<()> {
        let (a, b) = normalized_pair(from, to);
        let features_json =
            serde_json::to_string(features).context("failed to encode edge features")?;
        // MERGE は vertex / edge とも冪等（存在しなければ作り、あれば再利用）。
        self.client
            .command_with_params(
                "cypher",
                &format!(
                    "MERGE (a:{USER_TYPE} {{pubkey: $a}}) \
                     MERGE (b:{USER_TYPE} {{pubkey: $b}}) \
                     MERGE (a)-[r:{EDGE_TYPE}]->(b) \
                     SET r.features_json = $features_json"
                ),
                json!({ "a": a, "b": b, "features_json": features_json }),
            )
            .await?;
        Ok(())
    }

    async fn remove_edge(&self, from: &str, to: &str) -> Result<()> {
        self.client
            .command_with_params(
                "cypher",
                &format!(
                    "MATCH (a:{USER_TYPE} {{pubkey: $a}})-[r:{EDGE_TYPE}]-(b:{USER_TYPE} {{pubkey: $b}}) DELETE r"
                ),
                json!({ "a": from, "b": to }),
            )
            .await?;
        Ok(())
    }

    async fn pairwise_proximity(&self, viewer: &str, target: &str) -> Result<Option<Proximity>> {
        // 無向 match（-[r]-）: 正規化ペアで張った 1 本の edge が双方向から見える（対称性）。
        let value = self
            .client
            .command_with_params(
                "cypher",
                &format!(
                    "MATCH (a:{USER_TYPE} {{pubkey: $a}})-[r:{EDGE_TYPE}]-(b:{USER_TYPE} {{pubkey: $b}}) \
                     RETURN r.features_json AS features_json LIMIT 1"
                ),
                json!({ "a": viewer, "b": target }),
            )
            .await?;
        let rows = result_rows(&value);
        let Some(raw) = rows
            .first()
            .and_then(|row| string_column(row, "features_json"))
        else {
            return Ok(None);
        };
        // score の合成は backend 非依存の純関数（in-memory 実装と同一の値を返す contract）。
        Ok(Some(proximity_from_features(&features_from_json(&raw)?)))
    }

    async fn proximity_scores(
        &self,
        viewer: &str,
        candidates: &[String],
    ) -> Result<BTreeMap<String, f64>> {
        // relation 値 R の重み（ADR 0026 §8.2）。candidate ごとの往復を避け、分割した一括 query で読む。
        let mut scores = BTreeMap::new();
        for chunk in candidates.chunks(PROXIMITY_SCORES_CHUNK) {
            let value = self
                .client
                .command_with_params(
                    "cypher",
                    &format!(
                        "MATCH (a:{USER_TYPE} {{pubkey: $viewer}})-[r:{EDGE_TYPE}]-(b:{USER_TYPE}) \
                         WHERE b.pubkey IN $candidates \
                         RETURN b.pubkey AS pubkey, r.features_json AS features_json"
                    ),
                    json!({ "viewer": viewer, "candidates": chunk }),
                )
                .await?;
            for row in result_rows(&value) {
                let (Some(pubkey), Some(raw)) = (
                    string_column(&row, "pubkey"),
                    string_column(&row, "features_json"),
                ) else {
                    continue;
                };
                let score = proximity_from_features(&features_from_json(&raw)?).score;
                scores.insert(pubkey, score);
            }
        }
        Ok(scores)
    }

    async fn neighbors(&self, viewer: &str, k: usize) -> Result<Vec<String>> {
        let value = self
            .client
            .command_with_params(
                "cypher",
                &format!(
                    "MATCH (a:{USER_TYPE} {{pubkey: $viewer}})-[r:{EDGE_TYPE}]-(b:{USER_TYPE}) \
                     RETURN b.pubkey AS pubkey, r.features_json AS features_json"
                ),
                json!({ "viewer": viewer }),
            )
            .await?;
        let mut scored = Vec::new();
        for row in result_rows(&value) {
            let (Some(pubkey), Some(raw)) = (
                string_column(&row, "pubkey"),
                string_column(&row, "features_json"),
            ) else {
                continue;
            };
            let score = proximity_from_features(&features_from_json(&raw)?).score;
            scored.push((score, pubkey));
        }
        // proximity 降順・同点は pubkey 辞書順（in-memory 実装と同じ決定論的な並び）。
        scored.sort_by(|(sa, pa), (sb, pb)| {
            sb.partial_cmp(sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| pa.cmp(pb))
        });
        Ok(scored.into_iter().take(k).map(|(_, p)| p).collect())
    }

    async fn cluster_of(&self, pubkey: &str) -> Result<Option<ClusterRef>> {
        let value = self
            .client
            .command_with_params(
                "cypher",
                &format!(
                    "MATCH (u:{USER_TYPE} {{pubkey: $pubkey}}) RETURN u.cluster AS cluster LIMIT 1"
                ),
                json!({ "pubkey": pubkey }),
            )
            .await?;
        let rows = result_rows(&value);
        Ok(rows
            .first()
            .and_then(|row| string_column(row, "cluster"))
            .map(ClusterRef))
    }

    async fn set_cluster(&self, pubkey: &str, cluster: &ClusterRef) -> Result<()> {
        self.client
            .command_with_params(
                "cypher",
                &format!("MERGE (u:{USER_TYPE} {{pubkey: $pubkey}}) SET u.cluster = $cluster"),
                json!({ "pubkey": pubkey, "cluster": cluster.0 }),
            )
            .await?;
        Ok(())
    }

    async fn clear_cluster(&self, pubkey: &str) -> Result<()> {
        self.client
            .command_with_params(
                "cypher",
                &format!("MATCH (u:{USER_TYPE} {{pubkey: $pubkey}}) REMOVE u.cluster"),
                json!({ "pubkey": pubkey }),
            )
            .await?;
        Ok(())
    }
}
