use anyhow::Result;
use chrono::{Duration, Utc};
use kukuri_cn_core::{
    AdminOperation, INDEXED_POST_EXISTS_SQL, IndexEntryStore, IndexScopeKind,
    NewCommunityNodeReport, NewIndexEntry, NewTransmissionPrevention, OperatorReportStatus,
    PgIndexEntryStore, REMOVE_PREVENTED_POST_INDEX_SQL, TestDatabase, TransmissionPreventionBasis,
    TransmissionPreventionCapability, apply_operator_action, apply_transmission_prevention,
    connect_postgres, filter_surfaceable_objects, get_active_transmission_prevention,
    get_community_node_report, get_index_entry, initialize_database, insert_community_node_report,
    list_community_node_reports, list_malware_restrictions, list_operator_actions,
    release_transmission_prevention, resolve_rights_request_scope, upsert_index_entry,
    upsert_scan_verdict,
};
use kukuri_cn_protocol::RightsRequestScopeStatus;
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{ReasonCode, SafetyAction, SafetyVerdict};
use kukuri_cn_safety_runtime::VerdictPersistMeta;
use sha2::Digest;
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

/// 投稿 `post_id` への malware の通報を匿名で受け付け、受付参照 ID を返す（#1062）。
async fn malware_report(pool: &PgPool, subject_kind: &str, post_id: &str) -> Result<String> {
    Ok(insert_community_node_report(
        pool,
        &NewCommunityNodeReport {
            subject_kind: subject_kind.to_string(),
            subject_id: post_id.to_string(),
            capability: "community_index".to_string(),
            reason: "malware".to_string(),
            ..NewCommunityNodeReport::default()
        },
        None,
    )
    .await?
    .id)
}

fn restrict(report_id: &str) -> AdminOperation {
    AdminOperation::RestrictMalwarePost {
        report_id: report_id.to_string(),
    }
}

fn release(post_id: &str) -> AdminOperation {
    AdminOperation::ReleaseMalwareRestriction {
        post_id: post_id.to_string(),
        reason: "cleared after review".to_string(),
    }
}

/// `topic` に索引された allow の投稿を作る。
async fn indexed_post(pool: &PgPool, topic: &str, post_id: &str) -> Result<NewIndexEntry> {
    let verdict = upsert_scan_verdict(
        pool,
        SubjectKind::Post,
        post_id,
        &allow_verdict(),
        &VerdictPersistMeta::default(),
    )
    .await?;
    let entry = NewIndexEntry {
        scope_kind: IndexScopeKind::PublicTopic,
        scope_id: topic.to_string(),
        object_id: post_id.to_string(),
        author_pubkey: "author".to_string(),
        created_at: 1,
        source_replica_id: format!("topic::{topic}"),
        verdict_id: verdict.id,
        verdict_action: "allow".to_string(),
        critical: false,
    };
    upsert_index_entry(pool, &entry).await?;
    Ok(entry)
}

async fn surfaceable(pool: &PgPool, topic: &str, posts: &[&str]) -> Result<Vec<String>> {
    let candidates = posts
        .iter()
        .map(|post| (topic.to_string(), post.to_string()))
        .collect::<Vec<_>>();
    let mut posts = filter_surfaceable_objects(pool, IndexScopeKind::PublicTopic, &candidates)
        .await?
        .into_iter()
        .map(|entry| entry.object_id)
        .collect::<Vec<_>>();
    posts.sort();
    Ok(posts)
}

async fn report_status(pool: &PgPool, report_id: &str) -> Result<String> {
    Ok(get_community_node_report(pool, report_id)
        .await?
        .expect("report")
        .status)
}

/// 書込みの件数（判断・監査）。拒否された操作が何も書かないことを確かめる。
async fn writes(pool: &PgPool) -> Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM cn_legal.transmission_preventions),
                (SELECT COUNT(*) FROM cn_admin.operator_actions)",
    )
    .fetch_one(pool)
    .await?)
}

