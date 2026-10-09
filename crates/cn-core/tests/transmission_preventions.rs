use anyhow::Result;
use chrono::{Duration, Utc};
use kukuri_cn_core::{
    INDEXED_POST_EXISTS_SQL, IndexScopeKind, NewIndexEntry, NewTransmissionPrevention,
    REMOVE_PREVENTED_POST_INDEX_SQL, TestDatabase, TransmissionPreventionBasis,
    TransmissionPreventionCapability, apply_transmission_prevention, connect_postgres,
    filter_surfaceable_objects, get_active_transmission_prevention, get_index_entry,
    initialize_database, list_operator_actions, release_transmission_prevention,
    resolve_rights_request_scope, upsert_index_entry, upsert_scan_verdict,
};
use kukuri_cn_protocol::RightsRequestScopeStatus;
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{ReasonCode, SafetyAction, SafetyVerdict};
use kukuri_cn_safety_runtime::VerdictPersistMeta;
use sqlx::PgPool;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

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
        policy_version: "test".to_string(),
        scanned_at: "2026-08-25T00:00:00Z".to_string(),
    }
}

#[tokio::test]
async fn apply_survives_restart_gates_queries_and_requires_fresh_ingest_after_release() -> Result<()>
{
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping transmission-prevention integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_transmission_prevention").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let verdict = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-761",
            &allow_verdict(),
            &VerdictPersistMeta::default(),
        )
        .await?;
        let entry = NewIndexEntry {
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: "rust".to_string(),
            object_id: "post-761".to_string(),
            author_pubkey: "author".to_string(),
            created_at: 1,
            source_replica_id: "topic::rust".to_string(),
            verdict_id: verdict.id,
            verdict_action: "allow".to_string(),
            critical: false,
        };
        upsert_index_entry(&pool, &entry).await?;

        let applied = apply_transmission_prevention(
            &pool,
            "legal@node.example",
            &NewTransmissionPrevention {
                subject_kind: "post".to_string(),
                subject_id: "post-761".to_string(),
                basis: TransmissionPreventionBasis::Copyright,
                capabilities: vec![
                    TransmissionPreventionCapability::CommunityIndex,
                    TransmissionPreventionCapability::Search,
                    TransmissionPreventionCapability::Discovery,
                    TransmissionPreventionCapability::Recommendation,
                ],
                expires_at: None,
                related_report_id: None,
            },
        )
        .await?;
        assert_eq!(applied.removed_index_scopes.len(), 1);
        assert!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-761")
                .await?
                .is_none()
        );
        assert!(
            filter_surfaceable_objects(
                &pool,
                IndexScopeKind::PublicTopic,
                &[("rust".to_string(), "post-761".to_string())],
            )
            .await?
            .is_empty()
        );
        assert!(upsert_index_entry(&pool, &entry).await.is_err());
        drop(pool.clone());
        let restarted = connect_postgres(database.database_url.as_str()).await?;
        assert!(
            get_active_transmission_prevention(&restarted, "post", "post-761")
                .await?
                .is_some()
        );

        release_transmission_prevention(
            &restarted,
            "legal@node.example",
            "post",
            "post-761",
            "claim resolved",
        )
        .await?;
        assert!(
            get_active_transmission_prevention(&restarted, "post", "post-761")
                .await?
                .is_none()
        );
        assert!(
            get_index_entry(&restarted, IndexScopeKind::PublicTopic, "rust", "post-761")
                .await?
                .is_none(),
            "release must not resurrect stale index state"
        );
        upsert_index_entry(&restarted, &entry).await?;

        let expired_subject = NewTransmissionPrevention {
            subject_kind: "post".to_string(),
            subject_id: "post-expired-761".to_string(),
            basis: TransmissionPreventionBasis::Privacy,
            capabilities: vec![TransmissionPreventionCapability::Moderation],
            expires_at: Some(Utc::now() - Duration::minutes(1)),
            related_report_id: None,
        };
        apply_transmission_prevention(&restarted, "legal@node.example", &expired_subject).await?;
        assert!(
            get_active_transmission_prevention(&restarted, "post", "post-expired-761")
                .await?
                .is_none()
        );
        let renewed = NewTransmissionPrevention {
            expires_at: None,
            ..expired_subject
        };
        apply_transmission_prevention(&restarted, "legal@node.example", &renewed).await?;
        assert!(
            get_active_transmission_prevention(&restarted, "post", "post-expired-761")
                .await?
                .is_some()
        );

        let audit = list_operator_actions(&restarted, 10, 0).await?;
        assert!(
            audit
                .iter()
                .any(|row| row.action == "transmission_prevention.apply")
        );
        assert!(
            audit
                .iter()
                .any(|row| row.action == "transmission_prevention.release")
        );
        assert!(
            audit
                .iter()
                .any(|row| row.action == "transmission_prevention.expire")
        );
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// 索引の行を読んだ数（返した行と filter で捨てた行の和に loop 数を掛けたもの）。書込みの節点は数えない。
fn index_entry_rows_read(node: &serde_json::Value) -> f64 {
    let count = |key: &str| node[key].as_f64().unwrap_or(0.0);
    let own = if node["Relation Name"] == "index_entries" && node["Node Type"] != "ModifyTable" {
        (count("Actual Rows")
            + count("Rows Removed by Filter")
            + count("Rows Removed by Index Recheck"))
            * count("Actual Loops")
    } else {
        0.0
    };
    own + node["Plans"]
        .as_array()
        .into_iter()
        .flatten()
        .map(index_entry_rows_read)
        .sum::<f64>()
}

