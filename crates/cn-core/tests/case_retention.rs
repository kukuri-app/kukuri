//! #1704 案件保持（ADR 0034）の期限の判定と期限削除の Postgres integration テスト。

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Duration, SubsecRound, Utc};
use kukuri_cn_core::{
    CleanupCounts, EXPIRED_DELETES, LegalDataCipher, RetentionPolicy, TestDatabase,
    cleanup_expired, configure_case_retention, connect_postgres, export_legal_hold,
    get_community_node_report, get_rights_request, get_risk_signal, get_signed_moderation_event,
    get_tester_feedback, initialize_database, list_operator_actions, migrate_postgres_up_to,
    start_legal_hold,
};
use serde_json::Value;
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

/// 8 表へ起算点 `$1` の行を `$2` 行ずつ入れる。`$3` で id を分ける。機微区分は 4 区分、申出は 3 つの
/// 保持区分（未解決・措置済み・却下等）に同じ数を入れ、申出ごとに履歴を 1 行付ける。
const SEED: [&str; 8] = [
    "INSERT INTO cn_admin.reports (id, subject_kind, subject_id, capability, reason, created_at)
     SELECT 'report-' || $3 || '-' || i, 'post', 'post-' || i, 'moderation', 'spam', $1
     FROM generate_series(1, $2) AS i",
    "INSERT INTO cn_admin.tester_feedback
         (id, what_attempted, what_happened, what_seemed_wrong, client_version, os, created_at)
     SELECT 'feedback-' || $3 || '-' || i, 'a', 'b', 'c', '0.4.3', 'linux', $1
     FROM generate_series(1, $2) AS i",
    "INSERT INTO cn_legal.sensitive_items
         (id, owner_kind, owner_id, data_category, nonce, ciphertext, created_at)
     SELECT 'item-' || $3 || '-' || c || '-' || i, 'report', 'owner-' || $3 || '-' || i, c,
            decode(repeat('00', 24), 'hex'), decode(repeat('00', 32), 'hex'), $1
     FROM generate_series(1, $2) AS i
     CROSS JOIN unnest(ARRAY['report_contact', 'rights_request_contact',
                             'rights_request_identity', 'rights_request_evidence']) AS c",
    "INSERT INTO cn_legal.rights_requests
         (id, tracking_secret_hash, scope_revision, scope_status, status, subject_kind, subject_id,
          requested_capabilities, request_data, created_at, updated_at)
     SELECT 'request-' || $3 || '-' || s || '-' || i, repeat('0', 64), 'scope-1',
            'verified_scope', s, 'post', 'post-' || i, ARRAY['moderation'],
            jsonb_build_object(
                'scope_revision', 'scope-1', 'scope_acknowledged', true,
                'requester_kind', 'rights_holder', 'requester_name', '', 'email', '',
                'rights_category', 'copyright', 'rights_basis', 'b', 'subject_kind', 'post',
                'subject_id', 'post-' || i, 'infringement_description', 'd',
                'no_permission_statement', true),
            $1, $1
     FROM generate_series(1, $2) AS i
     CROSS JOIN unnest(ARRAY['received', 'actioned', 'declined']) AS s",
    "INSERT INTO cn_legal.rights_request_events (id, request_id, actor, action, to_status, occurred_at)
     SELECT 'event-' || id, id, 'operator', 'rights_request.create', status, $1
     FROM cn_legal.rights_requests
     WHERE id LIKE 'request-' || $3 || '-%' AND $2 > 0",
    "INSERT INTO cn_admin.operator_actions
         (id, occurred_at, actor, action, target_kind, target_id, before_json, after_json)
     SELECT 'action-' || $3 || '-' || i, $1, 'operator', 'report.set_status', 'report',
            'report-' || i, '{}'::jsonb, '{}'::jsonb
     FROM generate_series(1, $2) AS i",
    "INSERT INTO cn_safety.signed_moderation_events
         (id, issuer_node_id, target_type, target_id, action, reason_code, severity, basis,
          visibility, policy_version, signature, event_created_at, persisted_at)
     SELECT 'moderation-' || $3 || '-' || i, 'issuer', 'post', 'post-' || i, 'exclude',
            'general_moderation', 'low', 'classifier_score', 'local', 'v1', 'signature',
            '2026-10-09T00:00:00Z', $1
     FROM generate_series(1, $2) AS i",
    "INSERT INTO cn_safety.risk_signals
         (id, issuer_node_id, target, target_id, category, severity, basis, visibility,
          persisted_at)
     SELECT 'signal-' || $3 || '-' || i, 'issuer', 'post_id', 'post-' || $3 || '-' || i, 'spam',
            'low', 'classifier_score', 'local', $1
     FROM generate_series(1, $2) AS i",
];

