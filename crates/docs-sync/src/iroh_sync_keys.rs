//! 上限つきの key の一覧（手元の namespace と、自分の record の保持分の併合。ADR 0058 §7）。

use super::*;

impl IrohDocsSync {
    /// 上限つきの key の一覧。`author` があれば、その docs author の entry だけを読む。
    pub(super) async fn key_page(
        &self,
        replica_id: &ReplicaId,
        author: Option<AuthorId>,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        // `limit` が 0 でも replica は開く(`MemoryDocsSync` と同じ。権限の無い replica はここで失敗する)。
        let doc = self.read_replica(replica_id).await?;
        if query.limit == 0 {
            return Ok(DocKeyPage::default());
        }
        let page = match doc {
            Some(doc) => {
                self.local_key_page(replica_id, &doc, author, &query)
                    .await?
            }
            None => DocKeyPage::default(),
        };
        // 自分の record の保持分を合わせる。namespace が手元に無くても(Web の reload・native の restore の後)読める
        // (ADR 0058 §7)。手元の先頭 `limit` 件と保持分の先頭 `limit` 件から、和集合の先頭 `limit` 件を取る。
        let Some(cache) = self.remote_cache() else {
            return Ok(page);
        };
        let descending = query.order == DocKeyOrder::Descending;
        let author = author.map(|author| author.to_string());
        let (held, more) = cache
            .remote_record_keys(
                replica_id.as_str(),
                &query.prefix,
                descending,
                author.as_deref(),
                query.limit,
                true,
            )
            .await?;
        let held = held.into_iter().map(|held| DocKeyEntry {
            key: held.key,
            content_hash: held.content_hash,
            content_len: held.content_len,
            docs_author: Some(held.author),
        });
        let (entries, truncated) = kukuri_store::merge_record_keys(
            page.entries,
            held,
            |entry| {
                (
                    entry.key.as_str(),
                    entry.docs_author.as_deref().unwrap_or_default(),
                )
            },
            descending,
            query.limit,
        );
        Ok(DocKeyPage {
            entries,
            reached_limit: page.reached_limit || more || truncated,
        })
    }

    /// 手元の namespace の、上限つきの key の一覧。
    async fn local_key_page(
        &self,
        replica_id: &ReplicaId,
        doc: &Doc,
        author: Option<AuthorId>,
        query: &DocKeyQuery,
    ) -> Result<DocKeyPage> {
        let direction = match query.order {
            DocKeyOrder::Ascending => SortDirection::Asc,
            DocKeyOrder::Descending => SortDirection::Desc,
        };
        let query_limit = query.limit;
        let builder = match author {
            Some(author) => Query::author(author).key_prefix(&query.prefix),
            None => Query::key_prefix(&query.prefix),
        };
        let stream = doc
            .get_many(
                builder
                    .sort_by(SortBy::KeyAuthor, direction)
                    .limit(query.limit as u64)
                    .build(),
            )
            .await?;
        tokio::pin!(stream);
        let mut entries = Vec::new();
        let mut skipped_keys = 0usize;
        let mut scanned = 0usize;
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            scanned += 1;
            // 飛ばした分を読み足さない(追加の query を発行しない)。返す件数が `limit` より減るだけである。
            // 飛ばした entry も `scanned` に数え、打ち切られたかどうかを呼び出し側へ伝える(#1257)。
            let Some(key) = utf8_key(entry.key()) else {
                skipped_keys += 1;
                continue;
            };
            entries.push(DocKeyEntry {
                key,
                content_hash: entry.content_hash().to_string(),
                content_len: entry.content_len(),
                docs_author: Some(entry.author().to_string()),
            });
        }
        warn_skipped_keys(replica_id, skipped_keys);
        Ok(DocKeyPage {
            entries,
            reached_limit: scanned >= query_limit,
        })
    }
}
