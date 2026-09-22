use super::*;

const TIMELINE_SELECT: &str = r#"
    SELECT object_id, topic_id, author_pubkey, created_at, object_kind, root_object_id,
           reply_to_object_id, channel_id, payload_ref_json, content, attachments_json,
           repost_of_json, content_labels_json, source_replica_id, source_key,
           source_envelope_id, source_blob_hash, derived_at, projection_version,
           source_docs_author
"#;

const THREAD_SELECT: &str = r#"
    SELECT oic.object_id, oic.topic_id, oic.author_pubkey, oic.created_at, oic.object_kind,
           oic.root_object_id, oic.reply_to_object_id, oic.channel_id,
           oic.payload_ref_json, oic.content, oic.attachments_json, oic.repost_of_json,
           oic.content_labels_json, oic.source_replica_id, oic.source_key,
           oic.source_docs_author, oic.source_envelope_id, oic.source_blob_hash, oic.derived_at,
           oic.projection_version
    FROM object_thread_cache tc
    INNER JOIN object_index_cache oic ON oic.object_id = tc.object_id
"#;

/// cursor より古い行だけを読む条件を足す(#1239)。
///
/// 行の値の比較で書く。`created_at < ? OR (created_at = ? AND object_id < ?)` の形や、`? IS NULL OR …` と
/// 1 つの SQL にまとめた形は、索引の範囲の読み出しにならず、遡った深さに比例して行を読み飛ばす。
fn push_timeline_cursor(builder: &mut QueryBuilder<'_, Sqlite>, cursor: Option<&TimelineCursor>) {
    if let Some(cursor) = cursor {
        builder.push(" AND (created_at, object_id) < (");
        builder.push_bind(cursor.created_at);
        builder.push(", ");
        builder.push_bind(cursor.object_id.as_str().to_string());
        builder.push(")");
    }
}

/// タイムラインの 1 ページを読む SQL を組み立てる。`channel_id` が `None` なら channel で絞らない。
///
/// channel で絞るときは 1 つだけ(#1280)。(topic, channel, 時刻, id) の索引の範囲をそのまま読み、許可されない
/// channel の行を読み飛ばさない。複数の channel をまたぐページは作らない。
///
/// `prefix` は SQL の前に置く文字列(取得は空、query plan の test は `EXPLAIN QUERY PLAN `)。test が実装と同じ
/// SQL の plan を確かめられるように、組み立てをここに 1 つだけ置く。
pub(crate) fn timeline_page_query<'a>(
    prefix: &str,
    topic_id: &'a str,
    channel_id: Option<&'a str>,
    cursor: Option<&TimelineCursor>,
    limit: usize,
) -> QueryBuilder<'a, Sqlite> {
    let mut builder = QueryBuilder::<Sqlite>::new(prefix);
    builder.push(TIMELINE_SELECT);
    builder.push(" FROM object_index_cache WHERE topic_id = ");
    builder.push_bind(topic_id);
    if let Some(channel_id) = channel_id {
        builder.push(" AND channel_id = ");
        builder.push_bind(channel_id);
    }
    push_timeline_cursor(&mut builder, cursor);
    builder.push(" ORDER BY created_at DESC, object_id DESC LIMIT ");
    builder.push_bind(limit as i64);
    builder
}

/// thread の root の行を 1 行引く SQL(`after` が `None`)、または返信の範囲を読む SQL を組み立てる。
pub(crate) fn thread_page_query<'a>(
    prefix: &str,
    topic_id: &'a str,
    root_id: &'a str,
    channel_id: Option<&'a str>,
    part: ThreadPagePart<'_>,
) -> QueryBuilder<'a, Sqlite> {
    let mut builder = QueryBuilder::<Sqlite>::new(prefix);
    builder.push(THREAD_SELECT);
    builder.push(" WHERE tc.topic_id = ");
    builder.push_bind(topic_id);
    builder.push(" AND tc.root_object_id = ");
    builder.push_bind(root_id);
    if let Some(channel_id) = channel_id {
        builder.push(" AND tc.channel_id = ");
        builder.push_bind(channel_id);
    }
    match part {
        ThreadPagePart::Root => {
            builder.push(" AND tc.object_id = ");
            builder.push_bind(root_id);
        }
        ThreadPagePart::Replies { after, limit } => {
            builder.push(" AND tc.object_id != ");
            builder.push_bind(root_id);
            if let Some(after) = after {
                builder.push(" AND (tc.created_at, tc.object_id) > (");
                builder.push_bind(after.created_at);
                builder.push(", ");
                builder.push_bind(after.object_id.as_str().to_string());
                builder.push(")");
            }
            builder.push(" ORDER BY tc.created_at ASC, tc.object_id ASC LIMIT ");
            builder.push_bind(limit as i64);
        }
    }
    builder
}

pub(crate) enum ThreadPagePart<'a> {
    Root,
    Replies {
        after: Option<&'a TimelineCursor>,
        limit: usize,
    },
}

