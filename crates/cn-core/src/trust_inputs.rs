//! 信頼値の照会の読取り（ADR 0026 §10、#1702）。
//!
//! T は対象ごとの集計（`cn_safety.trust_target_totals`）から求め、basis は対象の生きている risk signal
//! （`cn_safety.trust_target_signals`）から 1 ページずつ読む。どちらも risk signal と著者の対応の変更を
//! migration の trigger が差分で反映する。照会は対象の行数にも総件数にもよらず、集計の 1 行と 1 ページ
//! （[`TRUST_BASIS_PAGE_SIZE`] + 1 行）だけを読む。期限を過ぎた行は [`sweep_expired_trust_signals`] が、
//! 運営者が変えた半減期は [`rebuild_trust_totals`] が背景で反映する。
//!
//! 断定ラベルにしない（ADR 0026 §2.5 / trust-semantics）: basis は必ず basis / confidence /
//! visibility / expiry / appeal を同伴する。`Cleared` は寄与 0 の説明用 basis として残す（§6.2）。

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use kukuri_cn_trust::{PullAudience, TrustRiskInput, TrustTotals, trust_component_for};

use crate::safety_events::{StoredRiskSignal, risk_signal_from_row};

/// basis の 1 ページの件数（ADR 0026 §10）。
pub const TRUST_BASIS_PAGE_SIZE: usize = 50;
/// 掃除が 1 回に消す行の上限。
pub const TRUST_SWEEP_BATCH: i64 = 1_000;
/// 半減期の作り直しが 1 回に扱う対象の上限。
pub const TRUST_REBUILD_BATCH: i64 = 100;

/// 対象ごとの集計を読む SQL（`$1` = 対象の pubkey の配列）。行の無い対象は成分 0 を返す。
pub const TRUST_TOTALS_SQL: &str = "SELECT k.target_pubkey,
        COALESCE(t.absolute_units, 0) AS absolute_units,
        COALESCE(t.relative_units, 0) AS relative_units,
        COALESCE(t.relative_at, now()) AS relative_at,
        COALESCE(t.half_life_days, s.relative_half_life_days) AS half_life_days,
        COALESCE(t.disclosed_public_units, 0) AS disclosed_public_units,
        COALESCE(t.disclosed_subscribed_units, 0) AS disclosed_subscribed_units,
        COALESCE(t.digest, 0) AS digest
    FROM unnest($1::text[]) AS k (target_pubkey)
    CROSS JOIN cn_safety.trust_settings s
    LEFT JOIN cn_safety.trust_target_totals t ON t.target_pubkey = k.target_pubkey";

/// basis の 1 ページを読む SQL（絶対成分が先、各成分は新しい順）。`$2`〜`$4` は前のページの最後の行。
pub const TRUST_BASIS_PAGE_SQL: &str = "SELECT s.*
    FROM cn_safety.trust_target_signals e
    JOIN cn_safety.risk_signals s ON s.id = e.signal_id
    WHERE e.target_pubkey = $1
      AND (e.absolute, e.persisted_at, e.signal_id) < ($2, $3, $4)
    ORDER BY e.absolute DESC, e.persisted_at DESC, e.signal_id DESC
    LIMIT $5";

/// 期限を過ぎた行を消す SQL（`$1` = 基準時刻、`$2` = 1 回の上限）。選んだ行は `ctid` の配列で引いて消す。
/// 選んだ行と結合して消すと、選ぶ行の見積りが多い回に Postgres が表の全行を読む計画を選ぶ（#1732）。
/// この表の行は trigger と掃除が INSERT と DELETE するだけで UPDATE しない。選んだ行は同じ文で lock を
/// 持つので、`ctid` は文の中で変わらない。
pub const TRUST_SWEEP_SQL: &str = "WITH lapsed AS (
         SELECT ctid FROM cn_safety.trust_target_signals
         WHERE removal_at <= $1
         ORDER BY removal_at
         LIMIT $2
         FOR UPDATE SKIP LOCKED
     ), retired AS (
         SELECT ctid FROM cn_safety.trust_target_signals
         WHERE persisted_at <= $1 - cn_admin.retention_interval('risk_signal')
         ORDER BY persisted_at
         LIMIT $2
         FOR UPDATE SKIP LOCKED
     )
     DELETE FROM cn_safety.trust_target_signals
     WHERE ctid = ANY (ARRAY(SELECT * FROM lapsed UNION SELECT * FROM retired LIMIT $2))";

