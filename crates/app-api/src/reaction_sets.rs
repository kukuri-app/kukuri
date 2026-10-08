//! カスタムリアクションのセット（#1232 D3、ADR 0017 §2.6）。セットは「画像の hash＋検索名（＋形式・大きさ・最初に
//! 作った人）」の並びの署名のない公開 blob で、その hash で共有する。取り込みはセットの blob を手元・既知の相手・DHT
//! （Web は Community Node の検索）から取り、各リアクションを画像＋検索名の ID のまま保存済みへ置く。

use crate::service::*;
use kukuri_core::{
    BlobHash, CUSTOM_REACTION_SET_MAX_BYTES, CUSTOM_REACTION_SET_MIME, CustomReactionSetItemV1,
    CustomReactionSetV1,
};
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};
use serde::Deserialize;

/// 作ったカスタムリアクションのセット（#1232 D3）。`set_hash` はセットの公開 blob の hash で、共有の文字列は
/// `kukuri:reaction-set:<set_hash>`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CustomReactionSetView {
    pub set_hash: String,
    pub name: String,
    pub item_count: u32,
    pub created_at: i64,
}

/// セットの取り込みの結果。`saved` は保存済みへ加えた（置き直した）リアクション、`skipped_own` は自分が作ったので
/// 保存しなかった件数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportedCustomReactionSetView {
    pub set_hash: String,
    pub name: String,
    pub saved: Vec<CustomReactionAssetView>,
    pub skipped_own: u32,
}

/// 作ったセットの一覧に示す件数（新しい順。#1232 D3）。
const OWN_REACTION_SETS: usize = 100;

/// 作ったセットの一覧の record（自分の author replica の `reactions/sets/<作成時刻>-<セットの hash>`）。
#[derive(Serialize, Deserialize)]
struct CustomReactionSetDocV1 {
    set_hash: String,
    name: String,
    item_count: u32,
    created_at: i64,
}

impl From<CustomReactionSetDocV1> for CustomReactionSetView {
    fn from(doc: CustomReactionSetDocV1) -> Self {
        Self {
            set_hash: doc.set_hash,
            name: doc.name,
            item_count: doc.item_count,
            created_at: doc.created_at,
        }
    }
}

impl AppService {
    /// 自作・保存済みから選んだリアクション（最大 100 件）と名前でセットを作り、公開 blob に置く。
    pub async fn create_custom_reaction_set(
        &self,
        name: &str,
        items: Vec<CustomReactionAssetView>,
    ) -> Result<CustomReactionSetView> {
        let set = CustomReactionSetV1 {
            name: name.trim().to_string(),
            items: items
                .into_iter()
                .map(|asset| CustomReactionSetItemV1 {
                    owner_pubkey: Pubkey::from(asset.owner_pubkey.as_str()),
                    blob_hash: BlobHash::new(asset.blob_hash),
                    search_key: asset.search_key.trim().to_string(),
                    mime: asset.mime,
                    bytes: asset.bytes,
                    width: asset.width,
                    height: asset.height,
                })
                .collect(),
        };
        let bytes = set.to_bytes()?;
        let stored = self
            .services
            .blob_service
            .put_blob(bytes, CUSTOM_REACTION_SET_MIME)
            .await?;
        let doc = CustomReactionSetDocV1 {
            set_hash: stored.hash.as_str().to_string(),
            name: set.name,
            item_count: set.items.len() as u32,
            created_at: Utc::now().timestamp_millis(),
        };
        let replica = author_replica_id(self.current_author_pubkey().as_str());
        self.services.docs_sync.open_replica(&replica).await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "reactions/sets",
                        &format!("{:020}-{}", doc.created_at, doc.set_hash),
                    ),
                    value: serde_json::to_value(&doc)?,
                },
            )
            .await?;
        Ok(doc.into())
    }

    /// 作ったセットの新しい順の `OWN_REACTION_SETS` 件。それより古いセットは示さない（共有の文字列はそのまま使える）。
    pub async fn list_my_custom_reaction_sets(&self) -> Result<Vec<CustomReactionSetView>> {
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        Ok(load_author_records(
            self.services.docs_sync.as_ref(),
            self.current_author_pubkey().as_str(),
            docs_author.as_deref(),
            DocKeyQuery {
                prefix: stable_key("reactions/sets", ""),
                order: DocKeyOrder::Descending,
                limit: OWN_REACTION_SETS,
            },
            |_| true,
            |value| serde_json::from_slice::<CustomReactionSetDocV1>(value).ok(),
        )
        .await?
        .into_iter()
        .map(CustomReactionSetView::from)
        .collect())
    }

    /// セットを取り、各リアクションを画像＋検索名の ID のまま保存済みへ置く（自分が作ったものは除く）。取れない・形式が
    /// 違うときは何も保存しない。読むのはセットの blob 1 件だけ。
    pub async fn import_custom_reaction_set(
        &self,
        set_hash: &str,
    ) -> Result<ImportedCustomReactionSetView> {
        let hash = BlobHash::new(set_hash.trim());
        let held = self.services.blob_service.local_blob_status(&hash).await?;
        let bytes = self
            .services
            .blob_service
            .fetch_blob_ephemeral_bounded(&hash, CUSTOM_REACTION_SET_MAX_BYTES)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the custom reaction set could not be fetched"))?;
        let set = CustomReactionSetV1::from_bytes(&bytes)?;
        // 取り込んだセットは cache に置き、他の人の取得に答える（公開投稿に貼られたものは告知もする）。
        if matches!(held, BlobStatus::Missing) {
            self.services
                .blob_service
                .put_remote_blob(bytes, CUSTOM_REACTION_SET_MIME)
                .await?;
        }
        let me = self.current_author_pubkey();
        let now = Utc::now().timestamp_millis();
        let mut saved = Vec::new();
        let mut skipped_own = 0;
        for (index, item) in set.items.into_iter().enumerate() {
            if item.owner_pubkey.as_str() == me {
                skipped_own += 1;
                continue;
            }
            let asset_id =
                kukuri_core::custom_reaction_id(item.blob_hash.as_str(), &item.search_key);
            // 新しい順の一覧でセットの並びになるよう、先頭ほど新しい時刻で置く。
            let bookmarked_at = now - index as i64;
            saved.push(
                self.put_custom_reaction_bookmark(
                    CustomReactionAssetSnapshotV1 {
                        asset_id,
                        owner_pubkey: item.owner_pubkey,
                        blob_hash: item.blob_hash,
                        search_key: item.search_key,
                        mime: item.mime,
                        bytes: item.bytes,
                        width: item.width,
                        height: item.height,
                    },
                    bookmarked_at,
                )
                .await?,
            );
        }
        Ok(ImportedCustomReactionSetView {
            set_hash: hash.as_str().to_string(),
            name: set.name,
            saved,
            skipped_own,
        })
    }
}
