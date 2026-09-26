//! relation 解析 worker（batch・非常駐, ADR 0026 §6.1, #415）。
//!
//! 取込みが保存した 2 者間のアクション（`cn_core::relation_actions`）から、印の付いたペアと author だけを
//! 古い順に上限つきで読み、relation graph（`RelationStore`）へ反映する（#1221 R5-E）。
//!
//! - ペアの edge は双方向（方向ごとに種類は異なってよい）にアクションがあるときだけ作り、値は
//!   `shared_topics`＝アクションのあった public topic の数、`co_participation_events`＝アクションの件数。
//!   成立しないペアの edge は消す。
//! - cluster 帰属は author の dominant topic（最多の索引件数の public topic）。参加 0 なら外す。
//! - 反映した印だけを外す。途中で止まっても、残った印から次の実行が続ける。
//! - **canonical 非改変**: 書き込み先は relation graph（node-local derived overlay）のみ。
//! - 起動は `cn-cli relation analyze`（定期 timer）。

use anyhow::Result;
use sqlx::PgPool;

use kukuri_cn_core::{
    dirty_relation_authors, dirty_relation_pairs, settle_relation_author, settle_relation_pair,
};
use kukuri_cn_trust::{
    ClusterRef, EdgeFeatures, FEATURE_CO_PARTICIPATION_EVENTS, FEATURE_SHARED_TOPICS, RelationStore,
};

/// 1 回の解析で読むペアと author のそれぞれの上限。
pub const DEFAULT_ANALYSIS_LIMIT: usize = 10_000;

/// dominant topic 由来の cluster 名。
pub fn topic_cluster(topic_id: &str) -> ClusterRef {
    ClusterRef(format!("topic:{topic_id}"))
}

/// 解析結果の要約（CLI 出力・テスト検証用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelationAnalysisReport {
    /// upsert した pairwise edge 数。
    pub edges_upserted: usize,
    /// 消した pairwise edge 数。
    pub edges_removed: usize,
    /// 割り当てた cluster 帰属数。
    pub clusters_assigned: usize,
    /// 外した cluster 帰属数。
    pub clusters_cleared: usize,
}

/// 印の付いたペアと author を最大 `limit` 件ずつ relation graph へ反映する。
pub async fn analyze_relations(
    pool: &PgPool,
    store: &dyn RelationStore,
    limit: usize,
) -> Result<RelationAnalysisReport> {
    let mut report = RelationAnalysisReport::default();
    for change in dirty_relation_pairs(pool, limit).await? {
        if change.is_mutual() {
            let features = EdgeFeatures::new()
                .with(FEATURE_SHARED_TOPICS, change.shared_topics as f64)
                .with(
                    FEATURE_CO_PARTICIPATION_EVENTS,
                    (change.a_to_b + change.b_to_a) as f64,
                );
            store
                .upsert_edge(&change.author_a, &change.author_b, &features)
                .await?;
            report.edges_upserted += 1;
        } else {
            store
                .remove_edge(&change.author_a, &change.author_b)
                .await?;
            report.edges_removed += 1;
        }
        settle_relation_pair(pool, &change).await?;
    }
    for change in dirty_relation_authors(pool, limit).await? {
        match &change.dominant_topic {
            Some(topic) => {
                store
                    .set_cluster(&change.author_pubkey, &topic_cluster(topic))
                    .await?;
                report.clusters_assigned += 1;
            }
            None => {
                store.clear_cluster(&change.author_pubkey).await?;
                report.clusters_cleared += 1;
            }
        }
        settle_relation_author(pool, &change).await?;
    }
    Ok(report)
}