const DISCLOSED_PUBLIC_PAGE_SQL: &str = "SELECT s.*
    FROM cn_safety.trust_target_signals e
    JOIN cn_safety.risk_signals s ON s.id = e.signal_id
    WHERE e.target_pubkey = $1 AND e.disclosure = 'public'
      AND (e.persisted_at, e.signal_id) < ($2, $3)
    ORDER BY e.persisted_at DESC, e.signal_id DESC
    LIMIT $4";

const DISCLOSED_SUBSCRIBED_PAGE_SQL: &str = "SELECT s.*
    FROM cn_safety.trust_target_signals e
    JOIN cn_safety.risk_signals s ON s.id = e.signal_id
    WHERE e.target_pubkey = $1 AND e.disclosure IS NOT NULL
      AND (e.persisted_at, e.signal_id) < ($2, $3)
    ORDER BY e.persisted_at DESC, e.signal_id DESC
    LIMIT $4";

/// basis のページの位置（前のページの最後の行）。wire では不透明な文字列として渡す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustBasisCursor {
    absolute: bool,
    persisted_at: DateTime<Utc>,
    signal_id: String,
}

impl TrustBasisCursor {
    /// 最初のページの前（どの行よりも後ろの位置）。
    fn start() -> Self {
        Self {
            absolute: true,
            persisted_at: DateTime::<Utc>::MAX_UTC,
            signal_id: String::new(),
        }
    }

    fn after(input: &TrustRiskInput) -> Self {
        Self {
            absolute: input.component == kukuri_cn_trust::TrustComponentKind::Absolute,
            persisted_at: input.persisted_at,
            signal_id: input.signal_id.clone(),
        }
    }

    /// `basis_next_cursor` の文字列を読む。読めなければ `None`。
    pub fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.splitn(3, '.');
        let absolute = match parts.next()? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        let persisted_at = DateTime::from_timestamp_micros(parts.next()?.parse().ok()?)?;
        let signal_id = parts.next()?.to_string();
        (!signal_id.is_empty()).then_some(Self {
            absolute,
            persisted_at,
            signal_id,
        })
    }

    fn encode(&self) -> String {
        format!(
            "{}.{}.{}",
            u8::from(self.absolute),
            self.persisted_at.timestamp_micros(),
            self.signal_id
        )
    }
}

/// basis の 1 ページ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustBasisPage {
    pub inputs: Vec<TrustRiskInput>,
    /// 続きがあれば次のページの cursor。
    pub next_cursor: Option<String>,
}

/// 永続化された risk signal 1 行を trust 入力にする（category で絶対 / 相対へ振り分ける）。
pub fn trust_risk_input(stored: &StoredRiskSignal) -> TrustRiskInput {
    let signal = &stored.signal;
    TrustRiskInput {
        signal_id: stored.id.clone(),
        issuer_node_id: stored.issuer_node_id.clone(),
        target: signal.target,
        target_id: signal.target_id.clone(),
        component: trust_component_for(signal.category),
        category: signal.category,
        severity: signal.severity,
        basis: signal.basis,
        confidence: signal.confidence,
        visibility: signal.visibility,
        appeal_status: signal.appeal_status.unwrap_or_default(),
        expires_at: signal.expires_at.clone(),
        persisted_at: stored.persisted_at,
        operator_adjusted_at: stored.operator_adjusted_at,
    }
}

