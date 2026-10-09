//! 対象ごとの信頼値の集計と basis のページ（ADR 0026 §10、#1702 AC-1）の Postgres integration テスト。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - `aggregates_follow_random_writes_like_full_recomputation`（AC-1 (a)）: 乱数の書込みの列の各手順の
//!   後で、集計とページが全行から求めた参照と一致する。
//! - `sql_units_match_signal_contribution`: 集計の寄与（SQL）が cn-trust の `signal_contribution` と一致する。
//! - `expired_signal_is_excluded_from_trust_inputs` / `invalid_expires_at_signal_is_ignored_and_read_succeeds`
//! - `sweep_removes_expired_rows_in_bounded_batches` / `half_life_change_is_rebuilt_in_bounded_batches`（AC-1 (c)）
//! - `trust_reads_do_not_scale_with_row_counts`（AC-1 (b)）
//! - `content_writes_and_attributions_wait_for_each_other`: 同じ内容の risk signal の書込みと著者の関連付けが
//!   重なっても、集計が全行から求めた参照と一致する。

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Duration, SubsecRound, Utc};
use serde::de::DeserializeOwned;
use sqlx::{PgPool, Row};

use kukuri_cn_core::{
    RetentionPolicy, RiskSignalCorrection, RiskSignalMetadataEdit, StoredRiskSignal,
    TRUST_BASIS_PAGE_SIZE, TRUST_BASIS_PAGE_SQL, TRUST_REBUILD_BATCH, TRUST_SWEEP_BATCH,
    TRUST_TOTALS_SQL, TestDatabase, TrustBasisCursor, apply_retention_policy,
    attribute_risk_signal_subject_author, cleanup_expired, connect_postgres, dispute_risk_signal,
    edit_risk_signal_detection_metadata, initialize_database, list_disclosed_trust_basis_page,
    list_trust_basis_page, load_trust_totals, persist_risk_signal_with_author,
    rebuild_trust_totals, reissue_corrected_risk_signal, sweep_expired_trust_signals,
    sync_trust_half_life, trust_risk_input, update_risk_signal_appeal_status,
};
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};
use kukuri_cn_trust::{PullAudience, TrustComponentKind, TrustRiskInput, signal_contribution};

#[path = "support/lock_wait.rs"]
mod lock_wait;
use lock_wait::blocked_on_lock;

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const ISSUER: &str = "issuer-node";
const AUTHORS: [&str; 3] = ["author-a", "author-b", "author-c"];
const CONTENTS: [(RiskSignalTarget, &str); 5] = [
    (RiskSignalTarget::PostId, "post-1"),
    (RiskSignalTarget::PostId, "post-2"),
    (RiskSignalTarget::PostId, "post-3"),
    (RiskSignalTarget::BlobCid, "blob-1"),
    (RiskSignalTarget::BlobCid, "blob-2"),
];
const CATEGORIES: [SafetyCategory; 9] = [
    SafetyCategory::Csam,
    SafetyCategory::Cse,
    SafetyCategory::Grooming,
    SafetyCategory::Nsfw,
    SafetyCategory::Objectionable,
    SafetyCategory::Spam,
    SafetyCategory::Malware,
    SafetyCategory::Phishing,
    SafetyCategory::ProviderTest,
];
const SEVERITIES: [Severity; 4] = [
    Severity::Critical,
    Severity::High,
    Severity::Medium,
    Severity::Low,
];
const BASES: [Basis; 4] = [
    Basis::KnownHashMatch,
    Basis::ProviderVerdict,
    Basis::ClassifierScore,
    Basis::LocalPolicy,
];
const VISIBILITIES: [Visibility; 3] = [
    Visibility::Local,
    Visibility::SubscribedNodes,
    Visibility::Public,
];

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

async fn with_database<F, Fut>(prefix: &str, body: F) -> Result<()>
where
    F: FnOnce(PgPool) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping #1702 integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), prefix).await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    let result = async {
        initialize_database(&pool).await?;
        body(pool.clone()).await
    }
    .await;
    pool.close().await;
    database.cleanup().await?;
    result
}

/// 再現できる擬似乱数（xorshift64*）。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

#[allow(clippy::unwrap_used)] // test fixture helper
fn db_enum<T: DeserializeOwned>(value: &str) -> T {
    serde_json::from_value(serde_json::Value::String(value.to_string())).unwrap()
}

