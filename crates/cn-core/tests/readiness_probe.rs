//! #616 プロバイダ疎通確認の期限付き保存の Postgres integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - slot 単位の upsert（同一 slot は上書き）。
//! - 保存されるのは判定と要約のみ（読み戻しで往復が安定する）。
//! - 索引の整合検査は他の行の数によらず同じ行を読む（#1714）。

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{TimeZone, Utc};
use kukuri_cn_core::{
    INDEX_INTEGRITY_SQL, IndexScopeKind, ReadinessProbeRecord, RelationAnalyzeRun, TestDatabase,
    add_supported_topic, connect_postgres, initialize_database, inspect_index_integrity,
    latest_relation_analyze_run, list_readiness_probes, record_relation_analyze_run,
    upsert_readiness_probe,
};
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

#[tokio::test]
async fn probe_cache_upserts_by_slot_and_round_trips() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping readiness probe test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_readiness_probe").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let first = ReadinessProbeRecord {
        configuration_fingerprint: None,
        provider_slot: "known_csam".to_string(),
        provider: "project-arachnid-shield".to_string(),
        pass: false,
        detail: "認証拒否 (HTTP 401)".to_string(),
        checked_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    };
    upsert_readiness_probe(&pool, &first).await?;

    // 同一 slot は上書きされ、行が増えない。
    let second = ReadinessProbeRecord {
        pass: true,
        detail: "認証と応答受信に成功".to_string(),
        checked_at: Utc.timestamp_opt(1_700_000_600, 0).unwrap(),
        ..first.clone()
    };
    upsert_readiness_probe(&pool, &second).await?;

    let third = ReadinessProbeRecord {
        configuration_fingerprint: Some("config-generation-1".into()),
        provider_slot: "general".to_string(),
        provider: "openai-compatible-vlm".to_string(),
        pass: true,
        detail: "接続と応答形式の解析に成功".to_string(),
        checked_at: Utc.timestamp_opt(1_700_000_700, 0).unwrap(),
    };
    upsert_readiness_probe(&pool, &third).await?;

    let records = list_readiness_probes(&pool).await?;
    assert_eq!(records, vec![third, second]);
    Ok(())
}

/// 表ごとに読んだ行（返した行と filter で捨てた行の和に loop 数を掛けたもの）を足す。
fn add_rows_read(node: &serde_json::Value, rows: &mut BTreeMap<String, f64>) {
    if let Some(relation) = node["Relation Name"].as_str() {
        let count = |key: &str| node[key].as_f64().unwrap_or(0.0);
        *rows.entry(relation.to_string()).or_default() += (count("Actual Rows")
            + count("Rows Removed by Filter")
            + count("Rows Removed by Index Recheck"))
            * count("Actual Loops");
    }
    for child in node["Plans"].as_array().into_iter().flatten() {
        add_rows_read(child, rows);
    }
}

/// 他の行（索引の行・それぞれが参照する許可の判定・公開 topic の supported）を `from..to` だけ足す。
/// 行数の計数の trigger を通すので 500 行ずつ入れる。
async fn add_other_rows(pool: &PgPool, from: i64, to: i64) -> Result<()> {
    for start in (from..to).step_by(500) {
        let end = (start + 500).min(to);
        for sql in [
            "INSERT INTO cn_safety.scan_verdicts
                (id, subject_kind, subject_id, action, critical, reason_code, policy_version,
                 scanned_at)
             SELECT 'verdict-' || g, 'post', 'post-' || g, 'allow', false, 'no_known_match',
                    'test', '2026-10-09T00:00:00Z'
             FROM generate_series($1::bigint, $2::bigint - 1) AS g",
            "INSERT INTO cn_index.index_entries
                (scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
                 verdict_id, verdict_action, critical)
             SELECT 'public_topic', 'topic-' || (g % 100), 'post-' || g, 'author-' || (g % 1000),
                    g, 'replica', 'verdict-' || g, 'allow', false
             FROM generate_series($1::bigint, $2::bigint - 1) AS g",
            "INSERT INTO cn_index.supported_topics (id, kind)
             SELECT 'topic-' || g, 'public_topic' FROM generate_series($1::bigint, $2::bigint - 1) AS g",
        ] {
            sqlx::query(sql).bind(start).bind(end).execute(pool).await?;
        }
    }
    Ok(())
}

/// #1714: readiness の整合検査は、他の行が増えても読む行が増えず、索引の総数と private channel の件数を返す。
#[tokio::test]
async fn index_integrity_inspection_does_not_read_other_rows() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping index integrity test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_readiness_integrity").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "channel-1714").await?;
        let mut reads = Vec::new();
        let mut others = 0;
        for total in [2_000, 20_000] {
            add_other_rows(&pool, others, total).await?;
            others = total;
            sqlx::query(
                "ANALYZE cn_index.index_entries, cn_safety.scan_verdicts, \
                 cn_index.supported_topics, cn_index.retention_state",
            )
            .execute(&pool)
            .await?;
            let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "EXPLAIN (ANALYZE, FORMAT JSON) {INDEX_INTEGRITY_SQL}"
            )))
            .fetch_one(&pool)
            .await?;
            let mut rows = BTreeMap::new();
            add_rows_read(&plan[0]["Plan"], &mut rows);
            reads.push(rows);
            let findings = inspect_index_integrity(&pool).await?;
            assert_eq!(findings.index_entries_total, total);
            assert_eq!(findings.private_scopes_supported, 1);
        }
        assert_eq!(
            reads[0], reads[1],
            "rows read must not grow with other rows"
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn relation_analyze_run_records_round_trip() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping relation run test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_relation_runs").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    assert_eq!(latest_relation_analyze_run(&pool).await?, None);

    let failed = RelationAnalyzeRun {
        started_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        finished_at: Utc.timestamp_opt(1_700_000_010, 0).unwrap(),
        success: false,
        edges_upserted: 0,
        clusters_assigned: 0,
        error: Some("ArcadeDB へ到達できません".to_string()),
    };
    record_relation_analyze_run(&pool, &failed).await?;
    let succeeded = RelationAnalyzeRun {
        started_at: Utc.timestamp_opt(1_700_000_100, 0).unwrap(),
        finished_at: Utc.timestamp_opt(1_700_000_110, 0).unwrap(),
        success: true,
        edges_upserted: 5,
        clusters_assigned: 2,
        error: None,
    };
    record_relation_analyze_run(&pool, &succeeded).await?;

    // 最新（finished_at の降順）が返る。
    assert_eq!(latest_relation_analyze_run(&pool).await?, Some(succeeded));
    Ok(())
}
