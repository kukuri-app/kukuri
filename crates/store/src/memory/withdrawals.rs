use super::*;

fn is_newer_withdrawal(candidate: &PostWithdrawalRow, current: &PostWithdrawalRow) -> bool {
    (
        candidate.generation,
        candidate.withdrawn_at,
        candidate.withdrawal_envelope_id.as_str(),
    ) > (
        current.generation,
        current.withdrawn_at,
        current.withdrawal_envelope_id.as_str(),
    )
}

#[async_trait]
impl PostWithdrawalStore for MemoryStore {
    async fn put_post_withdrawal(&self, row: PostWithdrawalRow) -> Result<bool> {
        let mut rows = self.post_withdrawal_rows.write().await;
        if rows
            .get(&row.target_object_id)
            .is_some_and(|current| !is_newer_withdrawal(&row, current))
        {
            return Ok(false);
        }
        rows.insert(row.target_object_id.clone(), row);
        Ok(true)
    }

    async fn get_post_withdrawal(
        &self,
        target_object_id: &EnvelopeId,
    ) -> Result<Option<PostWithdrawalRow>> {
        Ok(self
            .post_withdrawal_rows
            .read()
            .await
            .get(target_object_id)
            .cloned())
    }

    async fn queue_withdrawal_writes(&self, rows: Vec<WithdrawalWriteRow>) -> Result<()> {
        let mut pending = self.withdrawal_write_rows.write().await;
        for row in rows {
            pending.retain(|item| {
                (&item.withdrawal_envelope_id, &item.replica_id)
                    != (&row.withdrawal_envelope_id, &row.replica_id)
            });
            pending.push(row);
        }
        Ok(())
    }

    async fn pending_withdrawal_writes(&self, limit: usize) -> Result<Vec<WithdrawalWriteRow>> {
        Ok(self
            .withdrawal_write_rows
            .read()
            .await
            .iter()
            .take(limit)
            .cloned()
            .collect())
    }

    async fn finish_withdrawal_write(
        &self,
        withdrawal_envelope_id: &EnvelopeId,
        replica_id: &ReplicaId,
    ) -> Result<()> {
        self.withdrawal_write_rows.write().await.retain(|item| {
            (&item.withdrawal_envelope_id, &item.replica_id) != (withdrawal_envelope_id, replica_id)
        });
        Ok(())
    }
}