impl SqliteStore {
    /// thread の 1 ページ(root が先頭、返信は古い順)。
    ///
    /// 返信は `object_thread_cache` の (topic, root, 時刻, id) の索引の範囲を、cursor の位置から読む。root を先頭に
    /// 置くために全行を並べ替える形(`ORDER BY CASE …`)にすると、1 ページの取得が thread の返信の総数に比例する。
    /// root は最初のページでだけ 1 行引きし、返信の範囲からは除く。
    async fn thread_page(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        channel_id: Option<&str>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        if limit == 0 {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: cursor,
            });
        }
        let root_id = thread_root_object_id.as_str();
        // root を返すのは、cursor の無い最初のページだけ。root の位置の cursor は「返信の先頭から」を表す
        // (返信の時刻が root より前でも、時計のずれで飛ばさない)。
        let first_page = cursor.is_none();
        let after = cursor.filter(|cursor| cursor.object_id.as_str() != root_id);
        let mut rows = Vec::new();
        if first_page {
            rows.extend(
                thread_page_query("", topic_id, root_id, channel_id, ThreadPagePart::Root)
                    .build()
                    .fetch_all(&self.pool)
                    .await?,
            );
        }
        let remaining = limit.saturating_sub(rows.len());
        if remaining > 0 {
            rows.extend(
                thread_page_query(
                    "",
                    topic_id,
                    root_id,
                    channel_id,
                    ThreadPagePart::Replies {
                        after: after.as_ref(),
                        limit: remaining,
                    },
                )
                .build()
                .fetch_all(&self.pool)
                .await?,
            );
        }
        object_projection_page_from_rows(rows, limit)
    }
}

#[async_trait]
impl ObjectProjectionStore for SqliteStore {
    async fn put_object_projection(&self, row: ObjectProjectionRow) -> Result<()> {
        self.put_object_projections(vec![row]).await
    }

