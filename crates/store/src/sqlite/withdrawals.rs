use super::*;
use kukuri_core::{PostWithdrawalReason, WithdrawalReasonVisibility};

fn visibility_name(value: WithdrawalReasonVisibility) -> &'static str {
    match value {
        WithdrawalReasonVisibility::Public => "public",
        WithdrawalReasonVisibility::Private => "private",
    }
}

fn reason_name(value: PostWithdrawalReason) -> &'static str {
    match value {
        PostWithdrawalReason::AuthorRequest => "author_request",
        PostWithdrawalReason::Correction => "correction",
        PostWithdrawalReason::Privacy => "privacy",
        PostWithdrawalReason::Other => "other",
    }
}

fn parse_visibility(value: &str) -> Result<WithdrawalReasonVisibility> {
    match value {
        "public" => Ok(WithdrawalReasonVisibility::Public),
        "private" => Ok(WithdrawalReasonVisibility::Private),
        _ => anyhow::bail!("unknown withdrawal reason visibility: {value}"),
    }
}

fn parse_reason(value: Option<String>) -> Result<Option<PostWithdrawalReason>> {
    value
        .map(|value| match value.as_str() {
            "author_request" => Ok(PostWithdrawalReason::AuthorRequest),
            "correction" => Ok(PostWithdrawalReason::Correction),
            "privacy" => Ok(PostWithdrawalReason::Privacy),
            "other" => Ok(PostWithdrawalReason::Other),
            _ => anyhow::bail!("unknown post withdrawal reason: {value}"),
        })
        .transpose()
}

#[async_trait]
impl PostWithdrawalStore for SqliteStore {
    async fn queue_withdrawal_writes(&self, rows: Vec<WithdrawalWriteRow>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for row in rows {
            sqlx::query(
                "INSERT OR REPLACE INTO withdrawal_write_outbox                  (withdrawal_envelope_id, replica_id, target_object_id, envelope_json, target_replica_id)                  VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(row.withdrawal_envelope_id.as_str())
            .bind(row.replica_id.as_str())
            .bind(row.target_object_id.as_str())
            .bind(serde_json::to_string(&row.envelope)?)
            .bind(row.target_replica_id.as_ref().map(ReplicaId::as_str))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn pending_withdrawal_writes(&self, limit: usize) -> Result<Vec<WithdrawalWriteRow>> {
        let rows = sqlx::query(
            "SELECT withdrawal_envelope_id, replica_id, target_object_id, envelope_json, target_replica_id              FROM withdrawal_write_outbox ORDER BY rowid LIMIT ?1",
        )
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(WithdrawalWriteRow {
                    withdrawal_envelope_id: EnvelopeId::from(
                        row.get::<String, _>("withdrawal_envelope_id"),
                    ),
                    replica_id: ReplicaId::new(row.get::<String, _>("replica_id")),
                    target_object_id: EnvelopeId::from(row.get::<String, _>("target_object_id")),
                    envelope: serde_json::from_str(&row.get::<String, _>("envelope_json"))?,
                    target_replica_id: row
                        .get::<Option<String>, _>("target_replica_id")
                        .map(ReplicaId::new),
                })
            })
            .collect()
    }

    async fn finish_withdrawal_write(
        &self,
        withdrawal_envelope_id: &EnvelopeId,
        replica_id: &ReplicaId,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM withdrawal_write_outbox WHERE withdrawal_envelope_id = ?1 AND replica_id = ?2",
        )
        .bind(withdrawal_envelope_id.as_str())
        .bind(replica_id.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn put_post_withdrawal(&self, row: PostWithdrawalRow) -> Result<bool> {
        let object_id = row.target_object_id.clone();
        let mut tx = super::public_blobs::begin_public_blob_write(self).await?;
        let result = sqlx::query(
            r#"
            INSERT INTO post_withdrawals (
              target_object_id, target_author_pubkey, source_replica_id,
              withdrawal_envelope_id, withdrawn_at, generation, replacement_object_id,
              reason_visibility, reason
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(target_object_id) DO UPDATE SET
              target_author_pubkey = excluded.target_author_pubkey,
              source_replica_id = excluded.source_replica_id,
              withdrawal_envelope_id = excluded.withdrawal_envelope_id,
              withdrawn_at = excluded.withdrawn_at,
              generation = excluded.generation,
              replacement_object_id = excluded.replacement_object_id,
              reason_visibility = excluded.reason_visibility,
              reason = excluded.reason
            WHERE excluded.generation > post_withdrawals.generation
               OR (excluded.generation = post_withdrawals.generation
                   AND excluded.withdrawn_at > post_withdrawals.withdrawn_at)
               OR (excluded.generation = post_withdrawals.generation
                   AND excluded.withdrawn_at = post_withdrawals.withdrawn_at
                   AND excluded.withdrawal_envelope_id > post_withdrawals.withdrawal_envelope_id)
            "#,
        )
        .bind(row.target_object_id.as_str())
        .bind(row.target_author_pubkey)
        .bind(row.source_replica_id.as_str())
        .bind(row.withdrawal_envelope_id.as_str())
        .bind(row.withdrawn_at)
        .bind(i64::try_from(row.generation)?)
        .bind(row.replacement_object_id.as_ref().map(EnvelopeId::as_str))
        .bind(visibility_name(row.reason_visibility))
        .bind(row.reason.map(reason_name))
        .execute(&mut *tx)
        .await?;
        // #1632: 取り下げた投稿の本文・添付・リンクプレビューの画像は、公開参照から外す。
        super::public_blobs::forget_post_public_blob_refs(&mut tx, object_id.as_str()).await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    async fn get_post_withdrawal(
        &self,
        target_object_id: &EnvelopeId,
    ) -> Result<Option<PostWithdrawalRow>> {
        let row = sqlx::query(
            r#"
            SELECT target_object_id, target_author_pubkey, source_replica_id,
                   withdrawal_envelope_id, withdrawn_at, generation, replacement_object_id,
                   reason_visibility, reason
            FROM post_withdrawals
            WHERE target_object_id = ?1
            "#,
        )
        .bind(target_object_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(PostWithdrawalRow {
                target_object_id: EnvelopeId::from(row.try_get::<String, _>("target_object_id")?),
                target_author_pubkey: row.try_get("target_author_pubkey")?,
                source_replica_id: ReplicaId::new(row.try_get::<String, _>("source_replica_id")?),
                withdrawal_envelope_id: EnvelopeId::from(
                    row.try_get::<String, _>("withdrawal_envelope_id")?,
                ),
                withdrawn_at: row.try_get("withdrawn_at")?,
                generation: u64::try_from(row.try_get::<i64, _>("generation")?)?,
                replacement_object_id: row
                    .try_get::<Option<String>, _>("replacement_object_id")?
                    .map(EnvelopeId::from),
                reason_visibility: parse_visibility(
                    row.try_get::<String, _>("reason_visibility")?.as_str(),
                )?,
                reason: parse_reason(row.try_get("reason")?)?,
            })
        })
        .transpose()
    }
}
