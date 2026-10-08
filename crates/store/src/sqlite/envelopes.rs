use super::*;

impl SqliteStore {
    pub(super) async fn store_put_envelope_impl(&self, envelope: KukuriEnvelope) -> Result<()> {
        let tags_json = serde_json::to_string(&envelope.tags)?;

        let insert = sqlx::query(
            r#"
            INSERT INTO envelopes (envelope_id, pubkey, created_at, kind, content, tags_json, sig)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(envelope_id) DO UPDATE SET
              pubkey = excluded.pubkey,
              created_at = excluded.created_at,
              kind = excluded.kind,
              content = excluded.content,
              tags_json = excluded.tags_json,
              sig = excluded.sig
            "#,
        )
        .bind(envelope.id.as_str())
        .bind(envelope.pubkey.as_str())
        .bind(envelope.created_at)
        .bind(envelope.kind.as_str())
        .bind(envelope.content.as_str())
        .bind(tags_json)
        .bind(envelope.sig.as_str());
        // #1232 AC-3: 自作の custom reaction の asset は、envelope の行と画像の公開参照（ADR 0063 §1）を 1 つの
        // transaction で書く。
        match kukuri_core::parse_custom_reaction_asset(&envelope)? {
            Some(asset) => {
                let mut tx = super::public_blobs::begin_public_blob_write(self).await?;
                insert.execute(&mut *tx).await?;
                super::public_blobs::replace_public_blob_refs(
                    &mut tx,
                    "reaction_asset",
                    envelope.id.as_str(),
                    vec![asset.blob_hash.as_str().to_string()],
                )
                .await?;
                tx.commit().await?;
            }
            None => {
                insert.execute(&self.pool).await?;
            }
        }

        if let Some(topic_id) = envelope.topic_id() {
            sqlx::query(
                r#"
                INSERT INTO topic_objects (topic_id, object_id, created_at)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(topic_id, object_id) DO UPDATE SET created_at = excluded.created_at
                "#,
            )
            .bind(topic_id.as_str())
            .bind(envelope.id.as_str())
            .bind(envelope.created_at)
            .execute(&self.pool)
            .await?;

            let thread_ref = envelope.thread_ref().unwrap_or(ThreadRef {
                root: envelope.id.clone(),
                reply_to: None,
            });
            sqlx::query(
                r#"
                INSERT INTO object_threads (
                  topic_id, object_id, root_object_id, reply_to_object_id, created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(object_id) DO UPDATE SET
                  topic_id = excluded.topic_id,
                  root_object_id = excluded.root_object_id,
                  reply_to_object_id = excluded.reply_to_object_id,
                  created_at = excluded.created_at
                "#,
            )
            .bind(topic_id.as_str())
            .bind(envelope.id.as_str())
            .bind(thread_ref.root.as_str())
            .bind(thread_ref.reply_to.as_ref().map(EnvelopeId::as_str))
            .bind(envelope.created_at)
            .execute(&self.pool)
            .await?;
        }

        if let Some(profile) = parse_profile(&envelope)? {
            self.upsert_profile(profile).await?;
        }
        if let Some(edge) = parse_follow_edge(&envelope)? {
            self.upsert_follow_edge(edge).await?;
        }
        if let Some(edge) = parse_block_edge(&envelope)? {
            self.upsert_block_edge(edge).await?;
        }

        Ok(())
    }

    pub(super) async fn store_get_envelope_impl(
        &self,
        envelope_id: &EnvelopeId,
    ) -> Result<Option<KukuriEnvelope>> {
        let row = sqlx::query(
            r#"
            SELECT envelope_id, pubkey, created_at, kind, content, tags_json, sig
            FROM envelopes
            WHERE envelope_id = ?1
            "#,
        )
        .bind(envelope_id.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_envelope).transpose()
    }

    pub(super) async fn store_list_topic_timeline_impl(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>> {
        let rows = sqlx::query(
            r#"
            SELECT e.envelope_id, e.pubkey, e.created_at, e.kind, e.content, e.tags_json, e.sig
            FROM topic_objects tp
            INNER JOIN envelopes e ON e.envelope_id = tp.object_id
            WHERE tp.topic_id = ?1
              AND (
                ?2 IS NULL
                OR e.created_at < ?2
                OR (e.created_at = ?2 AND e.envelope_id < ?3)
              )
            ORDER BY e.created_at DESC, e.envelope_id DESC
            LIMIT ?4
            "#,
        )
        .bind(topic_id)
        .bind(cursor.as_ref().map(|value| value.created_at))
        .bind(cursor.as_ref().map(|value| value.object_id.as_str()))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        envelope_page_from_rows(rows, limit)
    }

    pub(super) async fn store_list_thread_impl(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>> {
        let rows = sqlx::query(
            r#"
            SELECT e.envelope_id, e.pubkey, e.created_at, e.kind, e.content, e.tags_json, e.sig
            FROM object_threads te
            INNER JOIN envelopes e ON e.envelope_id = te.object_id
            WHERE te.topic_id = ?1
              AND te.root_object_id = ?2
              AND (
                ?3 IS NULL
                OR e.created_at > ?3
                OR (e.created_at = ?3 AND e.envelope_id > ?4)
              )
            ORDER BY
              CASE WHEN e.envelope_id = te.root_object_id THEN 0 ELSE 1 END ASC,
              e.created_at ASC,
              e.envelope_id ASC
            LIMIT ?5
            "#,
        )
        .bind(topic_id)
        .bind(thread_root_object_id.as_str())
        .bind(cursor.as_ref().map(|value| value.created_at))
        .bind(cursor.as_ref().map(|value| value.object_id.as_str()))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        envelope_page_from_rows(rows, limit)
    }
}