    async fn put_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for row in rows {
            let payload_json = serde_json::to_string(&row.payload_ref)?;
            let attachments_json = serde_json::to_string(&row.attachments)?;
            let content_labels_json = serde_json::to_string(&row.content_labels)?;
            let repost_json = row
                .repost_of
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            sqlx::query(
                r#"
                INSERT INTO object_index_cache (
                  object_id, topic_id, channel_id, author_pubkey, created_at, object_kind,
                  root_object_id, reply_to_object_id, payload_ref_json, content, attachments_json,
                  repost_of_json, content_labels_json, source_replica_id, source_key,
                  source_envelope_id, source_blob_hash, derived_at, projection_version,
                  source_docs_author
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                ON CONFLICT(object_id) DO UPDATE SET
                  topic_id = excluded.topic_id,
                  channel_id = excluded.channel_id,
                  author_pubkey = excluded.author_pubkey,
                  created_at = excluded.created_at,
                  object_kind = excluded.object_kind,
                  root_object_id = excluded.root_object_id,
                  reply_to_object_id = excluded.reply_to_object_id,
                  payload_ref_json = excluded.payload_ref_json,
                  content = excluded.content,
                  attachments_json = excluded.attachments_json,
                  repost_of_json = excluded.repost_of_json,
                  content_labels_json = excluded.content_labels_json,
                  source_replica_id = excluded.source_replica_id,
                  source_key = excluded.source_key,
                  source_envelope_id = excluded.source_envelope_id,
                  source_blob_hash = excluded.source_blob_hash,
                  derived_at = excluded.derived_at,
                  projection_version = excluded.projection_version,
                  source_docs_author = excluded.source_docs_author
                "#,
            )
            .bind(row.object_id.as_str())
            .bind(row.topic_id.as_str())
            .bind(row.channel_id.as_str())
            .bind(row.author_pubkey.as_str())
            .bind(row.created_at)
            .bind(row.object_kind.as_str())
            .bind(row.root_object_id.as_ref().map(EnvelopeId::as_str))
            .bind(row.reply_to_object_id.as_ref().map(EnvelopeId::as_str))
            .bind(payload_json)
            .bind(row.content.as_deref())
            .bind(attachments_json)
            .bind(repost_json.as_deref())
            .bind(content_labels_json)
            .bind(row.source_replica_id.as_str())
            .bind(row.source_key.as_str())
            .bind(row.source_envelope_id.as_str())
            .bind(row.source_blob_hash.as_ref().map(BlobHash::as_str))
            .bind(row.derived_at)
            .bind(row.projection_version)
            .bind(row.source_docs_author.as_deref())
            .execute(&mut *tx)
            .await?;

            // #858: 成人向けラベル付き投稿(引用 snapshot 含む)の添付 hash を
            // 逆引きテーブルへ記録する(insert-only)。blob 取得ゲートの判定に使う。
            for hash in adult_media_hashes_for_row(&row) {
                sqlx::query(
                    r#"
                    INSERT INTO adult_media_hashes (blob_hash, marked_at)
                    VALUES (?1, ?2)
                    ON CONFLICT(blob_hash) DO NOTHING
                    "#,
                )
                .bind(hash)
                .bind(row.derived_at)
                .execute(&mut *tx)
                .await?;
            }

            sqlx::query(
                r#"
                INSERT INTO object_thread_cache (
                  object_id, topic_id, channel_id, root_object_id, created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(object_id) DO UPDATE SET
                  topic_id = excluded.topic_id,
                  channel_id = excluded.channel_id,
                  root_object_id = excluded.root_object_id,
                  created_at = excluded.created_at
                "#,
            )
            .bind(row.object_id.as_str())
            .bind(row.topic_id.as_str())
            .bind(row.channel_id.as_str())
            .bind(
                row.root_object_id
                    .as_ref()
                    .unwrap_or(&row.object_id)
                    .as_str(),
            )
            .bind(row.created_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn get_object_projection(
        &self,
        object_id: &EnvelopeId,
    ) -> Result<Option<ObjectProjectionRow>> {
        let row = sqlx::query(
            r#"
            SELECT object_id, topic_id, author_pubkey, created_at, object_kind, root_object_id,
                   reply_to_object_id, channel_id, payload_ref_json, content, attachments_json,
                   repost_of_json, content_labels_json, source_replica_id, source_key,
                   source_envelope_id, source_blob_hash, derived_at, projection_version,
                   source_docs_author
            FROM object_index_cache
            WHERE object_id = ?1
            "#,
        )
        .bind(object_id.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_object_projection).transpose()
    }

    async fn find_author_reposts_of(
        &self,
        topic_id: &str,
        author_pubkey: &str,
        source_object_id: &EnvelopeId,
        limit: usize,
    ) -> Result<Vec<ObjectProjectionRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // 条件の式は `idx_object_index_cache_repost_source` と同じ形に保つ(索引を使うため)。
        let rows = sqlx::query(
            r#"
            SELECT object_id, topic_id, author_pubkey, created_at, object_kind, root_object_id,
                   reply_to_object_id, channel_id, payload_ref_json, content, attachments_json,
                   repost_of_json, content_labels_json, source_replica_id, source_key,
                   source_envelope_id, source_blob_hash, derived_at, projection_version,
                   source_docs_author
            FROM object_index_cache
            WHERE object_kind = 'repost'
              AND topic_id = ?1
              AND author_pubkey = ?2
              AND json_extract(repost_of_json, '$.source_object_id') = ?3
            ORDER BY created_at DESC
            LIMIT ?4
            "#,
        )
        .bind(topic_id)
        .bind(author_pubkey)
        .bind(source_object_id.as_str())
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_object_projection).collect()
    }

    async fn mark_adult_media_hashes(&self, hashes: &[BlobHash]) -> Result<()> {
        if hashes.is_empty() {
            return Ok(());
        }
        let marked_at = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        let mut tx = self.pool.begin().await?;
        for hash in hashes {
            sqlx::query(
                r#"
                INSERT INTO adult_media_hashes (blob_hash, marked_at)
                VALUES (?1, ?2)
                ON CONFLICT(blob_hash) DO NOTHING
                "#,
            )
            .bind(hash.as_str())
            .bind(marked_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn is_adult_media_hash(&self, hash: &BlobHash) -> Result<bool> {
        let row = sqlx::query(
            r#"
            SELECT 1 AS marked FROM adult_media_hashes WHERE blob_hash = ?1
            "#,
        )
        .bind(hash.as_str())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    async fn list_topic_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        let rows = timeline_page_query("", topic_id, None, cursor.as_ref(), limit)
            .build()
            .fetch_all(&self.pool)
            .await?;
        object_projection_page_from_rows(rows, limit)
    }

    async fn list_topic_timeline_in_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        if limit == 0 {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: cursor,
            });
        }
        let rows = timeline_page_query("", topic_id, Some(channel_id), cursor.as_ref(), limit)
            .build()
            .fetch_all(&self.pool)
            .await?;
        object_projection_page_from_rows(rows, limit)
    }

    async fn list_thread(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        self.thread_page(topic_id, thread_root_object_id, None, cursor, limit)
            .await
    }

    async fn list_thread_filtered(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        allowed_channel: Option<&str>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        self.thread_page(
            topic_id,
            thread_root_object_id,
            allowed_channel,
            cursor,
            limit,
        )
        .await
    }

    async fn rebuild_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM object_thread_cache")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM object_index_cache")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM live_session_cache")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM game_room_cache")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM live_presence_cache")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM reaction_cache")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM adult_media_hashes")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.put_object_projections(rows).await?;
        sqlx::query(
            r#"
            DELETE FROM content_observations
            WHERE subject_kind = 'post'
              AND NOT EXISTS (
                SELECT 1 FROM object_index_cache
                WHERE object_index_cache.object_id = content_observations.subject_id
              )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
