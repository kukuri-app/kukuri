//! 公開 blob の発見（#1632、ADR 0063）。検証済みの公開記録が参照する blob の索引と、Mainline への告知の予定。
//!
//! 索引（`public_blob_refs`）は、公開記録を書く transaction で記録ごとに置き換える。告知の予定
//! （`public_blob_announcements`）は、公開参照と返せる保持（期限内の cache の行）の両方がある hash だけを
//! [`PUBLIC_BLOB_ANNOUNCEMENT_CAP`] 件まで持つ。満杯なら本人の blob を先に残し、次に cache の利用が新しいものを残す。

use super::remote_cache::{REMOTE_CACHE_UNUSED_MS, now_ms};
use super::*;
use kukuri_core::{AssetRef, CustomReactionAssetSnapshotV1, ObjectStatus, RepostSourceSnapshotV1};
use std::collections::BTreeSet;

/// 告知を続ける hash の上限（1 端末）。
pub const PUBLIC_BLOB_ANNOUNCEMENT_CAP: i64 = 512;
/// 公開 channel の id（app-api の `PUBLIC_CHANNEL_ID`）。
const PUBLIC_CHANNEL: &str = "public";

/// 公開投稿の行が参照する blob（本文、添付、repost の添付）。公開でない行は空。
pub(super) fn public_blob_hashes_for_row(row: &ObjectProjectionRow) -> Vec<String> {
    if row.channel_id != PUBLIC_CHANNEL {
        return Vec::new();
    }
    row.source_blob_hash
        .iter()
        .map(|hash| hash.as_str().to_string())
        .chain(
            row.attachments
                .iter()
                .map(|asset| asset.hash.as_str().to_string()),
        )
        .chain(
            row.repost_of
                .iter()
                .flat_map(|repost| repost.attachments.iter())
                .map(|asset| asset.hash.as_str().to_string()),
        )
        .collect()
}

/// 記録 1 件が参照する blob を置き換え、変わった hash の告知の予定を合わせる。
pub(super) async fn replace_public_blob_refs(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    source_kind: &str,
    source_id: &str,
    hashes: Vec<String>,
) -> Result<()> {
    let current = hashes.into_iter().collect::<BTreeSet<_>>();
    let previous = sqlx::query_scalar::<_, String>(
        "SELECT blob_hash FROM public_blob_refs WHERE source_kind = ?1 AND source_id = ?2",
    )
    .bind(source_kind)
    .bind(source_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    if previous == current {
        return Ok(());
    }
    sqlx::query("DELETE FROM public_blob_refs WHERE source_kind = ?1 AND source_id = ?2")
        .bind(source_kind)
        .bind(source_id)
        .execute(&mut **tx)
        .await?;
    for hash in &current {
        sqlx::query(
            "INSERT INTO public_blob_refs (source_kind, source_id, blob_hash) VALUES (?1, ?2, ?3)",
        )
        .bind(source_kind)
        .bind(source_id)
        .bind(hash)
        .execute(&mut **tx)
        .await?;
    }
    let now = now_ms()?;
    for hash in previous.union(&current) {
        refresh_public_blob_announcement(tx, hash, now).await?;
    }
    Ok(())
}

/// 投稿 1 件の参照（本文・添付と、リンクプレビューの画像）を外す（取り下げ、projection の回収）。
pub(super) async fn forget_post_public_blob_refs(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    object_id: &str,
) -> Result<()> {
    replace_public_blob_refs(tx, "post", object_id, Vec::new()).await?;
    replace_public_blob_refs(tx, "link_preview", object_id, Vec::new()).await
}

/// hash の告知の予定を、公開参照と保持に合わせる。両方あれば予定に載せ（満杯なら優先の最も低いものと
/// 入れ替える）、どちらかが無ければ外す。載っている hash の次の時刻は変えない。
pub(super) async fn refresh_public_blob_announcement(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    hash: &str,
    now: i64,
) -> Result<()> {
    let public = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM public_blob_refs WHERE blob_hash = ?1)",
    )
    .bind(hash)
    .fetch_one(&mut **tx)
    .await?
        != 0;
    let held = if public {
        sqlx::query_scalar::<_, i64>(
            "SELECT last_used_at FROM remote_content_cache \
             WHERE kind = 'blob' AND cache_key = ?1 AND (is_protected = 1 OR last_used_at > ?2)",
        )
        .bind(hash)
        .bind(now - REMOTE_CACHE_UNUSED_MS)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        None
    };
    let Some(last_used_at) = held else {
        return forget_public_blob_announcement(tx, hash).await;
    };
    let own = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM remote_content_cache_protected_ref \
         WHERE ref_id = ?1 AND kind = 'blob' AND cache_key = ?2)",
    )
    .bind(format!("own_blob:{hash}"))
    .bind(hash)
    .fetch_one(&mut **tx)
    .await?;
    let updated = sqlx::query("UPDATE public_blob_announcements SET own = ?2 WHERE blob_hash = ?1")
        .bind(hash)
        .bind(own)
        .execute(&mut **tx)
        .await?;
    if updated.rows_affected() != 0 {
        return Ok(());
    }
    let scheduled = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM public_blob_announcements")
        .fetch_one(&mut **tx)
        .await?;
    if scheduled >= PUBLIC_BLOB_ANNOUNCEMENT_CAP {
        // 優先の最も低いもの: 本人の blob でなく、cache の利用が最も古いもの。予定は上限の件数しか読まない。
        let (lowest, lowest_own, lowest_used) = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT a.blob_hash, a.own, COALESCE(c.last_used_at, 0) FROM public_blob_announcements a \
             LEFT JOIN remote_content_cache c ON c.kind = 'blob' AND c.cache_key = a.blob_hash \
             ORDER BY a.own, COALESCE(c.last_used_at, 0), a.blob_hash LIMIT 1",
        )
        .fetch_one(&mut **tx)
        .await?;
        if (lowest_own, lowest_used) >= (own, last_used_at) {
            return Ok(());
        }
        forget_public_blob_announcement(tx, &lowest).await?;
    }
    sqlx::query(
        "INSERT INTO public_blob_announcements (blob_hash, own, next_at) VALUES (?1, ?2, ?3)",
    )
    .bind(hash)
    .bind(own)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// hash を告知の予定から外す（保持が無くなった、公開参照が無くなった）。