fn random_signal(rng: &mut Rng, target: RiskSignalTarget, target_id: &str) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target,
        target_id: target_id.to_string(),
        category: rng.pick(&CATEGORIES),
        severity: rng.pick(&SEVERITIES),
        basis: rng.pick(&BASES),
        confidence: [None, Some(0), Some(37), Some(84), Some(100)][rng.below(5)],
        visibility: rng.pick(&VISIBILITIES),
        expires_at: None,
        appeal_status: Some(AppealStatus::None),
    }
}

/// 参照: 対象の全行（利用者が対象の行と、著者の対応を通した内容の行）のうち、`now` に生きている行。
async fn live_inputs(
    pool: &PgPool,
    target: &str,
    now: DateTime<Utc>,
) -> Result<Vec<TrustRiskInput>> {
    let rows = sqlx::query(
        "SELECT s.* FROM cn_safety.risk_signals s
         WHERE (s.target = 'user_pubkey' AND s.target_id = $1)
            OR EXISTS (SELECT 1 FROM cn_safety.risk_signal_subject_authors a
                       WHERE a.author_pubkey = $1 AND a.target = s.target
                         AND a.target_id = s.target_id)",
    )
    .bind(target)
    .fetch_all(pool)
    .await?;
    let mut live = Vec::new();
    for row in rows {
        let retention: DateTime<Utc> = row.try_get("retention_expires_at")?;
        let expires_at: Option<String> = row.try_get("expires_at")?;
        let expired = retention <= now
            || expires_at.as_deref().is_some_and(|value| {
                DateTime::parse_from_rfc3339(value).map_or(true, |at| at <= now)
            });
        if expired {
            continue;
        }
        let confidence: Option<i16> = row.try_get("confidence")?;
        let appeal_status: Option<String> = row.try_get("appeal_status")?;
        let stored = StoredRiskSignal {
            id: row.try_get("id")?,
            issuer_node_id: row.try_get("issuer_node_id")?,
            signal: SafetyRiskSignal {
                target: db_enum(row.try_get("target")?),
                target_id: row.try_get("target_id")?,
                category: db_enum(row.try_get("category")?),
                severity: db_enum(row.try_get("severity")?),
                basis: db_enum(row.try_get("basis")?),
                confidence: confidence.map(|value| value as u8),
                visibility: db_enum(row.try_get("visibility")?),
                expires_at,
                appeal_status: appeal_status.as_deref().map(db_enum),
            },
            persisted_at: row.try_get("persisted_at")?,
            operator_adjusted_at: row.try_get("operator_adjusted_at")?,
            operator_origin_category: None,
        };
        live.push(trust_risk_input(&stored));
    }
    Ok(live)
}

fn decayed_sum(inputs: &[&TrustRiskInput], now: DateTime<Utc>, half_life_days: f64) -> f64 {
    inputs
        .iter()
        .map(|input| {
            let age_days = (now - input.persisted_at).as_seconds_f64().max(0.0) / 86_400.0;
            signal_contribution(input) * 0.5_f64.powf(age_days / half_life_days)
        })
        .sum()
}

fn disclosed(input: &TrustRiskInput, audience: PullAudience) -> bool {
    input.component == TrustComponentKind::Absolute
        && input.appeal_status != AppealStatus::Cleared
        && matches!(input.basis, Basis::KnownHashMatch | Basis::ProviderVerdict)
        && match input.visibility {
            Visibility::Local => false,
            Visibility::SubscribedNodes => audience == PullAudience::SubscribedNodes,
            Visibility::Public => true,
        }
}

fn newest_first_ids(mut inputs: Vec<&TrustRiskInput>) -> Vec<String> {
    inputs.sort_by(|left, right| {
        (right.component == TrustComponentKind::Absolute)
            .cmp(&(left.component == TrustComponentKind::Absolute))
            .then(right.persisted_at.cmp(&left.persisted_at))
            .then(right.signal_id.cmp(&left.signal_id))
    });
    inputs
        .into_iter()
        .map(|input| input.signal_id.clone())
        .collect()
}