/// `SEED` で 1 行ずつ入れた起算点の行が、すべて期限切れになったときに消える行の数。
const ONE_EACH: CleanupCounts = CleanupCounts {
    sensitive_items: 4,
    rights_request_events: 3,
    reports: 1,
    tester_feedback: 1,
    rights_requests: 3,
    operator_actions: 1,
    moderation_events: 1,
    risk_signals: 1,
};

async fn seed(pool: &PgPool, tag: &str, start: DateTime<Utc>, count: i32) -> Result<()> {
    for sql in SEED {
        sqlx::query(sql)
            .bind(start)
            .bind(count)
            .bind(tag)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 8 表の全行の (表, id, 行の版)。行が書き直されると版（`xmin`）が変わる。
async fn row_versions(pool: &PgPool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT 'reports', id, xmin::text FROM cn_admin.reports
         UNION ALL SELECT 'tester_feedback', id, xmin::text FROM cn_admin.tester_feedback
         UNION ALL SELECT 'sensitive_items', id, xmin::text FROM cn_legal.sensitive_items
         UNION ALL SELECT 'rights_requests', id, xmin::text FROM cn_legal.rights_requests
         UNION ALL SELECT 'rights_request_events', id, xmin::text
             FROM cn_legal.rights_request_events
         UNION ALL SELECT 'operator_actions', id, xmin::text FROM cn_admin.operator_actions
         UNION ALL SELECT 'moderation_events', id, xmin::text
             FROM cn_safety.signed_moderation_events
         UNION ALL SELECT 'risk_signals', id, xmin::text FROM cn_safety.risk_signals
         ORDER BY 1, 2",
    )
    .fetch_all(pool)
    .await?)
}

/// すべての保持区分を `days` 日にした設定。
fn every_category(days: u32) -> RetentionPolicy {
    RetentionPolicy {
        report_days: days,
        report_contact_days: days,
        tester_feedback_days: days,
        rights_request_active_days: days,
        rights_request_resolved_days: days,
        rights_request_rejected_days: days,
        rights_request_contact_days: days,
        rights_request_identity_days: days,
        rights_request_evidence_days: days,
        rights_request_history_days: days,
        operator_audit_days: days,
        moderation_event_days: days,
        risk_signal_days: days,
    }
}

/// 通常読取りが `tag` の 1 行目を返すか。機微区分は復号が要るので削除で確かめ、履歴には通常読取りが無い。
async fn visible(pool: &PgPool, tag: &str) -> Result<Vec<bool>> {
    let action = format!("action-{tag}-1");
    Ok(vec![
        get_community_node_report(pool, &format!("report-{tag}-1"))
            .await?
            .is_some(),
        get_tester_feedback(pool, &format!("feedback-{tag}-1"))
            .await?
            .is_some(),
        get_rights_request(pool, &format!("request-{tag}-received-1"))
            .await?
            .is_some(),
        get_rights_request(pool, &format!("request-{tag}-actioned-1"))
            .await?
            .is_some(),
        get_rights_request(pool, &format!("request-{tag}-declined-1"))
            .await?
            .is_some(),
        list_operator_actions(pool, 200, 0)
            .await?
            .iter()
            .any(|entry| entry.id == action),
        get_signed_moderation_event(pool, &format!("moderation-{tag}-1"))
            .await?
            .is_some(),
        get_risk_signal(pool, &format!("signal-{tag}-1"))
            .await?
            .is_some(),
    ])
}

/// 実行計画（`EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`）の走査の節点で、表ごとに読んだ行の数と
/// 触ったページの数を足す。
fn add_reads(plan: &Value, totals: &mut BTreeMap<String, (f64, f64)>) {
    if let Some(relation) = plan["Relation Name"].as_str() {
        let rows: f64 = [
            "Actual Rows",
            "Rows Removed by Filter",
            "Rows Removed by Index Recheck",
        ]
        .iter()
        .filter_map(|key| plan[*key].as_f64())
        .sum();
        let pages: f64 = ["Shared Hit Blocks", "Shared Read Blocks"]
            .iter()
            .filter_map(|key| plan[*key].as_f64())
            .sum();
        let total = totals.entry(relation.to_string()).or_default();
        total.0 += rows * plan["Actual Loops"].as_f64().unwrap_or(1.0);
        total.1 += pages;
    }
    for child in plan["Plans"].as_array().into_iter().flatten() {
        add_reads(child, totals);
    }
}

