//! relation の観測 = 2 者間のアクション（#1221 R5-E、ADR 0026 §2.4。2026-09-26 ユーザー決定）。
//!
//! 取込みが観測した返信・repost・引用・リアクション・フォローを `cn_index.relation_actions` に 1 行ずつ保存する。
//! 行の追加・削除は trigger がペアの方向別の件数と共有 topic 数、author の参加数へ差分反映し、変化したペアと
//! author に印（`dirty_seq`）を付ける。解析は印の付いた行だけを古い順に上限つきで読み、処理した印だけを外す。
//! public topic 由来だけを扱い、private channel は入れない（取込み側が public topic のときだけ記録する）。

use anyhow::Result;
use sqlx::{PgPool, Row};

/// アクションの種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationActionKind {
    Reply,
    /// repost と引用（`repost_of` を持つ投稿）。
    Repost,
    Reaction,
    Follow,
}

impl RelationActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Repost => "repost",
            Self::Reaction => "reaction",
            Self::Follow => "follow",
        }
    }
}

/// actor から target への 1 件のアクション。`scope_id` と `anchor_object_id` はフォロー以外で必須。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationAction {
    pub kind: RelationActionKind,
    pub source_id: String,
    pub actor_pubkey: String,
    pub target_pubkey: String,
    pub scope_id: Option<String>,
    /// この public topic の索引行が消えると、このアクションも消える。
    pub anchor_object_id: Option<String>,
    /// 受入下限と比べる時刻（起点の投稿の作成時刻、フォローは観測した時刻。#1221 R5-F）。
    pub created_at: i64,
}

impl RelationAction {
    pub fn follow(actor_pubkey: &str, target_pubkey: &str) -> Self {
        Self {
            kind: RelationActionKind::Follow,
            source_id: format!("{actor_pubkey}:{target_pubkey}"),
            actor_pubkey: actor_pubkey.to_string(),
            target_pubkey: target_pubkey.to_string(),
            scope_id: None,
            anchor_object_id: None,
            created_at: chrono::Utc::now().timestamp(),
        }
    }
}

/// アクションを保存する。新しく保存したら true（同じ由来の行が既にあれば何もしない）。自分自身へのアクションは保存しない。
pub async fn record_relation_action(pool: &PgPool, action: &RelationAction) -> Result<bool> {
    if action.actor_pubkey == action.target_pubkey {
        return Ok(false);
    }
    let result = sqlx::query(
        "INSERT INTO cn_index.relation_actions
             (kind, source_id, actor_pubkey, target_pubkey, scope_id, anchor_object_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (kind, source_id) DO NOTHING",
    )
    .bind(action.kind.as_str())
    .bind(&action.source_id)
    .bind(&action.actor_pubkey)
    .bind(&action.target_pubkey)
    .bind(&action.scope_id)
    .bind(&action.anchor_object_id)
    .bind(action.created_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn remove_relation_action(
    pool: &PgPool,
    kind: RelationActionKind,
    source_id: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM cn_index.relation_actions WHERE kind = $1 AND source_id = $2")
        .bind(kind.as_str())
        .bind(source_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn relation_action_exists(
    pool: &PgPool,
    kind: RelationActionKind,
    source_id: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM cn_index.relation_actions WHERE kind = $1 AND source_id = $2)",
    )
    .bind(kind.as_str())
    .bind(source_id)
    .fetch_one(pool)
    .await?)
}

/// public topic の索引にある投稿の著者と作成時刻（返信先・リアクション先の解決）。索引に無ければ None。
pub async fn indexed_public_author(
    pool: &PgPool,
    scope_id: &str,
    object_id: &str,
) -> Result<Option<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT author_pubkey, created_at FROM cn_index.index_entries
         WHERE scope_kind = 'public_topic' AND scope_id = $1 AND object_id = $2",
    )
    .bind(scope_id)
    .bind(object_id)
    .fetch_optional(pool)
    .await?)
}