async fn all_basis_ids(
    pool: &PgPool,
    target: &str,
    audience: Option<PullAudience>,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut cursor: Option<TrustBasisCursor> = None;
    loop {
        let page = match audience {
            None => list_trust_basis_page(pool, target, cursor.as_ref()).await?,
            Some(audience) => {
                list_disclosed_trust_basis_page(pool, target, audience, cursor.as_ref()).await?
            }
        };
        ids.extend(page.inputs.iter().map(|input| input.signal_id.clone()));
        match page.next_cursor {
            Some(next) => {
                assert_eq!(
                    page.inputs.len(),
                    TRUST_BASIS_PAGE_SIZE,
                    "only the last page is short"
                );
                cursor = Some(TrustBasisCursor::parse(&next).expect("server cursor parses"));
            }
            None => return Ok(ids),
        }
    }
}

/// 集計とページが、全行から求めた参照と一致すること。
async fn assert_matches_full_recomputation(
    pool: &PgPool,
    now: DateTime<Utc>,
    half_life_days: f64,
    context: &str,
) -> Result<()> {
    let targets: Vec<String> = AUTHORS.iter().map(|author| author.to_string()).collect();
    let totals = load_trust_totals(pool, &targets).await?;
    for target in &targets {
        let live = live_inputs(pool, target, now).await?;
        let counted = |component| {
            live.iter()
                .filter(move |input| {
                    input.component == component && input.appeal_status != AppealStatus::Cleared
                })
                .collect::<Vec<_>>()
        };
        let expected_absolute: f64 = counted(TrustComponentKind::Absolute)
            .iter()
            .map(|input| signal_contribution(input))
            .sum();
        let expected_relative =
            decayed_sum(&counted(TrustComponentKind::Relative), now, half_life_days);
        let actual = &totals[target];
        let label = format!("{context} / {target}");
        assert!(
            (actual.absolute() - expected_absolute.clamp(-1.0, 1.0)).abs() < 1e-9,
            "{label}: absolute {} vs {expected_absolute}",
            actual.absolute()
        );
        assert!(
            (actual.relative(now) - expected_relative.clamp(-1.0, 1.0)).abs() < 1e-9,
            "{label}: relative {} vs {expected_relative}",
            actual.relative(now)
        );
        for audience in [PullAudience::Public, PullAudience::SubscribedNodes] {
            let expected: f64 = live
                .iter()
                .filter(|input| disclosed(input, audience))
                .map(signal_contribution)
                .sum();
            assert!(
                (actual.disclosed_absolute(audience) - expected.clamp(-1.0, 1.0)).abs() < 1e-9,
                "{label}: disclosed {audience:?}"
            );
            let expected_ids = newest_first_ids(
                live.iter()
                    .filter(|input| disclosed(input, audience))
                    .collect(),
            );
            assert_eq!(
                all_basis_ids(pool, target, Some(audience)).await?,
                expected_ids,
                "{label}: disclosed basis {audience:?}"
            );
        }
        let live_ids: Vec<String> = live.iter().map(|input| input.signal_id.clone()).collect();
        let expected_digest: i64 = sqlx::query_scalar(
            "SELECT COALESCE(bit_xor(cn_safety.trust_signal_digest(
                 id, appeal_status, operator_adjusted_at, expires_at)), 0)
             FROM cn_safety.risk_signals WHERE id = ANY($1)",
        )
        .bind(&live_ids)
        .fetch_one(pool)
        .await?;
        assert_eq!(actual.digest, expected_digest, "{label}: digest");
        assert_eq!(
            all_basis_ids(pool, target, None).await?,
            newest_first_ids(live.iter().collect()),
            "{label}: basis pages"
        );
    }
    Ok(())
}

async fn random_signal_id(pool: &PgPool, rng: &mut Rng) -> Result<Option<String>> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM cn_safety.risk_signals ORDER BY id")
        .fetch_all(pool)
        .await?;
    Ok((!ids.is_empty()).then(|| ids[rng.below(ids.len())].clone()))
}