/// `sql` を投稿 `object_id` について実行して取り消し、索引の行を読んだ数を返す。
async fn explain_index_entry_reads(pool: &PgPool, sql: &str, object_id: &str) -> Result<f64> {
    let mut tx = pool.begin().await?;
    let plan = sqlx::query_scalar::<_, serde_json::Value>(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, FORMAT JSON) {sql}"
    )))
    .bind(object_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.rollback().await?;
    Ok(index_entry_rows_read(&plan[0]["Plan"]))
}

/// 他の投稿（各 1 scope、判定は投稿ごと）の索引の行を `from..to` だけ足す。trigger を通すので 500 行ずつ入れる。
async fn add_other_indexed_posts(pool: &PgPool, from: i64, to: i64) -> Result<()> {
    for start in (from..to).step_by(500) {
        let end = (start + 500).min(to);
        sqlx::query(
            "INSERT INTO cn_safety.scan_verdicts
                (id, subject_kind, subject_id, action, critical, reason_code, policy_version,
                 scanned_at)
             SELECT 'verdict-' || g, 'post', encode(sha256(convert_to('post-' || g, 'UTF8')), 'hex'),
                    'allow', false, 'no_known_match', 'test', '2026-10-09T00:00:00Z'
             FROM generate_series($1::bigint, $2::bigint - 1) AS g",
        )
        .bind(start)
        .bind(end)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO cn_index.index_entries
                (scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
                 verdict_id, verdict_action, critical)
             SELECT 'public_topic', 'topic-' || (g % 100),
                    encode(sha256(convert_to('post-' || g, 'UTF8')), 'hex'), 'author-' || (g % 1000),
                    g, 'replica', 'verdict-' || g, 'allow', false
             FROM generate_series($1::bigint, $2::bigint - 1) AS g",
        )
        .bind(start)
        .bind(end)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// #1708: 投稿 id だけで索引の真実源を引く 2 つの文は、他の投稿の行が増えても読む行が増えず、結果も変わらない。
#[tokio::test]
async fn post_id_index_lookups_do_not_read_other_posts() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping transmission-prevention integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_post_id_index_lookup").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        let indexed = "ab".repeat(32);
        let absent = "cd".repeat(32);
        let verdict = upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            &indexed,
            &allow_verdict(),
            &VerdictPersistMeta::default(),
        )
        .await?;
        for (scope_kind, scope_id) in [
            (IndexScopeKind::PublicTopic, "rust"),
            (IndexScopeKind::PrivateChannel, "channel-1708"),
        ] {
            upsert_index_entry(
                &pool,
                &NewIndexEntry {
                    scope_kind,
                    scope_id: scope_id.to_string(),
                    object_id: indexed.clone(),
                    author_pubkey: "author".to_string(),
                    created_at: 1,
                    source_replica_id: "replica".to_string(),
                    verdict_id: verdict.id.clone(),
                    verdict_action: "allow".to_string(),
                    critical: false,
                },
            )
            .await?;
        }

        let mut reads = Vec::new();
        let mut others = 0;
        for total in [2_000, 20_000] {
            add_other_indexed_posts(&pool, others, total).await?;
            others = total;
            sqlx::query("ANALYZE cn_index.index_entries")
                .execute(&pool)
                .await?;
            let mut read = Vec::new();
            for sql in [INDEXED_POST_EXISTS_SQL, REMOVE_PREVENTED_POST_INDEX_SQL] {
                for object_id in [&indexed, &absent] {
                    read.push(explain_index_entry_reads(&pool, sql, object_id).await?);
                }
            }
            reads.push(read);
        }
        assert_eq!(
            reads[0], reads[1],
            "index_entries rows read must not grow with other posts"
        );

        let index_only = vec!["community_index".to_string()];
        assert_eq!(
            resolve_rights_request_scope(&pool, "post", &indexed, &index_only, &index_only).await?,
            RightsRequestScopeStatus::VerifiedScope
        );
        assert_eq!(
            resolve_rights_request_scope(&pool, "post", &absent, &index_only, &index_only).await?,
            RightsRequestScopeStatus::UnverifiedScope
        );
        let applied = apply_transmission_prevention(
            &pool,
            "legal@node.example",
            &NewTransmissionPrevention {
                subject_kind: "post".to_string(),
                subject_id: indexed.clone(),
                basis: TransmissionPreventionBasis::Copyright,
                capabilities: vec![TransmissionPreventionCapability::CommunityIndex],
                expires_at: None,
                related_report_id: None,
            },
        )
        .await?;
        let mut removed = applied.removed_index_scopes;
        removed.sort_by(|left, right| left.1.cmp(&right.1));
        assert_eq!(
            removed,
            vec![
                (IndexScopeKind::PrivateChannel, "channel-1708".to_string()),
                (IndexScopeKind::PublicTopic, "rust".to_string()),
            ]
        );
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM cn_index.projection_evictions WHERE object_id = $1",
        )
        .bind(&indexed)
        .fetch_one(&pool)
        .await?;
        assert_eq!(queued, 2);
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM cn_index.index_entries")
            .fetch_one(&pool)
            .await?;
        assert_eq!(remaining, others);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}
