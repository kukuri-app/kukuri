//! relation graph / 解析 worker の contract テスト（ADR 0026 §6.1、#415、#1221 R5-E）。
//!
//! - Postgres（`KUKURI_CN_RUN_INTEGRATION_TESTS=1`）: 2 者間のアクションの行 → trigger の差分 → 解析の反映を固定する。
//!   双方向のペアだけ edge を作り、成立しなくなった edge と参加 0 の cluster を消す。乱数の操作列を上限の小さい
//!   解析で少しずつ反映しても、全行から求めた期待値（oracle）と一致する。履歴を 10 倍にしても 1 件の変化で読む行と
//!   書く edge は同じ。
//! - ArcadeDB（`KUKURI_CN_RUN_ARCADEDB_TESTS=1`、要 live ArcadeDB）: in-memory と同一の共有 contract スイート。

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use sqlx::PgPool;

use kukuri_cn_core::{
    IndexScopeKind, NewIndexEntry, RelationAction, RelationActionKind, TestDatabase,
    connect_postgres, initialize_database, record_relation_action, remove_relation_action,
    upsert_index_entry, upsert_scan_verdict,
};
use kukuri_cn_indexer::{ArcadeDbConfig, ArcadeDbRelationGraph, analyze_relations, topic_cluster};
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{ReasonCode, SafetyAction, SafetyVerdict};
use kukuri_cn_safety_runtime::VerdictPersistMeta;
use kukuri_cn_trust::relation_testing::assert_relation_store_contracts;
use kukuri_cn_trust::{
    FEATURE_CO_PARTICIPATION_EVENTS, FEATURE_SHARED_TOPICS, MemoryRelationStore, RelationStore,
};

fn allow_verdict() -> SafetyVerdict {
    SafetyVerdict {
        action: SafetyAction::Allow,
        labels: Vec::new(),
        advisory_labels: Vec::new(),
        critical: false,
        reason_code: ReasonCode::NoKnownMatch,
        confidence: None,
        provider: Some("mock-known-csam".to_string()),
        provider_capability: None,
        policy_version: "policy-v1-test".to_string(),
        scanned_at: "2026-07-02T09:00:00Z".to_string(),
    }
}

async fn with_database<F, Fut>(prefix: &str, test: F) -> Result<()>
where
    F: FnOnce(PgPool) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let Some(admin_url) = kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        "postgres://cn:cn_password@127.0.0.1:15432/cn",
    ) else {
        eprintln!("skipping relation test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), prefix).await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        test(pool.clone()).await
    }
    .await;
    pool.close().await;
    database.cleanup().await?;
    result
}

async fn index_post(
    pool: &PgPool,
    scope_kind: IndexScopeKind,
    scope_id: &str,
    object_id: &str,
    author: &str,
) -> Result<()> {
    let verdict = upsert_scan_verdict(
        pool,
        SubjectKind::Post,
        object_id,
        &allow_verdict(),
        &VerdictPersistMeta::default(),
    )
    .await?;
    upsert_index_entry(
        pool,
        &NewIndexEntry {
            scope_kind,
            scope_id: scope_id.to_string(),
            object_id: object_id.to_string(),
            author_pubkey: author.to_string(),
            created_at: 1_700_000_000,
            source_replica_id: format!("topic::{scope_id}"),
            verdict_id: verdict.id,
            verdict_action: "allow".to_string(),
            critical: false,
        },
    )
    .await?;
    Ok(())
}

