//! 旧 iroh store の退役の台帳(#1221 R5-I)。
//!
//! kind ごとに位置と終端の時刻を持ち、1 回 128 対象以内で進める。`protected_migration`(R5-G)の終端の判定は
//! 表の全行を kind の数と比べるため、同じ表に相乗りしない。
//!
//! - `own_entries`: 旧 store の本人の docs entry を新しい store へ写す(呼出し元)。
//! - `pin_tags`: 旧 store の Dome の pin の tag と blob を新しい store へ写す(呼出し元)。
//! - `legacy_projection`: 旧同期で入った、課金の外の他人の projection 行を、最近のものは台帳へ移し(内容は呼出し元が
//!   旧 store から cache へ写す)、それ以外は回収する。本人の行には成人向けの hash の参照を置く。
//! - `adult_marker`: `legacy_projection` の後に、旧領域の移行まで保護していた成人向けの hash の行を、参照が残るものは
//!   保護を外し、残らないものは回収する。

use super::remote_cache::{delete_cache_item, now_ms};
use super::*;
use crate::row_mapping::row_to_object_projection;
use kukuri_core::PayloadRef;

pub const LEGACY_STORE_PAGE: usize = 128;
pub const LEGACY_STORE_KINDS: [&str; 4] = [
    "own_entries",
    "pin_tags",
    "legacy_projection",
    "adult_marker",
];

/// `legacy_projection` の 1 ページ。
#[derive(Clone, Debug)]
pub struct LegacyProjectionPage {
    /// 台帳へ移した行の本文・添付の blob。呼出し元が旧 store から cache へ写す。
    pub blobs: Vec<String>,
    pub cursor: String,
    pub done: bool,
}

