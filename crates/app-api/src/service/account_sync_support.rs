//! 本人の端末間の account 同期の item の書き込み・点読・merge（#1218 AC-3、ADR 0061 §4）。
//!
//! 採用した状態は item ごとに 1 行（`AccountSyncStore`）で、`(updated_at, op_id)` が大きいものだけで置き換える。
//! 読むのは account の docs author の組の 1 件と、それが無いときの key ごとの上限つきの旧候補だけで、replica の
//! 全件・全 snapshot を比べない。hint・起動・復帰で差分を取りに行く経路は `account_sync_fetch`（AC-5b）。

use super::*;
use kukuri_core::{AccountSyncItem, AccountSyncItemKey, SealedAccountSyncItem};
use kukuri_store::AccountSyncRow;

/// 著者を常に表示する指定の docs の key の prefix（ADR 0061 §2）。
const TRUST_ALWAYS_VISIBLE_PREFIX: &str = "trust/always-visible/";
/// 組の record が無い・開けないときに読む、key ごとの旧候補の上限（ADR 0053 §6 と同じ）。
const ACCOUNT_SYNC_RECORDS_PER_KEY: usize = 8;

fn trust_always_visible_item(
    author: &str,
    updated_at: i64,
    visible: bool,
) -> Result<AccountSyncItem> {
    Ok(AccountSyncItem::edit(
        AccountSyncItemKey::TrustAlwaysVisible {
            author: Pubkey::from(normalize_author_pubkey(author)?),
        },
        updated_at,
        visible.then_some(serde_json::Value::Bool(true)),
    ))
}

pub(super) fn row_of(item: &AccountSyncItem) -> Result<AccountSyncRow> {
    Ok(AccountSyncRow {
        key: item.key.docs_key(),
        op_id: item.op_id.clone(),
        updated_at: item.updated_at,
        value: item.value.as_ref().map(serde_json::to_string).transpose()?,
    })
}

impl AppService {
    /// この端末の編集を採用し、封をして account の replica へ書く。採用しなかった（同じか新しい状態がある）なら書かない。
    /// replica へ書けなくても、この端末の採用は戻さない（送り直しは `account_sync_fetch`）。
    pub(crate) async fn publish_account_sync_item(&self, item: AccountSyncItem) -> Result<()> {
        if !self
            .services
            .projection_store
            .adopt_account_sync_row(&row_of(&item)?)
            .await?
        {
            return Ok(());
        }
        self.write_account_sync_item(&item).await
    }

    pub(crate) async fn publish_profile_item(&self, envelope: &KukuriEnvelope) -> Result<()> {
        self.publish_account_sync_item(AccountSyncItem::profile(envelope)?)
            .await
    }

    /// item を点読する。account の docs author の組の 1 件を読み、無い・開けないときだけ key の旧候補（上限 8 件）から
    /// 最大の版を採る。
    #[cfg(test)]
    pub(crate) async fn read_account_sync_item(
        &self,
        key: &AccountSyncItemKey,
        policy: DocFetchPolicy,
    ) -> Result<Option<AccountSyncItem>> {
        self.read_account_sync_docs_key(&key.docs_key(), policy)
            .await
    }