async fn remove_post(pool: &PgPool, scope_id: &str, object_id: &str) -> Result<()> {
    sqlx::query(
        "DELETE FROM cn_index.index_entries
         WHERE scope_kind = 'public_topic' AND scope_id = $1 AND object_id = $2",
    )
    .bind(scope_id)
    .bind(object_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn action(
    kind: RelationActionKind,
    source: &str,
    actor: &str,
    target: &str,
    scope: &str,
    anchor: &str,
) -> RelationAction {
    RelationAction {
        kind,
        source_id: source.to_string(),
        actor_pubkey: actor.to_string(),
        target_pubkey: target.to_string(),
        scope_id: Some(scope.to_string()),
        anchor_object_id: Some(anchor.to_string()),
    }
}

async fn analyze_until_settled(
    pool: &PgPool,
    graph: &dyn RelationStore,
    limit: usize,
) -> Result<usize> {
    let mut rounds = 0;
    loop {
        let report = analyze_relations(pool, graph, limit).await?;
        rounds += 1;
        if report.edges_upserted
            + report.edges_removed
            + report.clusters_assigned
            + report.clusters_cleared
            == 0
        {
            return Ok(rounds);
        }
    }
}

fn feature(proximity: &kukuri_cn_trust::Proximity, key: &str) -> f64 {
    proximity
        .basis
        .iter()
        .find(|entry| entry.feature == key)
        .map_or(0.0, |entry| entry.value)
}

#[tokio::test]
async fn relation_edges_need_actions_in_both_directions_and_follow_removal() -> Result<()> {
    with_database("cn_relation_mutual", |pool| async move {
        use IndexScopeKind::{PrivateChannel, PublicTopic};
        index_post(&pool, PublicTopic, "topic-a", "post-a", "author-a").await?;
        index_post(&pool, PublicTopic, "topic-a", "reply-b", "author-b").await?;
        index_post(&pool, PublicTopic, "topic-b", "post-b", "author-b").await?;
        index_post(&pool, PublicTopic, "topic-b", "post-b2", "author-b").await?;
        index_post(&pool, PublicTopic, "topic-a", "reply-c", "author-c").await?;
        index_post(&pool, PrivateChannel, "chan-x", "post-d", "author-d").await?;
        // B → A（返信）、A → B（リアクション、別 topic）、C → A（返信だけ、片方向）。
        record_relation_action(
            &pool,
            &action(
                RelationActionKind::Reply,
                "reply-b",
                "author-b",
                "author-a",
                "topic-a",
                "reply-b",
            ),
        )
        .await?;
        record_relation_action(
            &pool,
            &action(
                RelationActionKind::Reaction,
                "reaction-a",
                "author-a",
                "author-b",
                "topic-b",
                "post-b",
            ),
        )
        .await?;
        record_relation_action(
            &pool,
            &action(
                RelationActionKind::Reply,
                "reply-c",
                "author-c",
                "author-a",
                "topic-a",
                "reply-c",
            ),
        )
        .await?;
        // 自分自身へのアクションは保存しない。
        assert!(
            !record_relation_action(
                &pool,
                &action(
                    RelationActionKind::Reply,
                    "self",
                    "author-a",
                    "author-a",
                    "topic-a",
                    "post-a"
                ),
            )
            .await?
        );
        let graph = MemoryRelationStore::new();
        analyze_until_settled(&pool, &graph, 1000).await?;

        let ab = graph
            .pairwise_proximity("author-a", "author-b")
            .await?
            .expect("mutual actions make an edge");
        assert_eq!(feature(&ab, FEATURE_SHARED_TOPICS), 2.0);
        assert_eq!(feature(&ab, FEATURE_CO_PARTICIPATION_EVENTS), 2.0);
        assert!(
            graph
                .pairwise_proximity("author-a", "author-c")
                .await?
                .is_none()
        );
        assert_eq!(
            graph.cluster_of("author-a").await?,
            Some(topic_cluster("topic-a"))
        );
        assert_eq!(
            graph.cluster_of("author-b").await?,
            Some(topic_cluster("topic-b"))
        );
        // private channel だけの author は cluster を持たない。
        assert!(graph.cluster_of("author-d").await?.is_none());

        // 相互フォローだけのペアは作らない。
        record_relation_action(&pool, &RelationAction::follow("author-a", "author-e")).await?;
        record_relation_action(&pool, &RelationAction::follow("author-e", "author-a")).await?;
        analyze_until_settled(&pool, &graph, 1000).await?;
        assert!(
            graph
                .pairwise_proximity("author-a", "author-e")
                .await?
                .is_none()
        );

        // A がフォローを加えると A → C もそろう。フォローは topic を数えない。
        record_relation_action(&pool, &RelationAction::follow("author-a", "author-c")).await?;
        analyze_until_settled(&pool, &graph, 1000).await?;
        let ac = graph
            .pairwise_proximity("author-c", "author-a")
            .await?
            .expect("follow completes the reverse direction");
        assert_eq!(feature(&ac, FEATURE_SHARED_TOPICS), 1.0);
        assert_eq!(feature(&ac, FEATURE_CO_PARTICIPATION_EVENTS), 2.0);

        // 返信の投稿の索引が消えると（撤回・送信防止・scope 解除）アクションも消え、片方向になった edge は消える。
        remove_post(&pool, "topic-a", "reply-b").await?;
        remove_relation_action(
            &pool,
            RelationActionKind::Follow,
            &RelationAction::follow("author-a", "author-c").source_id,
        )
        .await?;
        analyze_until_settled(&pool, &graph, 1000).await?;
        assert!(
            graph
                .pairwise_proximity("author-a", "author-b")
                .await?
                .is_none()
        );
        assert!(
            graph
                .pairwise_proximity("author-a", "author-c")
                .await?
                .is_none()
        );
        // 参加が 0 になった author の cluster は外れる。
        remove_post(&pool, "topic-b", "post-b").await?;
        remove_post(&pool, "topic-b", "post-b2").await?;
        analyze_until_settled(&pool, &graph, 1000).await?;
        assert!(graph.cluster_of("author-b").await?.is_none());
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cn_index.relation_pairs")
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            remaining, 2,
            "only the one-way A-C reply pair and the follow-only A-E pair keep their counters"
        );
        Ok(())
    })
    .await
}

/// 小さな決定的乱数（テスト内の操作列用）。
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

#[tokio::test]
async fn bounded_analysis_steps_converge_to_the_action_oracle() -> Result<()> {
    with_database("cn_relation_oracle", |pool| async move {
        let authors = (0..6).map(|i| format!("author-{i}")).collect::<Vec<_>>();
        let topics = ["t0", "t1", "t2"];
        let graph = MemoryRelationStore::new();
        let mut rng = Lcg(7);
        // 生きている行: (kind, source) -> (actor, target, scope)
        let mut live: BTreeMap<(String, String), (usize, usize, Option<usize>)> = BTreeMap::new();
        // 索引: (topic, object) -> author
        let mut posts: BTreeMap<(usize, String), usize> = BTreeMap::new();
        for step in 0..300 {
            match rng.next(5) {
                0 | 1 => {
                    let (actor, target) = (rng.next(6) as usize, rng.next(6) as usize);
                    let topic = rng.next(3) as usize;
                    let source = format!("act-{step}");
                    // アクションの起点となる投稿を索引へ入れる（返信・repost の行為の投稿）。
                    index_post(
                        &pool,
                        IndexScopeKind::PublicTopic,
                        topics[topic],
                        &source,
                        &authors[actor],
                    )
                    .await?;
                    posts.insert((topic, source.clone()), actor);
                    let kind = if rng.next(2) == 0 {
                        RelationActionKind::Reply
                    } else {
                        RelationActionKind::Repost
                    };
                    if record_relation_action(
                        &pool,
                        &action(
                            kind,
                            &source,
                            &authors[actor],
                            &authors[target],
                            topics[topic],
                            &source,
                        ),
                    )
                    .await?
                    {
                        live.insert((kind.as_str().into(), source), (actor, target, Some(topic)));
                    }
                }
                2 => {
                    let (actor, target) = (rng.next(6) as usize, rng.next(6) as usize);
                    if actor != target {
                        let follow = RelationAction::follow(&authors[actor], &authors[target]);
                        record_relation_action(&pool, &follow).await?;
                        live.insert(("follow".into(), follow.source_id), (actor, target, None));
                    }
                }
                3 => {
                    // 索引から投稿を 1 件消す（起点のアクションも消える）。
                    if let Some(key) = posts
                        .keys()
                        .nth(rng.next(posts.len().max(1) as u64) as usize)
                        .cloned()
                    {
                        remove_post(&pool, topics[key.0], &key.1).await?;
                        posts.remove(&key);
                        live.retain(|(_, source), _| source != &key.1);
                    }
                }
                _ => {
                    // 上限の小さい解析を途中まで進める（停止と再開）。
                    analyze_relations(&pool, &graph, 2).await?;
                }
            }
        }
        analyze_until_settled(&pool, &graph, 2).await?;

        for a in 0..6 {
            for b in (a + 1)..6 {
                let (mut ab, mut ba, mut shared) = (0, 0, BTreeSet::new());
                for (actor, target, topic) in live.values() {
                    if (*actor, *target) == (a, b) {
                        ab += 1;
                    } else if (*actor, *target) == (b, a) {
                        ba += 1;
                    } else {
                        continue;
                    }
                    if let Some(topic) = topic {
                        shared.insert(*topic);
                    }
                }
                let edge = graph.pairwise_proximity(&authors[a], &authors[b]).await?;
                if ab > 0 && ba > 0 && !shared.is_empty() {
                    let edge = edge.expect("oracle expects an edge");
                    assert_eq!(feature(&edge, FEATURE_SHARED_TOPICS), shared.len() as f64);
                    assert_eq!(
                        feature(&edge, FEATURE_CO_PARTICIPATION_EVENTS),
                        (ab + ba) as f64
                    );
                } else {
                    assert!(edge.is_none(), "one-way pair ({a}, {b}) must have no edge");
                }
            }
        }
        for (index, author) in authors.iter().enumerate() {
            let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
            for ((topic, _), owner) in &posts {
                if *owner == index {
                    *counts.entry(*topic).or_default() += 1;
                }
            }
            let dominant = counts
                .iter()
                .max_by(|(ta, ca), (tb, cb)| ca.cmp(cb).then(topics[**tb].cmp(topics[**ta])))
                .map(|(topic, _)| topic_cluster(topics[*topic]));
            assert_eq!(
                graph.cluster_of(author).await?,
                dominant,
                "cluster of {author}"
            );
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn one_change_after_tenfold_history_reads_and_writes_the_same() -> Result<()> {
    let mut observed = Vec::new();
    for history in [1usize, 10] {
        let observed_ref = &mut observed;
        with_database("cn_relation_scale", |pool| async move {
            let graph = MemoryRelationStore::new();
            for pair in 0..(20 * history) {
                let (a, b) = (format!("a-{pair:04}"), format!("b-{pair:04}"));
                index_post(
                    &pool,
                    IndexScopeKind::PublicTopic,
                    "topic",
                    &format!("pa-{pair}"),
                    &a,
                )
                .await?;
                index_post(
                    &pool,
                    IndexScopeKind::PublicTopic,
                    "topic",
                    &format!("pb-{pair}"),
                    &b,
                )
                .await?;
                record_relation_action(
                    &pool,
                    &action(
                        RelationActionKind::Reply,
                        &format!("pa-{pair}"),
                        &a,
                        &b,
                        "topic",
                        &format!("pa-{pair}"),
                    ),
                )
                .await?;
                record_relation_action(
                    &pool,
                    &action(
                        RelationActionKind::Reply,
                        &format!("pb-{pair}"),
                        &b,
                        &a,
                        "topic",
                        &format!("pb-{pair}"),
                    ),
                )
                .await?;
            }
            analyze_until_settled(&pool, &graph, 10_000).await?;
            // 既存のペアに 1 件のフォローを足す（索引は変わらない）。
            record_relation_action(&pool, &RelationAction::follow("a-0000", "b-0000")).await?;
            let marked: i64 = sqlx::query_scalar(
                "SELECT (SELECT COUNT(*) FROM cn_index.relation_pairs WHERE dirty_seq IS NOT NULL)
                      + (SELECT COUNT(*) FROM cn_index.relation_dirty_authors)",
            )
            .fetch_one(&pool)
            .await?;
            let report = analyze_relations(&pool, &graph, 10_000).await?;
            observed_ref.push((marked, report));
            Ok(())
        })
        .await?;
    }
    if observed.is_empty() {
        return Ok(());
    }
    assert_eq!(
        observed[0], observed[1],
        "the work of one change must not grow with history"
    );
    assert_eq!(observed[0].0, 1);
    assert_eq!(observed[0].1.edges_upserted, 1);
    Ok(())
}

/// ArcadeDB backend への共有 contract スイート実行。
///
/// `KUKURI_CN_RUN_ARCADEDB_TESTS=1` かつ live ArcadeDB（`COMMUNITY_NODE_ARCADEDB_*`）が
/// あるときのみ実行する（他の integration テストと同じ env-gate 流儀）。
#[tokio::test]
async fn arcadedb_relation_store_satisfies_shared_contracts() -> Result<()> {
    if !kukuri_test_support::env_flag_enabled("KUKURI_CN_RUN_ARCADEDB_TESTS") {
        eprintln!("skipping ArcadeDB relation test; set KUKURI_CN_RUN_ARCADEDB_TESTS=1");
        return Ok(());
    }
    let graph = ArcadeDbRelationGraph::new(ArcadeDbConfig::from_env())?;
    graph.ensure_schema().await?;
    // 走行間の残留データと衝突しない一意 prefix（in-memory と同一スイート）。
    let prefix = format!(
        "cntest-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    assert_relation_store_contracts(&graph, &prefix).await?;
    Ok(())
}