/// 対象ごとの集計を読む。要求した対象はすべて返す（行の無い対象は成分 0）。
pub async fn load_trust_totals(
    pool: &PgPool,
    targets: &[String],
) -> Result<HashMap<String, TrustTotals>> {
    let rows = sqlx::query(TRUST_TOTALS_SQL)
        .bind(targets)
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get("target_pubkey")?,
                TrustTotals {
                    absolute_units: row.try_get("absolute_units")?,
                    relative_units: row.try_get("relative_units")?,
                    relative_at: row.try_get("relative_at")?,
                    half_life_days: row.try_get("half_life_days")?,
                    disclosed_public_units: row.try_get("disclosed_public_units")?,
                    disclosed_subscribed_units: row.try_get("disclosed_subscribed_units")?,
                    digest: row.try_get("digest")?,
                },
            ))
        })
        .collect()
}

/// 対象の basis の 1 ページ（`cursor` が無ければ最初のページ）。
pub async fn list_trust_basis_page(
    pool: &PgPool,
    target_pubkey: &str,
    cursor: Option<&TrustBasisCursor>,
) -> Result<TrustBasisPage> {
    let start = TrustBasisCursor::start();
    let cursor = cursor.unwrap_or(&start);
    let rows = sqlx::query(TRUST_BASIS_PAGE_SQL)
        .bind(target_pubkey)
        .bind(cursor.absolute)
        .bind(cursor.persisted_at)
        .bind(cursor.signal_id.as_str())
        .bind(TRUST_BASIS_PAGE_SIZE as i64 + 1)
        .fetch_all(pool)
        .await?;
    page_from_rows(&rows)
}

/// pull で `audience` へ開示できる basis の 1 ページ（新しい順）。
pub async fn list_disclosed_trust_basis_page(
    pool: &PgPool,
    target_pubkey: &str,
    audience: PullAudience,
    cursor: Option<&TrustBasisCursor>,
) -> Result<TrustBasisPage> {
    let start = TrustBasisCursor::start();
    let cursor = cursor.unwrap_or(&start);
    let sql = match audience {
        PullAudience::Public => DISCLOSED_PUBLIC_PAGE_SQL,
        PullAudience::SubscribedNodes => DISCLOSED_SUBSCRIBED_PAGE_SQL,
    };
    let rows = sqlx::query(sql)
        .bind(target_pubkey)
        .bind(cursor.persisted_at)
        .bind(cursor.signal_id.as_str())
        .bind(TRUST_BASIS_PAGE_SIZE as i64 + 1)
        .fetch_all(pool)
        .await?;
    page_from_rows(&rows)
}

fn page_from_rows(rows: &[sqlx::postgres::PgRow]) -> Result<TrustBasisPage> {
    let mut inputs = rows
        .iter()
        .map(|row| risk_signal_from_row(row).map(|stored| trust_risk_input(&stored)))
        .collect::<Result<Vec<_>>>()?;
    let next_cursor = (inputs.len() > TRUST_BASIS_PAGE_SIZE).then(|| {
        inputs.truncate(TRUST_BASIS_PAGE_SIZE);
        inputs
            .last()
            .map(TrustBasisCursor::after)
            .unwrap_or_else(TrustBasisCursor::start)
            .encode()
    });
    Ok(TrustBasisPage {
        inputs,
        next_cursor,
    })
}

