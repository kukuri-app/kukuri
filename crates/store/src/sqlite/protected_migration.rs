//! 旧 `iroh-data` にしか無い本人のデータを、保護所有先(remote cache と保護参照)へ移す台帳(#1221 R5-G)。
//!
//! kind ごとに index の保存した位置の後ろを 1 回 128 行まで読み、呼出し元が旧領域から写した内容に保護参照を付ける。
//! 位置は呼出し元が写し終えてから進める。途中で止まれば同じ位置から読み直し、同じ結果になる(参照の置き換えと
//! 内容の書込みは冪等)。

use super::remote_cache::now_ms;
use super::*;
use kukuri_core::DirectMessageAttachmentManifestV1;

/// 1 回に読む index 行の上限。
pub const PROTECTED_MIGRATION_PAGE: usize = 128;

/// 移行する kind。`private` と `dome_pin` は呼出し元が旧領域の索引(capability の一覧・blob の tag)から読む。
pub const PROTECTED_MIGRATION_KINDS: [&str; 10] = [
    "own_envelope",
    "bookmark",
    "reaction_bookmark",
    "dm_outbox",
    "dm_message",
    "live_session",
    "game_room",
    "avatar",
    "private",
    "dome_pin",
];

/// 候補の保護参照の元。呼出し元は、これから旧領域の依存 record と追加の blob を求める。
#[derive(Clone, Debug)]
pub enum ProtectedSource {
    /// `blobs` だけを守る。
    Blobs,
    /// 本人の署名つき envelope(投稿・custom reaction asset)。
    Envelope(KukuriEnvelope),
    /// 送信待ちの DM。`blobs[0]` が frame で、暗号化添付の hash は frame の中にある。
    DirectMessageFrame,
    /// live・game の state。envelope の署名者が本人のときだけ守る。
    Session {
        replica: ReplicaId,
        state_key: String,
    },
}

#[derive(Clone, Debug)]
pub struct ProtectedCandidate {
    pub reference: String,
    pub blobs: Vec<String>,
    pub source: ProtectedSource,
}

#[derive(Clone, Debug)]
pub struct ProtectedMigrationPage {
    pub candidates: Vec<ProtectedCandidate>,
    /// 写し終えたら保存する位置。
    pub cursor: String,
    /// index の終端へ達した。
    pub done: bool,
}

fn blob_candidate(reference: String, blobs: Vec<String>) -> ProtectedCandidate {
    ProtectedCandidate {
        reference,
        blobs,
        source: ProtectedSource::Blobs,
    }
}