/// #1062 AC-1: malware の通報 → 判断待ち → 明示の制限 → 解除が、当該 CN の索引の候補の読取りと取込に効き、
/// 通報の状態・判断・監査が一致する。権利侵害の判断・他の投稿を巻き込まない。
#[tokio::test]
async fn malware_report_restricts_and_releases_only_its_post() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping malware restriction integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_malware_restriction").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        sqlx::query(
            "INSERT INTO cn_index.supported_topics (id, kind) VALUES ('rust', 'public_topic')",
        )
        .execute(&pool)
        .await?;
        let entry = indexed_post(&pool, "rust", "post-malware").await?;
        indexed_post(&pool, "rust", "post-other").await?;
        let posts = ["post-malware", "post-other"];
        let report = malware_report(&pool, "post", "post-malware").await?;
        let second = malware_report(&pool, "post", "post-malware").await?;

        // 受付だけでは制限しない。判断待ちにしても制限しない。汎用の状態変更で対応済みにできない。
        assert_eq!(surfaceable(&pool, "rust", &posts).await?, posts);
        apply_operator_action(
            &pool,
            "ops@node.example",
            &AdminOperation::SetReportStatus {
                report_id: report.clone(),
                status: OperatorReportStatus::Reviewing,
            },
        )
        .await?;
        let before = writes(&pool).await?;
        assert!(
            apply_operator_action(
                &pool,
                "ops@node.example",
                &AdminOperation::SetReportStatus {
                    report_id: report.clone(),
                    status: OperatorReportStatus::Actioned,
                },
            )
            .await
            .is_err()
        );
        assert_eq!(report_status(&pool, &report).await?, "reviewing");

        // malware 以外・投稿以外・期限切れの通報からは判断を書かない。
        let spam = insert_community_node_report(
            &pool,
            &NewCommunityNodeReport {
                subject_kind: "post".to_string(),
                subject_id: "post-malware".to_string(),
                capability: "community_index".to_string(),
                reason: "spam".to_string(),
                ..NewCommunityNodeReport::default()
            },
            None,
        )
        .await?
        .id;
        let media = malware_report(&pool, "media", "blob-hash").await?;
        let expired = malware_report(&pool, "post", "post-malware").await?;
        sqlx::query(
            "UPDATE cn_admin.reports SET created_at = NOW() - INTERVAL '1000 days' WHERE id = $1",
        )
        .bind(&expired)
        .execute(&pool)
        .await?;
        for rejected in [&spam, &media, &expired] {
            assert!(
                apply_operator_action(&pool, "ops@node.example", &restrict(rejected))
                    .await
                    .is_err()
            );
        }
        assert!(
            apply_operator_action(&pool, " ", &restrict(&report))
                .await
                .is_err()
        );
        assert!(
            apply_transmission_prevention(
                &pool,
                "ops@node.example",
                &NewTransmissionPrevention {
                    subject_kind: "post".to_string(),
                    subject_id: "post-malware".to_string(),
                    basis: TransmissionPreventionBasis::Malware,
                    capabilities: vec![TransmissionPreventionCapability::Search],
                    expires_at: None,
                    related_report_id: Some(report.clone()),
                },
            )
            .await
            .is_err(),
            "a malware decision is made only from its report"
        );
        assert_eq!(writes(&pool).await?, before);

        // 明示の制限。通報・判断・監査は同じ取引で書かれ、索引の行は残る。
        let audit = apply_operator_action(&pool, "ops@node.example", &restrict(&report)).await?;
        assert_eq!(
            (
                audit.action.as_str(),
                audit.target_kind.as_str(),
                audit.target_id.as_str()
            ),
            ("malware.restrict_post", "post", "post-malware")
        );
        assert_eq!(report_status(&pool, &report).await?, "actioned");
        let decision = get_active_transmission_prevention(&pool, "post", "post-malware")
            .await?
            .expect("active malware decision");
        assert_eq!(decision.basis, TransmissionPreventionBasis::Malware);
        assert_eq!(decision.related_report_id.as_deref(), Some(report.as_str()));
        assert_eq!(decision.decided_by, "ops@node.example");
        assert_eq!(audit.after["decision_id"], decision.id);
        assert_eq!(surfaceable(&pool, "rust", &posts).await?, ["post-other"]);
        assert!(
            get_index_entry(&pool, IndexScopeKind::PublicTopic, "rust", "post-malware")
                .await?
                .is_some()
        );
        // 取込・再取込は判断を照合する。clean な再 scan（内容 cache の hit を含む）は判断を消さない。
        let store = PgIndexEntryStore::new(pool.clone());
        assert!(store.is_transmission_prevented("post-malware").await?);
        assert!(!store.is_transmission_prevented("post-other").await?);
        assert!(upsert_index_entry(&pool, &entry).await.is_err());
        indexed_post(&pool, "rust", "post-other").await?;
        upsert_scan_verdict(
            &pool,
            SubjectKind::Post,
            "post-malware",
            &allow_verdict(),
            &VerdictPersistMeta::default(),
        )
        .await?;
        assert_eq!(surfaceable(&pool, "rust", &posts).await?, ["post-other"]);

        // 再送と同じ投稿への別の通報は、有効な判断を使い、判断を増やさない。対応済みの通報は戻せない。
        let resent = apply_operator_action(&pool, "ops@node.example", &restrict(&report)).await?;
        let other = apply_operator_action(&pool, "ops@node.example", &restrict(&second)).await?;
        assert_eq!(resent.after["decision_id"], decision.id);
        assert_eq!(other.after["decision_id"], decision.id);
        assert_eq!(report_status(&pool, &second).await?, "actioned");
        assert!(
            apply_operator_action(
                &pool,
                "ops@node.example",
                &AdminOperation::SetReportStatus {
                    report_id: report.clone(),
                    status: OperatorReportStatus::Dismissed,
                },
            )
            .await
            .is_err()
        );

        // 再起動後も判断は残る。権利侵害の解除の経路は malware の判断を解除しない。
        let restarted = connect_postgres(database.database_url.as_str()).await?;
        assert!(
            release_transmission_prevention(
                &restarted,
                "legal@node.example",
                "post",
                "post-malware",
                "x"
            )
            .await
            .is_err()
        );
        assert_eq!(
            get_active_transmission_prevention(&restarted, "post", "post-malware")
                .await?
                .map(|value| value.id),
            Some(decision.id.clone())
        );

        // 解除は対象の判断だけを失効させ、索引に残った投稿はすぐに戻る。通報の状態は変えない。
        let released =
            apply_operator_action(&restarted, "ops@node.example", &release("post-malware")).await?;
        assert_eq!(released.action, "malware.release_post");
        assert_eq!(released.after["release_reason"], "cleared after review");
        assert!(
            get_active_transmission_prevention(&restarted, "post", "post-malware")
                .await?
                .is_none()
        );
        assert_eq!(surfaceable(&restarted, "rust", &posts).await?, posts);
        assert_eq!(report_status(&restarted, &report).await?, "actioned");
        assert!(
            apply_operator_action(&restarted, "ops@node.example", &release("post-malware"))
                .await
                .is_err()
        );
        let listed = list_malware_restrictions(&restarted, 50).await?;
        assert_eq!(listed.len(), 1);
        assert!(listed[0].released_at.is_some());

        // 権利侵害の判断と併存し、どちらの解除も他方を残す。
        let renewed = malware_report(&restarted, "post", "post-malware").await?;
        apply_operator_action(&restarted, "ops@node.example", &restrict(&renewed)).await?;
        let copyright = NewTransmissionPrevention {
            subject_kind: "post".to_string(),
            subject_id: "post-malware".to_string(),
            basis: TransmissionPreventionBasis::Copyright,
            capabilities: vec![TransmissionPreventionCapability::Search],
            expires_at: None,
            related_report_id: None,
        };
        apply_transmission_prevention(&restarted, "legal@node.example", &copyright).await?;
        release_transmission_prevention(
            &restarted,
            "legal@node.example",
            "post",
            "post-malware",
            "x",
        )
        .await?;
        assert!(store.is_transmission_prevented("post-malware").await?);
        apply_transmission_prevention(&restarted, "legal@node.example", &copyright).await?;
        apply_operator_action(&restarted, "ops@node.example", &release("post-malware")).await?;
        assert_eq!(
            get_active_transmission_prevention(&restarted, "post", "post-malware")
                .await?
                .map(|value| value.basis),
            Some(TransmissionPreventionBasis::Copyright)
        );
        assert!(
            surfaceable(&restarted, "rust", &posts)
                .await?
                .iter()
                .all(|post| post != "post-malware")
        );

        // 同じ投稿への同時の制限は、判断を 1 つだけ作る。
        let (left, right) = (
            malware_report(&restarted, "post", "post-concurrent").await?,
            malware_report(&restarted, "post", "post-concurrent").await?,
        );
        let (left, right) = (restrict(&left), restrict(&right));
        let (first, second) = tokio::join!(
            apply_operator_action(&restarted, "ops@node.example", &left),
            apply_operator_action(&restarted, "ops@node.example", &right),
        );
        assert_eq!(first?.after["decision_id"], second?.after["decision_id"]);
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cn_legal.transmission_preventions
             WHERE subject_id = 'post-concurrent' AND released_at IS NULL",
        )
        .fetch_one(&restarted)
        .await?;
        assert_eq!(active, 1);
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}