/// 印の付いたペア。`a_to_b` と `b_to_a` が両方 1 以上で、フォロー以外のアクション（public topic のアクション）が
/// 1 件以上あるときだけ edge を作る（相互フォローだけのペアは作らない。2026-09-26 ユーザー決定）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationPairChange {
    pub author_a: String,
    pub author_b: String,
    pub a_to_b: i64,
    pub b_to_a: i64,
    pub shared_topics: i64,
    pub dirty_seq: i64,
}

impl RelationPairChange {
    pub fn is_mutual(&self) -> bool {
        self.a_to_b > 0 && self.b_to_a > 0 && self.shared_topics > 0
    }
}

/// 印の付いた author と、その dominant topic（最多の索引件数、同数は scope_id の辞書順）。参加 0 なら None。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationAuthorChange {
    pub author_pubkey: String,
    pub dominant_topic: Option<String>,
    pub dirty_seq: i64,
}

pub async fn dirty_relation_pairs(pool: &PgPool, limit: usize) -> Result<Vec<RelationPairChange>> {
    let rows = sqlx::query(
        "SELECT author_a, author_b, a_to_b, b_to_a, shared_topics, dirty_seq
         FROM cn_index.relation_pairs WHERE dirty_seq IS NOT NULL
         ORDER BY dirty_seq LIMIT $1",
    )
    .bind(i64::try_from(limit)?)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(RelationPairChange {
                author_a: row.try_get("author_a")?,
                author_b: row.try_get("author_b")?,
                a_to_b: row.try_get("a_to_b")?,
                b_to_a: row.try_get("b_to_a")?,
                shared_topics: row.try_get("shared_topics")?,
                dirty_seq: row.try_get("dirty_seq")?,
            })
        })
        .collect()
}

/// 反映したペアの印を外す。読んだ後に変化していれば（印が新しい）印を残し、次の解析でもう一度読む。
/// アクションが無くなったペアの行は消す。
pub async fn settle_relation_pair(pool: &PgPool, change: &RelationPairChange) -> Result<()> {
    sqlx::query(
        "DELETE FROM cn_index.relation_pairs
         WHERE author_a = $1 AND author_b = $2 AND dirty_seq = $3 AND a_to_b = 0 AND b_to_a = 0",
    )
    .bind(&change.author_a)
    .bind(&change.author_b)
    .bind(change.dirty_seq)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE cn_index.relation_pairs SET dirty_seq = NULL
         WHERE author_a = $1 AND author_b = $2 AND dirty_seq = $3",
    )
    .bind(&change.author_a)
    .bind(&change.author_b)
    .bind(change.dirty_seq)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn dirty_relation_authors(
    pool: &PgPool,
    limit: usize,
) -> Result<Vec<RelationAuthorChange>> {
    let rows = sqlx::query(
        "SELECT d.author_pubkey, d.dirty_seq, (
             SELECT p.scope_id FROM cn_index.relation_participation p
             WHERE p.author_pubkey = d.author_pubkey
             ORDER BY p.entries DESC, p.scope_id LIMIT 1
         ) AS dominant_topic
         FROM cn_index.relation_dirty_authors d
         ORDER BY d.dirty_seq LIMIT $1",
    )
    .bind(i64::try_from(limit)?)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(RelationAuthorChange {
                author_pubkey: row.try_get("author_pubkey")?,
                dominant_topic: row.try_get("dominant_topic")?,
                dirty_seq: row.try_get("dirty_seq")?,
            })
        })
        .collect()
}

pub async fn settle_relation_author(pool: &PgPool, change: &RelationAuthorChange) -> Result<()> {
    sqlx::query(
        "DELETE FROM cn_index.relation_dirty_authors WHERE author_pubkey = $1 AND dirty_seq = $2",
    )
    .bind(&change.author_pubkey)
    .bind(change.dirty_seq)
    .execute(pool)
    .await?;
    Ok(())
}