/// 同じ鍵の活性行が既にあるときの一意制約の失敗だけを読み飛ばす（trigger の失敗は試験を落とす）。
fn allow_active_key_conflict<T>(result: Result<T>) -> Result<()> {
    match result {
        Ok(_) => Ok(()),
        Err(error) if format!("{error:?}").contains("uq_cn_safety_risk_signals_active_key") => {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

async fn random_step(
    pool: &PgPool,
    rng: &mut Rng,
    now: &mut DateTime<Utc>,
    half_life: &mut f64,
) -> Result<String> {
    // 照会の時刻は、保存された行の時刻（DB の時計）より前にしない。Postgres に合わせてマイクロ秒に揃える。
    *now = (*now).max(Utc::now()).trunc_subsecs(6);
    let step = rng.below(12);
    match step {
        0 | 1 => {
            let author = rng.pick(&AUTHORS);
            let signal = random_signal(rng, RiskSignalTarget::UserPubkey, author);
            persist_risk_signal_with_author(pool, ISSUER, &signal, None).await?;
        }
        2 | 3 => {
            let (target, content) = rng.pick(&CONTENTS);
            let signal = random_signal(rng, target, content);
            persist_risk_signal_with_author(pool, ISSUER, &signal, Some(rng.pick(&AUTHORS)))
                .await?;
        }
        4 => {
            let (target, content) = rng.pick(&CONTENTS);
            attribute_risk_signal_subject_author(pool, target, content, rng.pick(&AUTHORS)).await?;
        }
        5 => {
            if let Some(id) = random_signal_id(pool, rng).await? {
                let status: Option<String> = sqlx::query_scalar(
                    "SELECT appeal_status FROM cn_safety.risk_signals WHERE id = $1",
                )
                .bind(&id)
                .fetch_one(pool)
                .await?;
                match status.as_deref().unwrap_or("none") {
                    "none" => {
                        dispute_risk_signal(pool, &id).await?;
                    }
                    "disputed" => {
                        let to = [AppealStatus::Cleared, AppealStatus::None][rng.below(2)];
                        update_risk_signal_appeal_status(pool, &id, to).await?;
                    }
                    _ => {}
                }
            }
        }
        6 => {
            if let Some(id) = random_signal_id(pool, rng).await? {
                let expires_at = [
                    None,
                    Some(*now + Duration::hours(1)),
                    Some(*now - Duration::hours(1)),
                ][rng.below(3)]
                .map(|at| at.to_rfc3339());
                let edit = RiskSignalMetadataEdit {
                    category: Some(rng.pick(&CATEGORIES)),
                    severity: Some(rng.pick(&SEVERITIES)),
                    confidence: Some([0, 50, 100][rng.below(3)]),
                    expires_at,
                };
                allow_active_key_conflict(
                    edit_risk_signal_detection_metadata(pool, &id, &edit, true).await,
                )?;
            }
        }
        7 => {
            if let Some(id) = random_signal_id(pool, rng).await? {
                let correction = RiskSignalCorrection {
                    category: Some(rng.pick(&CATEGORIES)),
                    severity: Some(rng.pick(&SEVERITIES)),
                    confidence: Some(90),
                    visibility: Some(rng.pick(&VISIBILITIES)),
                };
                allow_active_key_conflict(
                    reissue_corrected_risk_signal(pool, &id, &correction, &now.to_rfc3339(), true)
                        .await,
                )?;
            }
        }
        8 => {
            let policy = RetentionPolicy {
                risk_signal_days: [180, 30, 1][rng.below(3)],
                ..RetentionPolicy::default()
            };
            apply_retention_policy(pool, &policy).await?;
            cleanup_expired(pool, *now).await?;
        }
        9 => {
            *now += [
                Duration::minutes(1),
                Duration::hours(6),
                Duration::days(2),
                Duration::days(40),
            ][rng.below(4)];
        }
        10 => {
            let rows: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT target, target_id, author_pubkey FROM cn_safety.risk_signal_subject_authors
                 ORDER BY target, target_id, author_pubkey",
            )
            .fetch_all(pool)
            .await?;
            if !rows.is_empty() {
                let (target, target_id, author) = &rows[rng.below(rows.len())];
                sqlx::query(
                    "DELETE FROM cn_safety.risk_signal_subject_authors
                     WHERE target = $1 AND target_id = $2 AND author_pubkey = $3",
                )
                .bind(target)
                .bind(target_id)
                .bind(author)
                .execute(pool)
                .await?;
            }
        }
        _ => {
            *half_life = [30.0, 7.0, 45.0][rng.below(3)];
            sync_trust_half_life(pool, *half_life).await?;
            while rebuild_trust_totals(pool, *now).await? > 0 {}
        }
    }
    while sweep_expired_trust_signals(pool, *now).await? > 0 {}
    Ok(format!("step {step}"))
}

#[tokio::test]
async fn aggregates_follow_random_writes_like_full_recomputation() -> Result<()> {
    // 種ごとに別の DB で、照会の時刻を戻さずに進める。
    for seed in [0x1702_u64, 0xdead_beef] {
        with_database("cn_1702_trust_random", |pool| async move {
            // 1 ページ（50 件）を超える対象を作る（内容ごとに別の鍵になる）。
            for index in 0..60 {
                let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ index);
                let signal =
                    random_signal(&mut rng, RiskSignalTarget::PostId, &format!("bulk-{index}"));
                persist_risk_signal_with_author(&pool, ISSUER, &signal, Some(AUTHORS[0])).await?;
            }
            let mut rng = Rng(seed);
            let mut now = Utc::now().trunc_subsecs(6);
            let mut half_life = 30.0;
            assert_matches_full_recomputation(&pool, now, half_life, "start").await?;
            for index in 0..80 {
                let step = random_step(&pool, &mut rng, &mut now, &mut half_life).await?;
                assert_matches_full_recomputation(
                    &pool,
                    now,
                    half_life,
                    &format!("seed {seed:x} #{index} {step}"),
                )
                .await?;
            }
            Ok(())
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn content_writes_and_attributions_wait_for_each_other() -> Result<()> {
    with_database("cn_1702_trust_content_race", |pool| async move {
        let signal = SafetyRiskSignal {
            target: RiskSignalTarget::PostId,
            target_id: "post-1".to_string(),
            category: SafetyCategory::Spam,
            severity: Severity::High,
            basis: Basis::ProviderVerdict,
            confidence: Some(90),
            visibility: Visibility::Public,
            expires_at: None,
            appeal_status: Some(AppealStatus::None),
        };
        let stored =
            persist_risk_signal_with_author(&pool, ISSUER, &signal, Some(AUTHORS[0])).await?;

        // 審査で cleared にする取引の途中に、2 人目の著者を関連付ける。
        let mut review = pool.begin().await?;
        sqlx::query("UPDATE cn_safety.risk_signals SET appeal_status = 'cleared' WHERE id = $1")
            .bind(&stored.id)
            .execute(&mut *review)
            .await?;
        let attribution = tokio::spawn({
            let pool = pool.clone();
            async move {
                attribute_risk_signal_subject_author(
                    &pool,
                    RiskSignalTarget::PostId,
                    "post-1",
                    AUTHORS[1],
                )
                .await
            }
        });
        assert!(
            blocked_on_lock(&pool, &attribution).await?,
            "the attribution waits for the review"
        );
        review.commit().await?;
        attribution.await??;
        let now = Utc::now().trunc_subsecs(6);
        assert_matches_full_recomputation(&pool, now, 30.0, "cleared while attributing").await?;

        // 3 人目の著者を関連付ける取引（`insert_subject_author` と同じ lock）の途中に、同じ内容へ
        // 新しい行を保存する。
        let mut attributing = pool.begin().await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock_shared(
                hashtextextended('cn_safety.risk_signal_subject_authors', 0))",
        )
        .execute(&mut *attributing)
        .await?;
        sqlx::query(
            "INSERT INTO cn_safety.risk_signal_subject_authors (target, target_id, author_pubkey)
             VALUES ('post_id', 'post-1', $1)",
        )
        .bind(AUTHORS[2])
        .execute(&mut *attributing)
        .await?;
        let persisting = tokio::spawn({
            let pool = pool.clone();
            let signal = SafetyRiskSignal {
                category: SafetyCategory::Phishing,
                ..signal
            };
            async move {
                persist_risk_signal_with_author(&pool, ISSUER, &signal, Some(AUTHORS[0])).await
            }
        });
        assert!(
            blocked_on_lock(&pool, &persisting).await?,
            "the new risk signal waits for the attribution"
        );
        attributing.commit().await?;
        persisting.await??;
        let now = Utc::now().trunc_subsecs(6);
        assert_matches_full_recomputation(&pool, now, 30.0, "persisted while attributing").await
    })
    .await
}

#[tokio::test]
async fn sql_units_match_signal_contribution() -> Result<()> {
    with_database("cn_1702_trust_units", |pool| async move {
        for category in CATEGORIES {
            for severity in SEVERITIES {
                for confidence in [None, Some(0), Some(1), Some(37), Some(84), Some(100)] {
                    for appeal in [AppealStatus::None, AppealStatus::Disputed, AppealStatus::Cleared] {
                        let input = TrustRiskInput {
                            signal_id: "s".to_string(),
                            issuer_node_id: ISSUER.to_string(),
                            target: RiskSignalTarget::UserPubkey,
                            target_id: "t".to_string(),
                            component: kukuri_cn_trust::trust_component_for(category),
                            category,
                            severity,
                            basis: Basis::ClassifierScore,
                            confidence,
                            visibility: Visibility::Local,
                            appeal_status: appeal,
                            expires_at: None,
                            persisted_at: Utc::now(),
                            operator_adjusted_at: None,
                        };
                        let units: i32 = sqlx::query_scalar(
                            "SELECT cn_safety.trust_signal_units($1, $2, $3, $4)",
                        )
                        .bind(serde_json::to_value(category)?.as_str())
                        .bind(serde_json::to_value(severity)?.as_str())
                        .bind(confidence.map(i16::from))
                        .bind(serde_json::to_value(appeal)?.as_str())
                        .fetch_one(&pool)
                        .await?;
                        let expected = if appeal == AppealStatus::Cleared {
                            0.0
                        } else {
                            -signal_contribution(&input) * 1000.0
                        };
                        assert!(
                            (f64::from(units) - expected).abs() < 1e-9,
                            "{category:?} {severity:?} {confidence:?} {appeal:?}: {units} vs {expected}"
                        );
                    }
                }
            }
        }
        Ok(())
    })
    .await
}

/// 利用者が対象の行を、期限と保持期間を指定して直接入れる（検証を通らない値も入れるため）。
async fn insert_user_signal(
    pool: &PgPool,
    id: &str,
    target: &str,
    category: &str,
    expires_at: Option<&str>,
    persisted_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cn_safety.risk_signals
            (id, issuer_node_id, target, target_id, category, severity, basis, visibility,
             confidence, expires_at, appeal_status, persisted_at, retention_expires_at)
         VALUES ($1, $2, 'user_pubkey', $3, $4, 'high', 'classifier_score', 'local', 100, $5,
                 'none', $6, $6 + INTERVAL '180 days')",
    )
    .bind(id)
    .bind(ISSUER)
    .bind(target)
    .bind(category)
    .bind(expires_at)
    .bind(persisted_at)
    .execute(pool)
    .await?;
    Ok(())
}

async fn basis_ids(pool: &PgPool, target: &str) -> Result<Vec<String>> {
    all_basis_ids(pool, target, None).await
}

#[tokio::test]
async fn expired_signal_is_excluded_from_trust_inputs() -> Result<()> {
    with_database("cn_1702_trust_expired", |pool| async move {
        let now = Utc::now();
        let past = (now - Duration::days(1)).to_rfc3339();
        let future = (now + Duration::days(30)).to_rfc3339();
        insert_user_signal(&pool, "sig-expired", "x", "spam", Some(&past), now).await?;
        insert_user_signal(&pool, "sig-live", "x", "malware", Some(&future), now).await?;
        insert_user_signal(&pool, "sig-forever", "x", "phishing", None, now).await?;
        // 失効時刻を過ぎた行は集計にも basis にも入らない。期限内の行と無期限の行は入る。
        let mut ids = basis_ids(&pool, "x").await?;
        ids.sort();
        assert_eq!(ids, vec!["sig-forever", "sig-live"]);
        let totals = load_trust_totals(&pool, &["x".to_string()]).await?;
        assert!(
            (totals["x"].relative(now) - -1.0).abs() < 1e-12,
            "two live high signals"
        );

        // 期限が来た行は掃除の後に外れる。
        while sweep_expired_trust_signals(&pool, now + Duration::days(31)).await? > 0 {}
        assert_eq!(basis_ids(&pool, "x").await?, vec!["sig-forever"]);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn invalid_expires_at_signal_is_ignored_and_read_succeeds() -> Result<()> {
    with_database("cn_1702_trust_invalid_expiry", |pool| async move {
        // #700: 保存済みの不正な expires_at は移行せず「無視」する決定。該当判定だけを集計と basis から
        // 外し、読み取りは成功させ、他の判定には影響させない。
        let now = Utc::now();
        insert_user_signal(
            &pool,
            "sig-broken",
            "x",
            "csam",
            Some("not-a-timestamp"),
            now,
        )
        .await?;
        insert_user_signal(&pool, "sig-valid", "x", "csam", None, now).await?;
        assert_eq!(basis_ids(&pool, "x").await?, vec!["sig-valid"]);
        let totals = load_trust_totals(&pool, &["x".to_string()]).await?;
        assert!((totals["x"].absolute() - -0.7).abs() < 1e-12);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn sweep_removes_expired_rows_in_bounded_batches() -> Result<()> {
    with_database("cn_1702_trust_sweep", |pool| async move {
        // 1,500 件の内容の行を著者 x に付け、保持期間を 1 時間後にする。
        sqlx::query(
            "INSERT INTO cn_safety.risk_signal_subject_authors (target, target_id, author_pubkey)
             SELECT 'post_id', 'post-' || g, 'x' FROM generate_series(1, 1500) g",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO cn_safety.risk_signals
                (id, issuer_node_id, target, target_id, category, severity, basis, visibility,
                 confidence, appeal_status, retention_expires_at)
             SELECT 'sig-' || g, 'issuer', 'post_id', 'post-' || g, 'csam', 'low', 'known_hash_match',
                    'local', 1, 'none', NOW() + INTERVAL '1 hour'
             FROM generate_series(1, 1500) g",
        )
        .execute(&pool)
        .await?;
        let targets = ["x".to_string()];
        assert_eq!(load_trust_totals(&pool, &targets).await?["x"].absolute_units, 3_000);

        // 1 回は 1,000 行まで。止めても残りは次の回に消える。
        let later = Utc::now() + Duration::hours(2);
        assert_eq!(sweep_expired_trust_signals(&pool, later).await?, TRUST_SWEEP_BATCH as u64);
        assert_eq!(load_trust_totals(&pool, &targets).await?["x"].absolute_units, 1_000);
        assert_eq!(sweep_expired_trust_signals(&pool, later).await?, 500);
        assert_eq!(sweep_expired_trust_signals(&pool, later).await?, 0);
        // 0 行になった対象の集計の行は消える（読取りは成分 0 を返す）。
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cn_safety.trust_target_totals")
            .fetch_one(&pool)
            .await?;
        assert_eq!(left, 0);
        let empty = &load_trust_totals(&pool, &targets).await?["x"];
        assert_eq!(empty.absolute_units, 0);
        assert_eq!(empty.relative_units, 0.0);
        assert_eq!(empty.digest, 0);
        assert_eq!(empty.half_life_days, 30.0);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn half_life_change_is_rebuilt_in_bounded_batches() -> Result<()> {
    with_database("cn_1702_trust_rebuild", |pool| async move {
        let month_ago = (Utc::now() - Duration::days(30)).trunc_subsecs(6);
        for index in 0..150 {
            let (id, target) = (format!("sig-{index}"), format!("t-{index:03}"));
            insert_user_signal(&pool, &id, &target, "spam", None, month_ago).await?;
        }
        let targets: Vec<String> = (0..150).map(|index| format!("t-{index:03}")).collect();
        let now = Utc::now().trunc_subsecs(6);
        // high（0.7）を 30 日前に保存した対象の相対成分（半減期 h 日）。
        let expected = |half_life: f64| {
            -0.7 * 0.5_f64.powf((now - month_ago).as_seconds_f64() / 86_400.0 / half_life)
        };
        let before = load_trust_totals(&pool, &targets).await?;
        assert!((before["t-000"].relative(now) - expected(30.0)).abs() < 1e-9);

        sync_trust_half_life(&pool, 15.0).await?;
        // 作り直すまでは前の半減期の値を返す。
        let stale = load_trust_totals(&pool, &targets).await?;
        assert_eq!(stale["t-000"].half_life_days, 30.0);
        assert!((stale["t-000"].relative(now) - expected(30.0)).abs() < 1e-9);

        // 1 回 100 対象までで、残りは次の回に作り直す。
        assert_eq!(
            rebuild_trust_totals(&pool, now).await?,
            TRUST_REBUILD_BATCH as u64
        );
        assert_eq!(rebuild_trust_totals(&pool, now).await?, 50);
        assert_eq!(rebuild_trust_totals(&pool, now).await?, 0);
        let rebuilt = load_trust_totals(&pool, &targets).await?;
        for target in &targets {
            assert_eq!(rebuilt[target].half_life_days, 15.0, "{target}");
            assert!(
                (rebuilt[target].relative(now) - expected(15.0)).abs() < 1e-9,
                "{target}"
            );
        }
        // 作り直しの後に足した行も新しい半減期で入る。
        insert_user_signal(&pool, "sig-new", "t-new", "spam", None, month_ago).await?;
        let new = load_trust_totals(&pool, &["t-new".to_string()]).await?;
        assert_eq!(new["t-new"].half_life_days, 15.0);
        assert!((new["t-new"].relative(now) - expected(15.0)).abs() < 1e-9);
        Ok(())
    })
    .await
}

fn explain(sql: &str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(format!("EXPLAIN (ANALYZE, FORMAT JSON) {sql}"))
}

/// 実行計画（EXPLAIN ANALYZE）で、表から読んだ行（返した行と filter で捨てた行）の数を数える。
async fn rows_read(
    pool: &PgPool,
    query: sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments>,
) -> Result<HashMap<String, f64>> {
    let plan: serde_json::Value = query.fetch_one(pool).await?.try_get(0)?;
    let mut counts = HashMap::new();
    fn walk(node: &serde_json::Value, counts: &mut HashMap<String, f64>) {
        if let Some(relation) = node.get("Relation Name").and_then(|value| value.as_str()) {
            let number = |key: &str| {
                node.get(key)
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0)
            };
            *counts.entry(relation.to_string()).or_default() += (number("Actual Rows")
                + number("Rows Removed by Filter")
                + number("Rows Removed by Index Recheck"))
                * number("Actual Loops");
        }
        if let Some(children) = node.get("Plans").and_then(|value| value.as_array()) {
            for child in children {
                walk(child, counts);
            }
        }
    }
    walk(&plan[0]["Plan"], &mut counts);
    Ok(counts)
}

async fn seed_rows(pool: &PgPool, from: i32, to: i32) -> Result<()> {
    // 対象 x の行と、他の利用者の行を同じ数だけ足す（内容の行と著者の対応）。
    sqlx::query(
        "INSERT INTO cn_safety.risk_signal_subject_authors (target, target_id, author_pubkey)
         SELECT 'post_id', author || '-post-' || g, author
         FROM generate_series($1::int, $2::int) g, unnest(ARRAY['x', 'other-' || (g % 50)]) author",
    )
    .bind(from)
    .bind(to)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO cn_safety.risk_signals
            (id, issuer_node_id, target, target_id, category, severity, basis, visibility,
             confidence, appeal_status, persisted_at)
         SELECT target_id, 'issuer', 'post_id', target_id, 'spam', 'low', 'classifier_score',
                'local', 50, 'none', NOW() - make_interval(secs => g)
         FROM (SELECT author || '-post-' || g AS target_id, g
               FROM generate_series($1::int, $2::int) g,
                    unnest(ARRAY['x', 'other-' || (g % 50)]) author) rows",
    )
    .bind(from)
    .bind(to)
    .execute(pool)
    .await?;
    sqlx::query("ANALYZE cn_safety.trust_target_signals")
        .execute(pool)
        .await?;
    sqlx::query("ANALYZE cn_safety.trust_target_totals")
        .execute(pool)
        .await?;
    sqlx::query("ANALYZE cn_safety.risk_signals")
        .execute(pool)
        .await?;
    Ok(())
}

async fn read_counts(pool: &PgPool) -> Result<(HashMap<String, f64>, HashMap<String, f64>)> {
    let targets = vec!["x".to_string()];
    let totals = rows_read(pool, sqlx::query(explain(TRUST_TOTALS_SQL)).bind(targets)).await?;
    let page = rows_read(
        pool,
        sqlx::query(explain(TRUST_BASIS_PAGE_SQL))
            .bind("x")
            .bind(true)
            .bind(DateTime::<Utc>::MAX_UTC)
            .bind("")
            .bind(TRUST_BASIS_PAGE_SIZE as i64 + 1),
    )
    .await?;
    Ok((totals, page))
}

#[tokio::test]
async fn trust_reads_do_not_scale_with_row_counts() -> Result<()> {
    with_database("cn_1702_trust_scale", |pool| async move {
        seed_rows(&pool, 1, 2_000).await?;
        let small = read_counts(&pool).await?;
        // 対象自身の行も他の利用者の行も 10 倍にする。
        seed_rows(&pool, 2_001, 20_000).await?;
        let large = read_counts(&pool).await?;
        assert_eq!(small, large, "rows read must not grow with the row counts");
        // 集計の 1 行と、ページの 51 行（と、その risk signal の行）だけを読む。
        assert_eq!(large.0.get("trust_target_totals"), Some(&1.0));
        assert_eq!(
            large.1.get("trust_target_signals"),
            Some(&(TRUST_BASIS_PAGE_SIZE as f64 + 1.0))
        );
        Ok(())
    })
    .await
}