/// 表ごとの (順次走査の回数, 順次走査で読んだ行, 索引から読んだ行)。`pool` は接続 1 本で、直前の操作の
/// 統計をこの接続から書き出してから読む。
async fn table_reads(pool: &PgPool) -> Result<Vec<(String, i64, i64, i64)>> {
    sqlx::query("SELECT pg_stat_force_next_flush()")
        .execute(pool)
        .await?;
    Ok(sqlx::query_as(
        "SELECT relname::text, COALESCE(seq_scan, 0), COALESCE(seq_tup_read, 0),
                COALESCE(idx_tup_fetch, 0)
         FROM pg_stat_user_tables
         WHERE schemaname IN ('cn_admin', 'cn_legal', 'cn_index', 'cn_safety')
         ORDER BY schemaname, relname",
    )
    .fetch_all(pool)
    .await?)
}

/// `before` から `after` への増分のうち、増えた表だけ。
fn read_delta(
    before: &[(String, i64, i64, i64)],
    after: &[(String, i64, i64, i64)],
) -> Vec<(String, i64, i64, i64)> {
    after
        .iter()
        .zip(before)
        .map(|(a, b)| (a.0.clone(), a.1 - b.1, a.2 - b.2, a.3 - b.3))
        .filter(|row| row.1 + row.2 + row.3 > 0)
        .collect()
}

