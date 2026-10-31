//! #1998 AC-3: ゲームのアカウント A を既存のアカウント B へまとめたとき、A のフォロー・ブロック・ミュートを B で付け直す
//! （ADR 0070 §4.2）。A の DB を読取り専用で開き、相手の公開鍵の順に 1 page ずつ進める。進んだ位置の保存と再開は呼び出し側
//! （SDK の profile）が持つ。

use std::path::Path;

use anyhow::Result;
use kukuri_core::{BlockEdgeStatus, FollowEdgeStatus};
use kukuri_store::{SqliteStore, Store};
use serde::{Deserialize, Serialize};

use super::DesktopRuntime;
use crate::requests::AuthorRequest;

const PAGE: usize = 64;

/// 付け直す対象の種類。この順に進める。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocialCopyKind {
    Follows,
    Blocks,
    Mutes,
}

impl SocialCopyKind {
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Follows => Some(Self::Blocks),
            Self::Blocks => Some(Self::Mutes),
            Self::Mutes => None,
        }
    }
}

/// 付け直しの元（A の DB）。
pub struct SocialCopySource {
    store: SqliteStore,
    pubkey: String,
}

impl SocialCopySource {
    pub async fn open(db: &Path, pubkey: &str) -> Result<Self> {
        Ok(Self {
            store: SqliteStore::connect_file_read_only(db).await?,
            pubkey: pubkey.to_owned(),
        })
    }

    /// `after` より後の相手を 1 page。有効なもの（解除していないもの）かどうかを添える。
    async fn page(&self, kind: SocialCopyKind, after: Option<&str>) -> Result<Vec<(String, bool)>> {
        let subject = self.pubkey.as_str();
        Ok(match kind {
            SocialCopyKind::Follows => self
                .store
                .list_follow_edges_by_subject_after(subject, after, PAGE)
                .await?
                .into_iter()
                .map(|edge| {
                    (
                        edge.target_pubkey.0,
                        edge.status == FollowEdgeStatus::Active,
                    )
                })
                .collect(),
            SocialCopyKind::Blocks => self
                .store
                .list_block_edges_by_subject_after(subject, after, PAGE)
                .await?
                .into_iter()
                .map(|edge| (edge.target_pubkey.0, edge.status == BlockEdgeStatus::Active))
                .collect(),
            SocialCopyKind::Mutes => self
                .store
                .list_muted_authors_after(after, PAGE)
                .await?
                .into_iter()
                .map(|author| (author, true))
                .collect(),
        })
    }
}

impl DesktopRuntime {
    /// 移行で受けた必須 bundle の反映が終わったか（置き場が無い）。
    pub async fn account_transfer_merged(&self) -> Result<bool> {
        crate::accounts::transfer::merged(&self.db_path).await
    }

    /// `after` の次の 1 page を付け直し、最後に読んだ相手を返す（読む相手が無ければ `None`）。相手が自分、解除済み、
    /// 公開鍵の形が不正、または自分に同じ相手の記録が（解除済みを含め）ある行は飛ばす。付け直しの失敗はそのまま返す。
    pub async fn copy_social_page(
        &self,
        source: &SocialCopySource,
        kind: SocialCopyKind,
        after: Option<&str>,
    ) -> Result<Option<String>> {
        let me = self.local_author_pubkey();
        let rows = source.page(kind, after).await?;
        for (target, active) in &rows {
            let valid = target.len() == 64 && target.chars().all(|c| c.is_ascii_hexdigit());
            if !active
                || !valid
                || *target == me
                || self.has_social_record(kind, &me, target).await?
            {
                continue;
            }
            let request = AuthorRequest {
                pubkey: target.clone(),
            };
            match kind {
                SocialCopyKind::Follows => self.follow_author(request).await?,
                SocialCopyKind::Blocks => self.block_author(request).await?,
                SocialCopyKind::Mutes => self.mute_author(request).await?,
            };
        }
        Ok(rows.into_iter().last().map(|(target, _)| target))
    }

    async fn has_social_record(
        &self,
        kind: SocialCopyKind,
        me: &str,
        target: &str,
    ) -> Result<bool> {
        Ok(match kind {
            SocialCopyKind::Follows => self.store.get_follow_edge(me, target).await?.is_some(),
            SocialCopyKind::Blocks => self.store.get_block_edge(me, target).await?.is_some(),
            SocialCopyKind::Mutes => self.store.get_muted_author(target).await?.is_some(),
        })
    }
}
