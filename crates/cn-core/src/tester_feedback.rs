//! community node のテスターフィードバック保存(#802 / ADR 0039)。
//!
//! テスターの自由記述 3 項目と自動付与された client version / OS のみを保存する。
//! 送信者の identity(pubkey)は保存しない。本文は連絡先 PII ではないため
//! plain TEXT で保存する(ADR 0039)。

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use uuid::Uuid;

/// 保存済みのテスターフィードバックレコード。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TesterFeedback {
    pub id: String,
    pub what_attempted: String,
    pub what_happened: String,
    pub what_seemed_wrong: String,
    pub client_version: String,
    pub os: String,
    pub created_at: DateTime<Utc>,
}

/// 新規テスターフィードバックの入力。送信者 identity は受け取らない。
#[derive(Clone, Debug, Default)]
pub struct NewTesterFeedback {
    pub what_attempted: String,
    pub what_happened: String,
    pub what_seemed_wrong: String,
    pub client_version: String,
    pub os: String,
}

/// 受信したテスターフィードバックを保存し、参照 ID を含むレコードを返す。保持の起算点は保存した時刻。
pub async fn insert_tester_feedback(
    pool: &PgPool,
    input: &NewTesterFeedback,
) -> Result<TesterFeedback> {
    let id = Uuid::new_v4().to_string();
    let row = sqlx::query(
        "INSERT INTO cn_admin.tester_feedback
            (id, what_attempted, what_happened, what_seemed_wrong, client_version, os)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, what_attempted, what_happened, what_seemed_wrong, client_version, os,
                   created_at",
    )
    .bind(&id)
    .bind(&input.what_attempted)
    .bind(&input.what_happened)
    .bind(&input.what_seemed_wrong)
    .bind(&input.client_version)
    .bind(&input.os)
    .fetch_one(pool)
    .await?;
    tester_feedback_from_row(&row)
}

/// 受信したテスターフィードバックを新着順で取得する(運営者の確認用)。
pub async fn list_tester_feedback(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<TesterFeedback>> {
    let rows = sqlx::query(
        "SELECT id, what_attempted, what_happened, what_seemed_wrong, client_version, os,
                created_at
         FROM cn_admin.tester_feedback
         WHERE created_at > NOW() - cn_admin.retention_interval('tester_feedback')
         ORDER BY created_at DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    rows.iter().map(tester_feedback_from_row).collect()
}

/// 単一のテスターフィードバックを ID で取得する。
pub async fn get_tester_feedback(pool: &PgPool, id: &str) -> Result<Option<TesterFeedback>> {
    let row = sqlx::query(
        "SELECT id, what_attempted, what_happened, what_seemed_wrong, client_version, os,
                created_at
         FROM cn_admin.tester_feedback
         WHERE id = $1 AND created_at > NOW() - cn_admin.retention_interval('tester_feedback')",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(tester_feedback_from_row).transpose()
}

fn tester_feedback_from_row(row: &PgRow) -> Result<TesterFeedback> {
    Ok(TesterFeedback {
        id: row.try_get("id")?,
        what_attempted: row.try_get("what_attempted")?,
        what_happened: row.try_get("what_happened")?,
        what_seemed_wrong: row.try_get("what_seemed_wrong")?,
        client_version: row.try_get("client_version")?,
        os: row.try_get("os")?,
        created_at: row.try_get("created_at")?,
    })
}