/// 無関係な通報・malware と権利侵害の判断（半分は解除済み）を `from..to` だけ足す。
async fn add_unrelated_reviews(pool: &PgPool, from: i64, to: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_admin.reports (id, subject_kind, subject_id, capability, reason, status, created_at)
         SELECT 'unrelated-' || g, 'post', 'unrelated-post-' || g, 'community_index',
                CASE WHEN g % 2 = 0 THEN 'malware' ELSE 'spam' END, 'received',
                NOW() - INTERVAL '1 day'
         FROM generate_series($1::bigint, $2::bigint - 1) AS g",
    )
    .bind(from)
    .bind(to)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO cn_legal.transmission_preventions
             (id, subject_kind, subject_id, basis_category, capabilities, decided_by, decided_at,
              related_report_id, released_at, released_by, release_reason)
         SELECT 'unrelated-' || g, 'post', 'unrelated-post-' || g,
                CASE WHEN g % 3 = 0 THEN 'copyright' ELSE 'malware' END, ARRAY['search'], 'ops',
                NOW() - INTERVAL '1 day', 'unrelated-' || g,
                CASE WHEN g % 2 = 0 THEN NOW() END, CASE WHEN g % 2 = 0 THEN 'ops' END,
                CASE WHEN g % 2 = 0 THEN 'cleared' END
         FROM generate_series($1::bigint, $2::bigint - 1) AS g",
    )
    .bind(from)
    .bind(to)
    .execute(pool)
    .await?;
    Ok(())
}