/// 期限（保持期間・失効時刻）が `now` までに来た行を、1 回 [`TRUST_SWEEP_BATCH`] 行まで集計から外す。
/// 消した行数を返す。他の取引が削除中の行は飛ばす（次の回に残る）。保持期間は保存時刻と保持日数
/// （`cn_admin.retention_days`）で判定するので、日数を縮めた分の行もここで外れる。
pub async fn sweep_expired_trust_signals(pool: &PgPool, now: DateTime<Utc>) -> Result<u64> {
    restore_retained_trust_signals(pool, now).await?;
    let result = sqlx::query(TRUST_SWEEP_SQL)
        .bind(now)
        .bind(TRUST_SWEEP_BATCH)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// risk signal の保持日数を延ばした分の期間に保存された未削除の行を、集計へ戻す（前の日数で期限切れとして
/// 外していた）。集計に反映した日数（`trust_settings.risk_signal_days`）を今の日数に揃える。
async fn restore_retained_trust_signals(pool: &PgPool, now: DateTime<Utc>) -> Result<()> {
    let mut tx = pool.begin().await?;
    let changed: Option<(i32, i32)> = sqlx::query_as(
        "UPDATE cn_safety.trust_settings s SET risk_signal_days = d.days
         FROM cn_admin.retention_days d,
              (SELECT risk_signal_days FROM cn_safety.trust_settings) previous
         WHERE d.category = 'risk_signal' AND s.risk_signal_days <> d.days
         RETURNING previous.risk_signal_days, d.days",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((previous, days)) = changed.filter(|(previous, days)| days > previous) {
        // 内容の行の集計を作り直す trigger と同じ lock（#1699）を排他で取り、著者の対応の変更と重ねない。
        sqlx::query(
            "SELECT pg_advisory_xact_lock(
                hashtextextended('cn_safety.risk_signal_subject_authors', 0))",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO cn_safety.trust_target_signals
             SELECT entry.*
             FROM cn_safety.risk_signals signal
             CROSS JOIN LATERAL cn_safety.trust_signal_entries(signal) AS entry
             WHERE signal.persisted_at > $1 - make_interval(days => $3)
               AND signal.persisted_at <= $1 - make_interval(days => $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(now)
        .bind(previous)
        .bind(days)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 集計に使う半減期を運営者の設定と揃える。違う半減期で作った集計は [`rebuild_trust_totals`] が作り直す。
pub async fn sync_trust_half_life(pool: &PgPool, half_life_days: f64) -> Result<()> {
    sqlx::query(
        "UPDATE cn_safety.trust_settings SET relative_half_life_days = $1
         WHERE relative_half_life_days <> $1",
    )
    .bind(half_life_days)
    .execute(pool)
    .await?;
    Ok(())
}

/// 設定と違う半減期で作った集計を、1 回 [`TRUST_REBUILD_BATCH`] 対象まで作り直す。作り直した対象の数を返す。
///
/// 先に集計の行を lock してから、別の文（新しい snapshot）で対象の行を読み直す。lock の後に確定した行は
/// 読み直しに入り、lock の間に書かれた行はその取引の trigger が作り直した後の集計へ足す。
pub async fn rebuild_trust_totals(pool: &PgPool, now: DateTime<Utc>) -> Result<u64> {
    let mut tx = pool.begin().await?;
    let targets: Vec<String> = sqlx::query_scalar(
        "SELECT t.target_pubkey FROM cn_safety.trust_target_totals t
         WHERE t.target_pubkey IN (
             (SELECT target_pubkey FROM cn_safety.trust_target_totals, cn_safety.trust_settings
              WHERE half_life_days < relative_half_life_days
              ORDER BY half_life_days LIMIT $1)
             UNION ALL
             (SELECT target_pubkey FROM cn_safety.trust_target_totals, cn_safety.trust_settings
              WHERE half_life_days > relative_half_life_days
              ORDER BY half_life_days DESC LIMIT $1))
         ORDER BY t.target_pubkey
         LIMIT $1
         FOR UPDATE SKIP LOCKED",
    )
    .bind(TRUST_REBUILD_BATCH)
    .fetch_all(&mut *tx)
    .await?;
    if targets.is_empty() {
        return Ok(0);
    }
    sqlx::query(
        "UPDATE cn_safety.trust_target_totals t
         SET (relative_units, relative_terms, relative_at, half_life_days) = (
             SELECT COALESCE(sum(e.units * power(0.5::float8,
                        extract(epoch FROM r.at - e.persisted_at)::float8
                            / 86400 / s.relative_half_life_days)), 0),
                    count(e.signal_id), r.at, s.relative_half_life_days
             FROM cn_safety.trust_settings s
             CROSS JOIN LATERAL (
                 SELECT GREATEST($2, max(persisted_at)) AS at
                 FROM cn_safety.trust_target_signals
                 WHERE target_pubkey = t.target_pubkey AND NOT absolute AND units > 0
             ) AS r
             LEFT JOIN cn_safety.trust_target_signals e
               ON e.target_pubkey = t.target_pubkey AND NOT e.absolute AND e.units > 0
             GROUP BY r.at, s.relative_half_life_days)
         WHERE t.target_pubkey = ANY($1)",
    )
    .bind(&targets)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(targets.len() as u64)
}