impl SqliteStore {
    /// kind の位置と、終端へ達したか。
    pub async fn legacy_store_position(&self, kind: &str) -> Result<(String, bool)> {
        let row =
            sqlx::query("SELECT cursor, done_at FROM legacy_store_retirement WHERE kind = ?1")
                .bind(kind)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map_or((String::new(), false), |row| {
            (
                row.get("cursor"),
                row.get::<Option<i64>, _>("done_at").is_some(),
            )
        }))
    }

    pub async fn finish_legacy_store_page(
        &self,
        kind: &str,
        cursor: &str,
        done: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO legacy_store_retirement (kind, cursor, done_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(kind) DO UPDATE SET cursor = excluded.cursor, done_at = excluded.done_at",
        )
        .bind(kind)
        .bind(cursor)
        .bind(if done { Some(now_ms()?) } else { None })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 旧 store を退役させてよいか(ADR 0048 §7): writer の切替の時刻より後に保護移行の全 kind の終端の時刻があり、
    /// この台帳の全 kind が終端へ達している。
    pub async fn legacy_store_retirable(&self) -> Result<bool> {
        let (Some(caught_up), Some(switched)) = (
            self.protected_migration_caught_up_at().await?,
            self.writer_switched_at().await?,
        ) else {
            return Ok(false);
        };
        let done: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM legacy_store_retirement WHERE done_at IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(caught_up > switched && usize::try_from(done)? == LEGACY_STORE_KINDS.len())
    }

    /// 旧 store の無い account(新規・退役済み・復元)。保護移行を終えたものとし、新形式の writer へ切り替える。
    /// 戻り値は切替の時刻。
    pub async fn settle_without_legacy_store(&self) -> Result<Option<i64>> {
        let now = now_ms()?;
        for kind in PROTECTED_MIGRATION_KINDS {
            sqlx::query(
                "INSERT INTO protected_migration (kind, cursor, caught_up_at) VALUES (?1, '', ?2) \
                 ON CONFLICT(kind) DO UPDATE SET caught_up_at = COALESCE(caught_up_at, excluded.caught_up_at)",
            )
            .bind(kind)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        for kind in LEGACY_STORE_KINDS {
            sqlx::query(
                "INSERT INTO legacy_store_retirement (kind, cursor, done_at) VALUES (?1, '', ?2) \
                 ON CONFLICT(kind) DO UPDATE SET done_at = COALESCE(done_at, excluded.done_at)",
            )
            .bind(kind)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        self.switch_writer_if_migrated().await
    }

    /// projection 行を位置の後ろから 128 行読む。本人の行は成人向けの hash の参照を置き、台帳にある行はそのまま。
    /// 他人の旧い行は、`REMOTE_CACHE_UNUSED_MS` より新しく導いたものを、その時刻を最後の利用として台帳へ移し
    /// (入らなければ回収)、それ以外を回収する。
    pub async fn retire_legacy_projection_page(
        &self,
        local_pubkey: &str,
        cursor: &str,
    ) -> Result<LegacyProjectionPage> {
        let _gate = self.remote_cache_gate.lock().await;
        let budget =
            super::remote_cache::REMOTE_CACHE_CAPACITY_BYTES.saturating_sub(i64::try_from(
                self.remote_cache_reserved
                    .load(std::sync::atomic::Ordering::Acquire),
            )?);
        let now = now_ms()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE remote_content_cache_usage SET used_bytes = used_bytes WHERE id = 1")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(
            "SELECT rowid, object_id, topic_id, author_pubkey, created_at, object_kind, root_object_id, \
             reply_to_object_id, channel_id, payload_ref_json, content, attachments_json, repost_of_json, \
             content_labels_json, source_replica_id, source_key, source_envelope_id, source_blob_hash, \
             derived_at, projection_version, source_docs_author \
             FROM object_index_cache WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        )
        .bind(cursor.parse::<i64>().unwrap_or(0))
        .bind(i64::try_from(LEGACY_STORE_PAGE)?)
        .fetch_all(&mut *tx)
        .await?;
        let scanned = rows.len();
        let next = rows.last().map_or(cursor.to_string(), |row| {
            row.get::<i64, _>("rowid").to_string()
        });
        let mut blobs = Vec::new();
        let mut label_evictions = Vec::new();
        let mut removed_files = Vec::new();
        for row in rows {
            let row = row_to_object_projection(row)?;
            if row.author_pubkey == local_pubkey {
                for hash in adult_media_hashes_for_row(&row) {
                    sqlx::query(
                        "INSERT OR IGNORE INTO remote_adult_media_hash_refs (object_id, blob_hash) \
                         VALUES (?1, ?2)",
                    )
                    .bind(row.object_id.as_str())
                    .bind(hash)
                    .execute(&mut *tx)
                    .await?;
                }
                continue;
            }
            let charged = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM remote_content_cache \
                 WHERE kind = 'projection' AND cache_key = ?1)",
            )
            .bind(row.object_id.as_str())
            .fetch_one(&mut *tx)
            .await?
                != 0;
            if charged {
                continue;
            }
            let recent = row.derived_at > now - super::remote_cache::REMOTE_CACHE_UNUSED_MS;
            if recent
                && self
                    .charge_remote_projection(
                        &mut tx,
                        &row,
                        budget,
                        row.derived_at,
                        &mut label_evictions,
                        &mut removed_files,
                    )
                    .await?
            {
                self.sync_remote_adult_hash_refs(&mut tx, &row, &mut label_evictions)
                    .await?;
                if let PayloadRef::BlobText { hash, .. } = &row.payload_ref {
                    blobs.push(hash.as_str().to_string());
                }
                blobs.extend(
                    row.attachments
                        .iter()
                        .chain(row.repost_of.iter().flat_map(|repost| &repost.attachments))
                        .map(|asset| asset.hash.as_str().to_string()),
                );
            } else {
                delete_cache_item(
                    &mut tx,
                    "projection",
                    row.object_id.as_str(),
                    &mut label_evictions,
                    &mut removed_files,
                )
                .await?;
            }
        }
        tx.commit().await?;
        self.publish_adult_label_evictions(label_evictions);
        self.remove_remote_blob_files(removed_files).await?;
        Ok(LegacyProjectionPage {
            blobs,
            cursor: next,
            done: scanned < LEGACY_STORE_PAGE,
        })
    }

    /// 成人向けの hash の行を位置の後ろから 128 行読み、旧領域の移行まで保護していた行を、参照が残れば非保護へ、
    /// 残らなければ回収する。
    pub async fn retire_legacy_adult_marker_page(&self, cursor: &str) -> Result<(String, bool)> {
        let _gate = self.remote_cache_gate.lock().await;
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            "SELECT rowid, blob_hash, is_protected FROM adult_media_hashes \
             WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        )
        .bind(cursor.parse::<i64>().unwrap_or(0))
        .bind(i64::try_from(LEGACY_STORE_PAGE)?)
        .fetch_all(&mut *tx)
        .await?;
        let next = rows.last().map_or(cursor.to_string(), |row| {
            row.get::<i64, _>("rowid").to_string()
        });
        let mut evicted = Vec::new();
        for row in &rows {
            if row.get::<i64, _>("is_protected") == 0 {
                continue;
            }
            let hash: String = row.get("blob_hash");
            let referenced = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM remote_adult_media_hash_refs WHERE blob_hash = ?1)",
            )
            .bind(&hash)
            .fetch_one(&mut *tx)
            .await?
                != 0;
            if referenced {
                sqlx::query("UPDATE adult_media_hashes SET is_protected = 0 WHERE blob_hash = ?1")
                    .bind(&hash)
                    .execute(&mut *tx)
                    .await?;
            } else {
                sqlx::query("DELETE FROM adult_media_hashes WHERE blob_hash = ?1")
                    .bind(&hash)
                    .execute(&mut *tx)
                    .await?;
                evicted.push(hash);
            }
        }
        tx.commit().await?;
        self.publish_adult_label_evictions(evicted);
        Ok((next, rows.len() < LEGACY_STORE_PAGE))
    }
}
