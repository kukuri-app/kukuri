//! 公開参照の索引（#1632 AC-6、ADR 0063 §8）。native の `public_blob_refs` と同じ意味で、検証済みの公開記録
//! （公開 topic の投稿の本文・添付・repost の添付・リンクプレビューの画像、profile の画像、公開 topic の custom reaction
//! の asset）が参照する blob を、記録ごとに書込みと同じ transaction で置き換える。Community Node へ保持端末の検索を
//! 頼むのは、ここに参照のある hash だけ。導入前の行は、取込みの位置（`meta`）から小分けに取り込む。

use std::collections::BTreeSet;

use anyhow::Result;
use kukuri_core::Profile;
use kukuri_store::{
    ObjectProjectionRow, ReactionProjectionRow, public_blob_hashes_for_reaction,
    public_blob_hashes_for_row,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use web_sys::{IdbKeyRange, IdbTransaction};

use crate::IndexedDbCache;
use crate::content_cache::{META, OBJECTS, PROFILES, PUBLIC_REFS, REACTIONS};
use crate::idb::{self, Mode, js_error};
use crate::rows::{self, Txn, key, only, prefix, text};

/// 取込みの位置の `meta` の key。値が無ければ最初から取り込む（導入前の database）。
const BACKFILL: &str = "public_refs_backfill";
/// 取り込む記録の種類の順。
const KINDS: [&str; 3] = ["post", "profile", "reaction"];

#[derive(Serialize, Deserialize)]
struct PublicRef {
    kind: String,
    source_id: String,
    blob_hash: String,
}

/// 取込みの位置。`kind` が `KINDS` の数なら取り込み終えた。`after` は最後に読んだ行の主 key。
#[derive(Default, Serialize, Deserialize)]
struct Backfill {
    kind: usize,
    after: Option<Vec<String>>,
}

/// 記録 1 件（`kind`・`source_id`）が参照する blob を `hashes` に置き換える。
pub(crate) fn replace(
    tx: &IdbTransaction,
    kind: &str,
    source_id: &str,
    hashes: Vec<String>,
) -> Result<()> {
    rows::delete(
        tx,
        PUBLIC_REFS,
        &prefix(&[text(kind), text(source_id)])?.into(),
    )?;
    for blob_hash in hashes.into_iter().collect::<BTreeSet<_>>() {
        let entry = PublicRef {
            kind: kind.to_owned(),
            source_id: source_id.to_owned(),
            blob_hash,
        };
        rows::put(tx, PUBLIC_REFS, &entry, &[])?;
    }
    Ok(())
}

/// 投稿の参照（本文・添付と、リンクプレビューの画像）を外す（取り下げ、行の回収）。
pub(crate) fn forget_post(tx: &IdbTransaction, object_id: &str) -> Result<()> {
    replace(tx, "post", object_id, Vec::new())?;
    replace(tx, "link_preview", object_id, Vec::new())
}

pub(crate) fn replace_profile(tx: &IdbTransaction, profile: &Profile) -> Result<()> {
    let hashes = profile
        .picture_asset
        .iter()
        .map(|asset| asset.hash.as_str().to_string())
        .collect();
    replace(tx, "profile", profile.pubkey.as_str(), hashes)
}

/// reaction の参照を、手元の対象の投稿の行（公開 topic か）で決める。
pub(crate) async fn replace_reaction(
    tx: &IdbTransaction,
    row: &ReactionProjectionRow,
) -> Result<()> {
    let target =
        rows::get::<ObjectProjectionRow>(tx, OBJECTS, &text(row.target_object_id.as_str())).await?;
    let hashes =
        public_blob_hashes_for_reaction(row, target.as_ref().map(|row| row.channel_id.as_str()));
    replace(tx, "reaction", row.reaction_id.as_str(), hashes)
}

impl IndexedDbCache {
    /// `hash` を参照する公開記録があるか（最初の 1 行だけを読む）。
    pub(crate) async fn has_public_ref(&self, hash: &str) -> Result<bool> {
        let hash = hash.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PUBLIC_REFS], Mode::Read)?;
            let mut found = false;
            rows::walk(
                &tx,
                PUBLIC_REFS,
                Some("hash"),
                &only(&text(&hash))?,
                false,
                |_| {
                    found = true;
                    Ok(false)
                },
            )
            .await?;
            Ok(found)
        })
        .await
    }

    /// 導入前の記録を、今の種類の `limit` 行まで取り込む。すべて取り込み終えていれば `true`。
    pub(crate) async fn backfill_public_refs_step(&self, limit: usize) -> Result<bool> {
        self.run(move |db| async move {
            let tx = Txn::begin(
                &db.idb,
                &[META, OBJECTS, PROFILES, REACTIONS, PUBLIC_REFS],
                Mode::Write,
            )?;
            let meta = rows::store(&tx, META)?;
            let saved = idb::done(&meta.get(&BACKFILL.into()).map_err(js_error)?).await?;
            let mut state = match saved.as_string() {
                Some(saved) => serde_json::from_str::<Backfill>(&saved)?,
                None => Backfill::default(),
            };
            let Some(kind) = KINDS.get(state.kind).copied() else {
                return Ok(true);
            };
            let range = match &state.after {
                Some(after) => after_range(after)?,
                // 数（-∞）はどの型の key よりも小さい。
                None => IdbKeyRange::lower_bound(&JsValue::from_f64(f64::NEG_INFINITY))
                    .map_err(js_error)?,
            };
            let read = match kind {
                "post" => {
                    let rows =
                        rows::scan::<ObjectProjectionRow>(&tx, OBJECTS, None, &range, false, limit)
                            .await?;
                    for row in &rows {
                        replace(
                            &tx,
                            kind,
                            row.object_id.as_str(),
                            public_blob_hashes_for_row(row),
                        )?;
                    }
                    state.after = rows
                        .last()
                        .map(|row| vec![row.object_id.as_str().to_owned()]);
                    rows.len()
                }
                "profile" => {
                    let rows =
                        rows::scan::<Profile>(&tx, PROFILES, None, &range, false, limit).await?;
                    for row in &rows {
                        replace_profile(&tx, row)?;
                    }
                    state.after = rows.last().map(|row| vec![row.pubkey.as_str().to_owned()]);
                    rows.len()
                }
                _ => {
                    let rows = rows::scan::<ReactionProjectionRow>(
                        &tx, REACTIONS, None, &range, false, limit,
                    )
                    .await?;
                    for row in &rows {
                        replace_reaction(&tx, row).await?;
                    }
                    state.after = rows.last().map(|row| {
                        vec![
                            row.source_replica_id.as_str().to_owned(),
                            row.target_object_id.as_str().to_owned(),
                            row.reaction_id.as_str().to_owned(),
                        ]
                    });
                    rows.len()
                }
            };
            if read < limit {
                state = Backfill {
                    kind: state.kind + 1,
                    after: None,
                };
            }
            meta.put_with_key(&serde_json::to_string(&state)?.into(), &BACKFILL.into())
                .map_err(js_error)?;
            tx.commit().await?;
            Ok(state.kind >= KINDS.len())
        })
        .await
    }
}

/// 最後に読んだ主 key（1 欄の store は文字列、複数欄の store は配列）より後の範囲。
fn after_range(after: &[String]) -> Result<IdbKeyRange> {
    let last: JsValue = match after {
        [single] => text(single),
        parts => key(&parts.iter().map(|part| text(part)).collect::<Vec<_>>()),
    };
    IdbKeyRange::lower_bound_with_open(&last, true).map_err(js_error)
}