impl SqliteStore {
    pub async fn protected_migration_cursor(&self, kind: &str) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT cursor FROM protected_migration WHERE kind = ?1")
                .bind(kind)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or_default(),
        )
    }

    /// 保存した位置の後ろの index 行を最大 128 行読む。本人の判定が要る kind は `local_pubkey` で選ぶ。
    pub async fn protected_migration_page(
        &self,
        kind: &str,
        local_pubkey: &str,
    ) -> Result<ProtectedMigrationPage> {
        let cursor = self.protected_migration_cursor(kind).await?;
        let limit = i64::try_from(PROTECTED_MIGRATION_PAGE)?;
        let rowid = cursor.parse::<i64>().unwrap_or(0);
        let rowid_page = |sql: &'static str| {
            sqlx::query(sql)
                .bind(rowid)
                .bind(limit)
                .fetch_all(&self.pool)
        };
        let mut candidates = Vec::new();
        let last_rowid = |rows: &[sqlx::sqlite::SqliteRow]| {
            (
                rows.last()
                    .map(|row| row.get::<i64, _>("rowid").to_string()),
                rows.len(),
            )
        };
        let (next, scanned) = match kind {
            "own_envelope" => {
                let rows = rowid_page(
                    "SELECT rowid, envelope_id, pubkey, created_at, kind, content, tags_json, sig \
                     FROM envelopes WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
                )
                .await?;
                let position = last_rowid(&rows);
                for row in rows {
                    if row.get::<String, _>("pubkey") == local_pubkey {
                        let envelope = row_to_envelope(row)?;
                        candidates.push(ProtectedCandidate {
                            reference: format!("own:{}", envelope.id.as_str()),
                            blobs: Vec::new(),
                            source: ProtectedSource::Envelope(envelope),
                        });
                    }
                }
                position
            }
            "bookmark" => {
                let rows = rowid_page(
                    "SELECT rowid, source_object_id, source_envelope_id, source_replica_id, topic_id, \
                     channel_id, author_pubkey, created_at, object_kind, payload_ref_json, content, \
                     attachments_json, reply_to_object_id, root_object_id, repost_of_json, bookmarked_at \
                     FROM bookmarked_posts WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
                )
                .await?;
                let position = last_rowid(&rows);
                for row in rows {
                    let bookmark = row_to_bookmarked_post(row)?;
                    candidates.push(blob_candidate(
                        format!("bookmark:{}", bookmark.source_object_id.as_str()),
                        super::bookmarks::bookmark_cache_refs(&bookmark)
                            .into_iter()
                            .map(|(_, hash)| hash)
                            .collect(),
                    ));
                }
                position
            }
            "reaction_bookmark" => {
                let rows = rowid_page(
                    "SELECT rowid, asset_id, blob_hash FROM bookmarked_custom_reactions \
                     WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
                )
                .await?;
                for row in &rows {
                    candidates.push(blob_candidate(
                        format!("reaction_bookmark:{}", row.get::<String, _>("asset_id")),
                        vec![row.get("blob_hash")],
                    ));
                }
                last_rowid(&rows)
            }
            "dm_message" => {
                let rows = rowid_page(
                    "SELECT rowid, dm_id, message_id, attachment_manifest_json FROM dm_messages \
                     WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
                )
                .await?;
                for row in &rows {
                    let Some(manifest) = row
                        .get::<Option<String>, _>("attachment_manifest_json")
                        .filter(|value| !value.trim().is_empty())
                    else {
                        continue;
                    };
                    let Ok(manifest) =
                        serde_json::from_str::<DirectMessageAttachmentManifestV1>(&manifest)
                    else {
                        continue;
                    };
                    candidates.push(blob_candidate(
                        format!(
                            "dm_message:{}/{}",
                            row.get::<String, _>("dm_id"),
                            row.get::<String, _>("message_id")
                        ),
                        std::iter::once(&manifest.original)
                            .chain(manifest.poster.as_ref())
                            .map(|blob| blob.hash.as_str().to_string())
                            .collect(),
                    ));
                }
                last_rowid(&rows)
            }
            "dm_outbox" => {
                let (created_at, message_id, dm_id) =
                    serde_json::from_str::<(i64, String, String)>(&cursor).unwrap_or((
                        i64::MIN,
                        String::new(),
                        String::new(),
                    ));
                let rows = sqlx::query(
                    "SELECT dm_id, message_id, frame_blob_hash, created_at FROM dm_outbox \
                     WHERE (created_at, message_id, dm_id) > (?1, ?2, ?3) \
                     ORDER BY created_at, message_id, dm_id LIMIT ?4",
                )
                .bind(created_at)
                .bind(message_id)
                .bind(dm_id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
                for row in &rows {
                    candidates.push(ProtectedCandidate {
                        reference: format!(
                            "dm_outbox:{}/{}",
                            row.get::<String, _>("dm_id"),
                            row.get::<String, _>("message_id")
                        ),
                        blobs: vec![row.get("frame_blob_hash")],
                        source: ProtectedSource::DirectMessageFrame,
                    });
                }
                let next = rows
                    .last()
                    .map(|row| {
                        serde_json::to_string(&(
                            row.get::<i64, _>("created_at"),
                            row.get::<String, _>("message_id"),
                            row.get::<String, _>("dm_id"),
                        ))
                    })
                    .transpose()?;
                (next, rows.len())
            }
            "live_session" | "game_room" => {
                let (table, id, prefix) = if kind == "live_session" {
                    ("live_session_cache", "session_id", "live")
                } else {
                    ("game_room_cache", "room_id", "game")
                };
                let (derived_at, last_id) = serde_json::from_str::<(i64, String)>(&cursor)
                    .unwrap_or((i64::MIN, String::new()));
                let rows = sqlx::query(&format!(
                    "SELECT {id} AS id, derived_at, source_replica_id, source_key, manifest_blob_hash \
                     FROM {table} WHERE (derived_at, {id}) > (?1, ?2) ORDER BY derived_at, {id} LIMIT ?3"
                ))
                .bind(derived_at)
                .bind(last_id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
                for row in &rows {
                    candidates.push(ProtectedCandidate {
                        reference: format!("{prefix}:{}", row.get::<String, _>("id")),
                        blobs: vec![row.get("manifest_blob_hash")],
                        source: ProtectedSource::Session {
                            replica: ReplicaId::new(row.get::<String, _>("source_replica_id")),
                            state_key: row.get("source_key"),
                        },
                    });
                }
                let next = rows
                    .last()
                    .map(|row| {
                        serde_json::to_string(&(
                            row.get::<i64, _>("derived_at"),
                            row.get::<String, _>("id"),
                        ))
                    })
                    .transpose()?;
                (next, rows.len())
            }
            "avatar" => {
                // 本人の行は 1 行だけ。写した hash を位置に置き、変わったら参照を置き換える。
                let hash = sqlx::query_scalar::<_, Option<String>>(
                    "SELECT picture_blob_hash FROM profiles WHERE pubkey = ?1",
                )
                .bind(local_pubkey)
                .fetch_optional(&self.pool)
                .await?
                .flatten()
                .unwrap_or_default();
                if hash != cursor {
                    candidates.push(blob_candidate(
                        format!("avatar:{local_pubkey}"),
                        (!hash.is_empty())
                            .then(|| hash.clone())
                            .into_iter()
                            .collect(),
                    ));
                }
                return Ok(ProtectedMigrationPage {
                    candidates,
                    cursor: hash,
                    done: true,
                });
            }
            _ => anyhow::bail!("protected migration kind `{kind}` is not read from the store"),
        };
        Ok(ProtectedMigrationPage {
            candidates,
            cursor: next.unwrap_or(cursor),
            done: scanned < PROTECTED_MIGRATION_PAGE,
        })
    }

    /// 写し終えた位置を保存する。終端へ達したときは、その時刻を残す。
    pub async fn finish_protected_migration_page(
        &self,
        kind: &str,
        cursor: &str,
        done: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO protected_migration (kind, cursor, caught_up_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(kind) DO UPDATE SET cursor = excluded.cursor, caught_up_at = excluded.caught_up_at",
        )
        .bind(kind)
        .bind(cursor)
        .bind(if done { Some(now_ms()?) } else { None })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 索引の順に並ばない kind(private の参加状態)を、索引が変わったときに先頭から読み直させる。
    pub async fn reset_protected_migration(&self, kind: &str) -> Result<()> {
        self.finish_protected_migration_page(kind, "", false).await
    }

    /// 全 kind が終端へ達していれば、その中で最も古い時刻。旧領域を削除できるのは、この時刻が writer の切替
    /// (R5-H)の永続化より後のときだけ。
    pub async fn protected_migration_caught_up_at(&self) -> Result<Option<i64>> {
        caught_up_at(&mut *self.pool.acquire().await?).await
    }

    /// 停止した account の SQLite を 1 本の接続で読み、backup に含める保護された file の名前
    /// (= blob hash)を返す。移行が終端へ達していなければ `None`。pool を使わず、WAL を畳んで閉じ終えてから返す
    /// (読取り専用の接続は -wal・-shm を残す)。
    pub async fn read_protected_backup_files(path: &Path) -> Result<Option<Vec<String>>> {
        use sqlx::Connection;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false);
        let mut connection = sqlx::SqliteConnection::connect_with(&options).await?;
        let result = async {
            if caught_up_at(&mut connection).await?.is_none() {
                return Ok(None);
            }
            Ok(Some(
                sqlx::query_scalar(
                    "SELECT file_name FROM remote_content_cache \
                     WHERE is_protected = 1 AND file_name IS NOT NULL ORDER BY file_name",
                )
                .fetch_all(&mut connection)
                .await?,
            ))
        }
        .await;
        connection.close().await?;
        result
    }

    /// `reference` の保護参照を `desired` に置き換える。index 行が既に消えていれば参照を外し、`false` を返す
    /// (移行と bookmark の解除・ACK が競合しても、消えた行の内容を守り続けない)。
    pub async fn set_protected_refs(
        &self,
        reference: &str,
        desired: &[(String, String)],
    ) -> Result<bool> {
        let mut update = self.begin_protected_ref_update().await?;
        let exists = protected_source_exists(&mut update.tx, reference).await?;
        self.set_refs_in(&mut update, reference, if exists { desired } else { &[] })
            .await?;
        self.commit_protected_ref_update(update).await?;
        Ok(exists)
    }
}