/// 保持区分ごとの期限削除の文が、表ごとに読む行の数と、削除する表で触るページの数。削除は取り消す。
/// 索引の 2 列目だけで絞る全走査は、条件に合わない項目を索引の中で読み飛ばすので行の数に出ない。
/// そのため削除する表はページの数も比べる。突き合わせる表のページの数は、計画の形で変わるので比べない。
async fn reads_by_expired_deletes(
    pool: &PgPool,
    now: DateTime<Utc>,
) -> Result<BTreeMap<(&'static str, String), (f64, f64)>> {
    sqlx::query("ANALYZE").execute(pool).await?;
    let mut reads = BTreeMap::new();
    for (categories, sql) in EXPIRED_DELETES {
        for category in categories {
            let mut tx = pool.begin().await?;
            sqlx::query("SELECT set_config('kukuri.retention_cleanup', 'on', true)")
                .execute(&mut *tx)
                .await?;
            let explain: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}"
            )))
            .bind(now)
            .bind(category)
            .fetch_one(&mut *tx)
            .await?;
            tx.rollback().await?;
            let plan = &explain[0]["Plan"];
            let mut totals = BTreeMap::new();
            for child in plan["Plans"].as_array().into_iter().flatten() {
                add_reads(child, &mut totals);
            }
            for (relation, (rows, pages)) in totals {
                let pages = if plan["Relation Name"] == relation.as_str() {
                    pages
                } else {
                    0.0
                };
                reads.insert((*category, relation), (rows, pages));
            }
        }
    }
    Ok(reads)
}