/// #1062 AC-1: 運営画面の 50 件の窓、100 件の候補の読取り、1 件の制限と解除は、無関係な通報・判断・索引の行が
/// 2 万から 6 万に増えても、増やした表を順次走査せず、それぞれ窓・候補・対象の数で決まる上限までしか読まない。
#[tokio::test]
async fn malware_review_reads_do_not_grow_with_unrelated_rows() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping malware restriction integration test");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_malware_restriction_reads").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        sqlx::query(
            "INSERT INTO cn_index.supported_topics (id, kind)
             SELECT 'topic-' || g, 'public_topic' FROM generate_series(0, 99) AS g",
        )
        .execute(&pool)
        .await?;
        // 統計を同じ接続から読むため、測る操作は接続 1 本で行う。
        let single = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(database.database_url.as_str())
            .await?;
        let mut window = (0..99)
            .map(|g| {
                (
                    format!("topic-{}", g % 100),
                    hex::encode(sha2::Sha256::digest(format!("post-{g}"))),
                )
            })
            .collect::<Vec<_>>();
        window.push(("topic-0".to_string(), String::new()));
        let grown = [
            "reports",
            "transmission_preventions",
            "index_entries",
            "scan_verdicts",
        ];
        // 手順ごとに、増やした表の 1 つから読んでよい行の上限（窓 50 件、候補 100 件、対象 1 件）。
        let bounds = [50, 2, 2 * window.len() as i64, 2];
        let mut others = 0;
        for total in [20_000, 60_000] {
            add_other_indexed_posts(&pool, others, total).await?;
            add_unrelated_reviews(&pool, others, total).await?;
            others = total;
            sqlx::query("ANALYZE").execute(&pool).await?;
            let post = format!("measured-post-{total}");
            indexed_post(&pool, "topic-0", &post).await?;
            let report = malware_report(&pool, "post", &post).await?;
            window.last_mut().expect("measured candidate").1 = post.clone();

            for (step, bound) in bounds.into_iter().enumerate() {
                let before = table_reads(&single).await?;
                match step {
                    0 => {
                        list_community_node_reports(&single, 50, 0).await?;
                        list_malware_restrictions(&single, 50).await?;
                    }
                    1 => {
                        apply_operator_action(&single, "ops@node.example", &restrict(&report))
                            .await?;
                    }
                    2 => {
                        let surfaced = filter_surfaceable_objects(
                            &single,
                            IndexScopeKind::PublicTopic,
                            &window,
                        )
                        .await?;
                        assert_eq!(surfaced.len(), window.len() - 1);
                        assert!(surfaced.iter().all(|entry| entry.object_id != post));
                    }
                    _ => {
                        apply_operator_action(&single, "ops@node.example", &release(&post)).await?;
                    }
                }
                let delta = read_delta(&before, &table_reads(&single).await?);
                assert!(
                    delta
                        .iter()
                        .filter(|row| grown.contains(&row.0.as_str()))
                        .all(|row| row.1 == 0 && row.3 <= bound),
                    "step {step} with {total} unrelated rows read beyond its bound: {delta:?}"
                );
            }
        }
        anyhow::Ok(())
    }
    .await;
    database.cleanup().await?;
    result
}