async fn caught_up_at(connection: &mut sqlx::SqliteConnection) -> Result<Option<i64>> {
    let row = sqlx::query(
        "SELECT COUNT(caught_up_at) AS caught_up, MIN(caught_up_at) AS oldest FROM protected_migration",
    )
    .fetch_one(connection)
    .await?;
    Ok(
        (usize::try_from(row.get::<i64, _>("caught_up"))? == PROTECTED_MIGRATION_KINDS.len())
            .then(|| row.get("oldest")),
    )
}

async fn protected_source_exists(
    tx: &mut sqlx::Transaction<'static, Sqlite>,
    reference: &str,
) -> Result<bool> {
    let (sql, first, second) = match reference.split_once(':') {
        Some(("bookmark", id)) => (
            "SELECT EXISTS(SELECT 1 FROM bookmarked_posts WHERE source_object_id = ?1 AND ?2 = '')",
            id,
            "",
        ),
        Some(("reaction_bookmark", id)) => (
            "SELECT EXISTS(SELECT 1 FROM bookmarked_custom_reactions WHERE asset_id = ?1 AND ?2 = '')",
            id,
            "",
        ),
        Some(("dm_outbox", pair)) => (
            "SELECT EXISTS(SELECT 1 FROM dm_outbox WHERE dm_id = ?1 AND message_id = ?2)",
            pair.split_once('/').map_or(pair, |(dm, _)| dm),
            pair.split_once('/').map_or("", |(_, message)| message),
        ),
        Some(("dm_message", pair)) => (
            "SELECT EXISTS(SELECT 1 FROM dm_messages WHERE dm_id = ?1 AND message_id = ?2)",
            pair.split_once('/').map_or(pair, |(dm, _)| dm),
            pair.split_once('/').map_or("", |(_, message)| message),
        ),
        _ => return Ok(true),
    };
    Ok(sqlx::query_scalar::<_, i64>(sql)
        .bind(first)
        .bind(second)
        .fetch_one(&mut **tx)
        .await?
        != 0)
}