#[tokio::test]
async fn retention_runs_do_not_rewrite_rows() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping case retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_case_retention_rows").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let now = Utc::now();
        seed(&pool, "live", now - Duration::days(1), 2).await?;
        let before = row_versions(&pool).await?;
        assert_eq!(before.len(), 2 * (1 + 1 + 4 + 3 + 3 + 1 + 1 + 1));

        // 起動時（日数の書込みと期限削除）と毎時（期限削除）の処理は、期限内の行を書き直さない。
        configure_case_retention(&pool, &RetentionPolicy::default()).await?;
        cleanup_expired(&pool, now).await?;
        cleanup_expired(&pool, now).await?;
        assert_eq!(row_versions(&pool).await?, before);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn every_category_follows_the_configured_days() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping case retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_case_retention_days").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let now = Utc::now().trunc_subsecs(6);
        seed(&pool, "case", now - Duration::days(10), 1).await?;

        // 20 日なら期限内。通常読取りが返し、削除しない。
        configure_case_retention(&pool, &every_category(20)).await?;
        assert_eq!(visible(&pool, "case").await?, vec![true; 8]);
        assert_eq!(cleanup_expired(&pool, now).await?, CleanupCounts::default());
        // 5 日に縮めると、既存の行も期限切れになり、通常読取りが返さない。
        configure_case_retention(&pool, &every_category(5)).await?;
        assert_eq!(visible(&pool, "case").await?, vec![false; 8]);
        // 20 日に戻すと、消えていない行は再び読める。
        configure_case_retention(&pool, &every_category(20)).await?;
        assert_eq!(visible(&pool, "case").await?, vec![true; 8]);
        // 10 日なら、起算点＋10 日の直前は期限内で、ちょうどで期限切れになる。
        configure_case_retention(&pool, &every_category(10)).await?;
        assert_eq!(
            cleanup_expired(&pool, now - Duration::microseconds(1)).await?,
            CleanupCounts::default()
        );
        assert_eq!(cleanup_expired(&pool, now).await?, ONE_EACH);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn report_hold_export_reports_expiry_from_start_and_days() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping case retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_case_retention_export").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let start = Utc::now().trunc_subsecs(6) - Duration::days(10);
        seed(&pool, "held", start, 1).await?;
        let hold = start_legal_hold(
            &pool,
            "report",
            "report-held-1",
            &["report".to_string()],
            "court preservation order",
            "final disposition",
            "legal@node.example",
            Utc::now(),
        )
        .await?;
        let cipher =
            LegalDataCipher::from_key_material("unit-test-legal-data-key-0123456789abcdef")?;
        let export = export_legal_hold(
            &pool,
            &cipher,
            &hold.id,
            "reviewer@node.example",
            Utc::now(),
        )
        .await?;
        assert_eq!(
            serde_json::from_value::<DateTime<Utc>>(export.data["report"]["expires_at"].clone())?,
            start + Duration::days(i64::from(RetentionPolicy::default().report_days))
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn expired_deletes_read_the_same_rows_however_many_live_rows() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping case retention test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_case_retention_reads").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let now = Utc::now();
        seed(&pool, "expired", now - Duration::days(1_000), 3).await?;
        seed(&pool, "small", now - Duration::days(1), 2_000).await?;
        let small = reads_by_expired_deletes(&pool, now).await?;
        seed(&pool, "large", now - Duration::days(1), 18_000).await?;
        let large = reads_by_expired_deletes(&pool, now).await?;
        // 期限内の行が 2,000 行でも 20,000 行でも、どの区分の期限削除も読む行と触るページは同じ。
        assert_eq!(small, large);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

#[tokio::test]
async fn migration_keeps_the_expiry_written_by_the_old_policy() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping case retention migration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_case_retention_migration").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        // 期限の列がある版。tester feedback と機微区分の期限の列には既定が無いので、入れられるようにする。
        migrate_postgres_up_to(&pool, 202_609_260_003).await?;
        sqlx::raw_sql(
            "ALTER TABLE cn_admin.tester_feedback ALTER COLUMN expires_at SET DEFAULT NOW();
             ALTER TABLE cn_legal.sensitive_items ALTER COLUMN expires_at SET DEFAULT NOW();",
        )
        .execute(&pool)
        .await?;
        let now = Utc::now();
        seed(&pool, "expired", now - Duration::days(1_000), 1).await?;
        seed(&pool, "live", now - Duration::days(1), 1).await?;
        // 旧版の起動時の書き直し（既定の日数）と同じ式で期限を書く。
        sqlx::raw_sql(
            "BEGIN;
             SELECT set_config('kukuri.retention_reconcile', 'on', true);
             UPDATE cn_admin.reports SET expires_at = created_at + INTERVAL '180 days';
             UPDATE cn_admin.tester_feedback SET expires_at = created_at + INTERVAL '180 days';
             UPDATE cn_legal.sensitive_items SET expires_at = created_at + make_interval(
                 days => CASE data_category WHEN 'report_contact' THEN 90 ELSE 180 END);
             UPDATE cn_legal.rights_requests SET expires_at = updated_at + make_interval(
                 days => CASE WHEN status = 'actioned' THEN 365
                              WHEN status IN ('declined', 'out_of_scope', 'withdrawn') THEN 180
                              ELSE 730 END);
             UPDATE cn_legal.rights_request_events
                 SET expires_at = occurred_at + INTERVAL '365 days';
             UPDATE cn_admin.operator_actions SET expires_at = occurred_at + INTERVAL '365 days';
             UPDATE cn_safety.signed_moderation_events
                 SET retention_expires_at = persisted_at + INTERVAL '180 days';
             UPDATE cn_safety.risk_signals
                 SET retention_expires_at = persisted_at + INTERVAL '180 days';
             COMMIT;",
        )
        .execute(&pool)
        .await?;
        let expired: (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT
               (SELECT COUNT(*) FROM cn_legal.sensitive_items WHERE expires_at <= $1),
               (SELECT COUNT(*) FROM cn_legal.rights_request_events WHERE expires_at <= $1),
               (SELECT COUNT(*) FROM cn_admin.reports WHERE expires_at <= $1),
               (SELECT COUNT(*) FROM cn_admin.tester_feedback WHERE expires_at <= $1),
               (SELECT COUNT(*) FROM cn_legal.rights_requests WHERE expires_at <= $1),
               (SELECT COUNT(*) FROM cn_admin.operator_actions WHERE expires_at <= $1),
               (SELECT COUNT(*) FROM cn_safety.signed_moderation_events
                WHERE retention_expires_at <= $1),
               (SELECT COUNT(*) FROM cn_safety.risk_signals WHERE retention_expires_at <= $1)",
        )
        .bind(now)
        .fetch_one(&pool)
        .await?;
        let before = CleanupCounts {
            sensitive_items: expired.0 as u64,
            rights_request_events: expired.1 as u64,
            reports: expired.2 as u64,
            tester_feedback: expired.3 as u64,
            rights_requests: expired.4 as u64,
            operator_actions: expired.5 as u64,
            moderation_events: expired.6 as u64,
            risk_signals: expired.7 as u64,
        };
        assert_eq!(before, ONE_EACH);

        // 移行すると期限の列は無くなり、移行前に期限切れだった行だけが消える。
        initialize_database(&pool).await?;
        let columns: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM information_schema.columns
             WHERE (table_schema, table_name, column_name) IN (
               ('cn_admin', 'reports', 'expires_at'),
               ('cn_admin', 'tester_feedback', 'expires_at'),
               ('cn_legal', 'sensitive_items', 'expires_at'),
               ('cn_legal', 'rights_requests', 'expires_at'),
               ('cn_legal', 'rights_request_events', 'expires_at'),
               ('cn_admin', 'operator_actions', 'expires_at'),
               ('cn_safety', 'signed_moderation_events', 'retention_expires_at'),
               ('cn_safety', 'risk_signals', 'retention_expires_at'))",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(columns, 0);
        assert_eq!(cleanup_expired(&pool, now).await?, before);
        assert_eq!(visible(&pool, "live").await?, vec![true; 8]);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}