pub(super) async fn forget_public_blob_announcement(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    hash: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM public_blob_announcements WHERE blob_hash = ?1")
        .bind(hash)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 書込み lock を最初の文で取って始める transaction。読取で始めた transaction の書込みへの昇格は、
/// busy_timeout を待たずに失敗する（remote cache の書込みと同じ台帳の行で取る）。
pub(super) async fn begin_public_blob_write(
    store: &SqliteStore,
) -> Result<sqlx::Transaction<'static, Sqlite>> {
    let mut tx = store.pool.begin().await?;
    sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

impl SqliteStore {
    /// 検証済みの公開記録が参照する blob か。
    pub async fn is_public_blob(&self, hash: &str) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM public_blob_refs WHERE blob_hash = ?1)",
        )
        .bind(hash)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }

    /// 告知の時刻が来た hash を時刻の順に `limit` 件まで。無ければ、次に来る時刻。
    pub async fn due_public_blob_announcements(
        &self,
        now: i64,
        limit: usize,
    ) -> Result<(Vec<String>, Option<i64>)> {
        let due = sqlx::query_scalar::<_, String>(
            "SELECT blob_hash FROM public_blob_announcements WHERE next_at <= ?1 \
             ORDER BY next_at, blob_hash LIMIT ?2",
        )
        .bind(now)
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?;
        if !due.is_empty() {
            return Ok((due, None));
        }
        let next = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MIN(next_at) FROM public_blob_announcements",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok((due, next))
    }

    /// 告知の次の時刻を決める。`None` なら予定から外す。予定に無い hash は足さない。
    pub async fn reschedule_public_blob_announcement(
        &self,
        hash: &str,
        next_at: Option<i64>,
    ) -> Result<()> {
        match next_at {
            Some(next_at) => {
                sqlx::query(
                    "UPDATE public_blob_announcements SET next_at = ?2 WHERE blob_hash = ?1",
                )
                .bind(hash)
                .bind(next_at)
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM public_blob_announcements WHERE blob_hash = ?1")
                    .bind(hash)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// 告知を始め直すとき、予定のすべての時刻を `now` にする。前の告知の成功（restart の前・backup の
    /// 復元の前）は信頼しない。予定は上限の件数しか無く、告知する側が少しずつ進める。
    pub async fn restart_public_blob_announcements(&self, now: i64) -> Result<()> {
        sqlx::query("UPDATE public_blob_announcements SET next_at = ?1")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// 投稿のリンクプレビューの画像を、その投稿の公開参照として記録する。
    pub async fn note_link_preview_image(&self, object_id: &str, hash: &str) -> Result<()> {
        let mut tx = begin_public_blob_write(self).await?;
        replace_public_blob_refs(&mut tx, "link_preview", object_id, vec![hash.to_string()])
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 導入前の公開記録を、種類ごとに `limit` 行まで索引へ取り込む。すべて取り込み終えていれば `true`。
    pub async fn backfill_public_blob_refs_step(&self, limit: usize) -> Result<bool> {
        let limit = i64::try_from(limit)?;
        let pending = sqlx::query_as::<_, (String, i64)>(
            "SELECT kind, cursor FROM public_blob_ref_backfill ORDER BY kind",
        )
        .fetch_all(&self.pool)
        .await?;
        for (kind, cursor) in &pending {
            let mut tx = begin_public_blob_write(self).await?;
            let rows = backfill_page(&mut tx, kind, *cursor, limit).await?;
            let read = i64::try_from(rows.len())?;
            let last = rows.last().map_or(*cursor, |(rowid, _, _)| *rowid);
            for (_, source_id, hashes) in rows {
                replace_public_blob_refs(&mut tx, kind, &source_id, hashes).await?;
            }
            if read < limit {
                sqlx::query("DELETE FROM public_blob_ref_backfill WHERE kind = ?1")
                    .bind(kind)
                    .execute(&mut *tx)
                    .await?;
            } else {
                sqlx::query("UPDATE public_blob_ref_backfill SET cursor = ?2 WHERE kind = ?1")
                    .bind(kind)
                    .bind(last)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        Ok(pending.is_empty())
    }
}

/// 種類ごとの次の `limit` 行を rowid の順に読み、(rowid, 記録の id, 公開参照の hash) にする。
async fn backfill_page(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    kind: &str,
    cursor: i64,
    limit: i64,
) -> Result<Vec<(i64, String, Vec<String>)>> {
    Ok(match kind {
        "post" => sqlx::query_as::<_, (i64, String, String, Option<String>, String, Option<String>)>(
            "SELECT rowid, object_id, channel_id, source_blob_hash, attachments_json, repost_of_json \
             FROM object_index_cache WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|(rowid, object_id, channel, body, attachments, repost)| {
            let mut hashes = Vec::new();
            if channel == PUBLIC_CHANNEL {
                let attachments = serde_json::from_str::<Vec<AssetRef>>(&attachments)?;
                let repost = repost
                    .map(|repost| serde_json::from_str::<RepostSourceSnapshotV1>(&repost))
                    .transpose()?;
                hashes.extend(body);
                hashes.extend(
                    attachments
                        .iter()
                        .chain(repost.iter().flat_map(|repost| repost.attachments.iter()))
                        .map(|asset| asset.hash.as_str().to_string()),
                );
            }
            Ok((rowid, object_id, hashes))
        })
        .collect::<Result<_>>()?,
        "profile" => sqlx::query_as::<_, (i64, String, Option<String>)>(
            "SELECT rowid, pubkey, picture_blob_hash FROM profiles \
             WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|(rowid, pubkey, picture)| (rowid, pubkey, picture.into_iter().collect()))
        .collect(),
        "reaction" => sqlx::query_as::<_, (i64, String, String, Option<String>, Option<String>)>(
            "SELECT r.rowid, r.reaction_id, r.status, r.custom_asset_snapshot_json, o.channel_id \
             FROM reaction_cache r LEFT JOIN object_index_cache o ON o.object_id = r.target_object_id \
             WHERE r.rowid > ?1 ORDER BY r.rowid LIMIT ?2",
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|(rowid, reaction_id, status, snapshot, channel)| {
            let mut hashes = Vec::new();
            if status == "active" && channel.as_deref() == Some(PUBLIC_CHANNEL)
                && let Some(snapshot) = snapshot
            {
                let snapshot = serde_json::from_str::<CustomReactionAssetSnapshotV1>(&snapshot)?;
                hashes.push(snapshot.blob_hash.as_str().to_string());
            }
            Ok((rowid, reaction_id, hashes))
        })
        .collect::<Result<_>>()?,
        _ => anyhow::bail!("unknown public blob backfill kind {kind}"),
    })
}

/// custom reaction の asset の公開参照（公開 topic の有効な reaction だけ）を、reaction の行と同じ transaction で記録する。
pub(super) async fn sync_reaction_public_blob_refs(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    row: &ReactionProjectionRow,
) -> Result<()> {
    let public = sqlx::query_scalar::<_, Option<String>>(
        "SELECT channel_id FROM object_index_cache WHERE object_id = ?1",
    )
    .bind(row.target_object_id.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .flatten()
    .as_deref()
        == Some(PUBLIC_CHANNEL);
    let hashes = match (&row.status, &row.custom_asset_snapshot) {
        (ObjectStatus::Active, Some(snapshot)) if public => {
            vec![snapshot.blob_hash.as_str().to_string()]
        }
        _ => Vec::new(),
    };
    replace_public_blob_refs(tx, "reaction", row.reaction_id.as_str(), hashes).await
}

#[cfg(test)]
mod tests;