    /// 手元の replica から docs の key で item を点読する。
    pub(crate) async fn read_account_sync_docs_key(
        &self,
        docs_key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<AccountSyncItem>> {
        self.read_account_sync_record(self.services.docs_sync.as_ref(), docs_key, policy)
            .await
    }

    /// `source`（手元の docs か、本人の別の端末の reader）から、account の docs author の組の 1 件を読み、無い・開け
    /// ないときだけ key の旧候補（上限 8 件）から最大の版を採る。全端末が同じ docs author で書く（ADR 0053）。
    pub(crate) async fn read_account_sync_record(
        &self,
        source: &dyn DocsSync,
        docs_key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<AccountSyncItem>> {
        let keys = self.services.keys.derive_account_sync();
        let account = self.services.keys.public_key();
        let docs_key = docs_key.to_string();
        let open = |record: &DocRecord| {
            serde_json::from_slice::<SealedAccountSyncItem>(&record.value)
                .ok()
                .and_then(|sealed| keys.open(&account, &docs_key, &sealed).ok())
        };
        if let Some(docs_author) = self.services.docs_sync.local_docs_author().await?
            && let Some(record) = source
                .query_replica_by_author(keys.replica_id(), &docs_author, &docs_key, policy)
                .await?
            && let Some(item) = open(&record)
        {
            return Ok(Some(item));
        }
        Ok(source
            .query_replica_exact_bounded(
                keys.replica_id(),
                &docs_key,
                ACCOUNT_SYNC_RECORDS_PER_KEY,
                policy,
            )
            .await?
            .iter()
            .filter_map(open)
            .max_by(|a, b| (a.updated_at, &a.op_id).cmp(&(b.updated_at, &b.op_id))))
    }

    /// 本人の別の端末の item を、`(updated_at, op_id)` の勝者の規則で採用して反映する。採った（行を置き換えた・鍵の
    /// 行を足した）ら true。同じ操作の再受信・古い版（restore した古い端末の版を含む）は false。
    pub(crate) async fn merge_account_sync_item(&self, item: AccountSyncItem) -> Result<bool> {
        match &item.key {
            AccountSyncItemKey::Profile => {
                let envelope: KukuriEnvelope = serde_json::from_value(
                    item.value
                        .clone()
                        .context("a profile item must carry its envelope")?,
                )?;
                envelope.verify()?;
                anyhow::ensure!(
                    envelope.pubkey.as_str() == self.current_author_pubkey()
                        && AccountSyncItem::profile(&envelope)? == item
                        && parse_profile(&envelope)?.is_some(),
                    "the profile item does not match its envelope"
                );
                // この版より前の profile（行の無いもの）は、手元の profile の時刻と比べる。
                let row = self
                    .services
                    .projection_store
                    .get_account_sync_row(&item.key.docs_key())
                    .await?;
                if row.is_none() && self.get_my_profile().await?.updated_at > item.updated_at {
                    return Ok(false);
                }
                if !self
                    .services
                    .projection_store
                    .adopt_account_sync_row(&row_of(&item)?)
                    .await?
                {
                    return Ok(false);
                }
                self.commit_my_profile(envelope).await?;
                Ok(true)
            }
            AccountSyncItemKey::TrustAlwaysVisible { .. } => {
                anyhow::ensure!(
                    matches!(item.value, None | Some(serde_json::Value::Bool(true))),
                    "an always-visible item must be true or a tombstone"
                );
                self.services
                    .projection_store
                    .adopt_account_sync_row(&row_of(&item)?)
                    .await
            }
            // private channel の参加・鍵・担当（ADR 0061 §9）。
            _ => self.merge_channel_item(&item).await,
        }
    }

    /// 著者を常に表示する指定を設定・解除する（本人の端末で共有する。ADR 0061 §2）。
    pub async fn set_trust_always_visible(&self, author: &str, visible: bool) -> Result<()> {
        self.publish_account_sync_item(trust_always_visible_item(
            author,
            Utc::now().timestamp_millis(),
            visible,
        )?)
        .await
    }

    /// 旧版の端末内の設定を取り込む。編集した時刻は分からないので、どの編集よりも古い 0 にする。
    /// この端末で採用し、replica へは送り直しの規則（同じ key の版が違えば書く。ADR 0061 §10）で書く。
    pub async fn import_trust_always_visible(&self, author: &str) -> Result<()> {
        let item = trust_always_visible_item(author, 0, true)?;
        self.services
            .projection_store
            .adopt_account_sync_row(&row_of(&item)?)
            .await?;
        Ok(())
    }

    /// `authors` のうち、常に表示する著者（著者ごとに 1 行を読む）。
    pub async fn trust_always_visible(&self, authors: &[String]) -> Result<BTreeSet<String>> {
        let mut visible = BTreeSet::new();
        for author in authors {
            if self
                .services
                .projection_store
                .get_account_sync_row(&format!("{TRUST_ALWAYS_VISIBLE_PREFIX}{author}"))
                .await?
                .is_some_and(|row| row.value.is_some())
            {
                visible.insert(author.clone());
            }
        }
        Ok(visible)
    }

    /// 常に表示する著者の一覧（管理導線用）。
    pub async fn list_trust_always_visible(&self) -> Result<Vec<String>> {
        Ok(self
            .services
            .projection_store
            .list_account_sync_keys(TRUST_ALWAYS_VISIBLE_PREFIX)
            .await?
            .into_iter()
            .filter_map(|key| {
                key.strip_prefix(TRUST_ALWAYS_VISIBLE_PREFIX)
                    .map(str::to_string)
            })
            .collect())
    }
}
