//! object を 1 件、key 指定で projection へ反映する(#1239、ADR 0052 §2)。replica は走査しない。
//!
//! docs の event・hint の個別反映、利用者の操作の対象の反映、ページの範囲の照合が、この経路を通る。

use super::hydration_limits::{
    MissingBodyLedger, fetch_local_projection_blob_text, fetch_projection_blob_text_bounded,
};
use super::hydration_support::scrub_withdrawn_header;
use super::*;

/// object を 1 件、key 指定で反映した結果(#1239)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectHydration {
    /// projection へ反映した。
    Hydrated,
    /// `objects/<object id>/state` が手元に無い(entry が無い、または本体が未着)。後の反映で拾える。
    Missing,
    /// state が投稿の header として読めない。反映し直しても変わらないので、投稿として扱わない。
    Invalid,
}

/// 本文が blob の投稿を反映するときの、本文の取り方。docs の読み出しの policy とは別に決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyFetch {
    /// 手元にある本文だけを読む。1 回に多数の object を反映する経路(ページの範囲の照合)で使う。
    /// 欠けた本文は、表示のときに `recover_missing_bodies` が背景で取りに行く。
    LocalOnly,
    /// 手元に無ければ、台帳(`MissingBodyLedger`)の間隔と回数の内でだけ remote を試す。
    /// object を 1 件ずつ反映する経路(docs の event・hint、利用者の操作の対象)で使う。
    Bounded,
}

/// `objects/<object id>/state` の record を 1 件、projection へ反映する。
///
/// 投稿の header として読めない record は `Invalid` を返し、エラーにしない。public topic の replica は誰でも
/// 書けるので、読めない record を 1 件置くだけで、ページの取得や topic 全体の操作を止められないようにする
/// (ADR 0052 §2)。projection の読み書きの失敗はエラーとして返す。
pub(crate) async fn hydrate_object_projection_from_record(
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    missing_bodies: &MissingBodyLedger,
    replica: &ReplicaId,
    record: DocRecord,
    body_fetch: BodyFetch,
) -> Result<ObjectHydration> {
    let mut header: CanonicalPostHeader = match serde_json::from_slice(&record.value) {
        Ok(header) => header,
        Err(error) => {
            warn!(
                replica = %replica.as_str(),
                key = %record.key,
                error = %error,
                "ignored an object state record that is not a post header"
            );
            return Ok(ObjectHydration::Invalid);
        }
    };
    if projection_store
        .get_post_withdrawal(&header.object_id)
        .await?
        .is_some()
    {
        header = scrub_withdrawn_header(header);
        projection_store
            .put_object_projection(projection_row_from_header(
                &header,
                Some(String::new()),
                replica,
            ))
            .await?;
        return Ok(ObjectHydration::Hydrated);
    }
    let content = match &header.payload_ref {
        PayloadRef::InlineText { text } => Some(text.clone()),
        PayloadRef::BlobText { hash, .. } => {
            let payload = match body_fetch {
                BodyFetch::LocalOnly => fetch_local_projection_blob_text(blob_service, hash).await,
                BodyFetch::Bounded => {
                    fetch_projection_blob_text_bounded(blob_service, missing_bodies, hash).await
                }
            };
            projection_store
                .mark_blob_status(
                    hash,
                    match payload {
                        Some(_) => BlobCacheStatus::Available,
                        None => BlobCacheStatus::Missing,
                    },
                )
                .await?;
            payload
        }
    };
    for attachment in &header.attachments {
        let status = best_effort_blob_cache_status(blob_service, &attachment.hash).await;
        projection_store
            .mark_blob_status(&attachment.hash, status)
            .await?;
    }
    projection_store
        .put_object_projection(projection_row_from_header(&header, content, replica))
        .await?;
    Ok(ObjectHydration::Hydrated)
}

/// object id を 1 つ指定して、その投稿を projection へ反映する(#1239)。replica は走査しない。
/// 戻り値は反映できたか。本文は、手元に無ければ台帳の間隔の内でだけ remote を試す(`BodyFetch::Bounded`)。
///
/// `policy` は docs の key の読み出しに使う。利用者の操作は `LocalOnly`(操作を docs の remote 取得で
/// 待たせない)、event・hint・repost 元の解決は `LocalThenRemote`。
pub(crate) async fn hydrate_object_by_id(
    services: &ServiceHandles,
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    policy: DocFetchPolicy,
) -> Result<bool> {
    Ok(
        hydrate_object_by_id_with(services, replica, object_id, policy, BodyFetch::Bounded).await?
            == ObjectHydration::Hydrated,
    )
}

/// `hydrate_object_by_id` の本体。反映の結果の内訳と、本文の取り方を指定できる。
///
/// 取り下げを先に反映する。投稿の反映は projection の取り下げ表を見て本文と添付を伏せるため、
/// この順にすると、取り下げより後に反映した投稿でも本文が残らない。取り下げの event が対象の
/// envelope より先に届いて反映できなかった場合も、投稿を反映するときにここで取り直す。
pub(crate) async fn hydrate_object_by_id_with(
    services: &ServiceHandles,
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    policy: DocFetchPolicy,
    body_fetch: BodyFetch,
) -> Result<ObjectHydration> {
    let docs_sync = services.docs_sync.as_ref();
    let projection_store = services.projection_store.as_ref();
    let withdrawal_key = stable_key("withdrawals", &format!("{}/state", object_id.as_str()));
    if let Some(record) =
        query_replica_with_fetch_policy(docs_sync, replica, DocQuery::Exact(withdrawal_key), policy)
            .await?
            .into_iter()
            .next()
    {
        // 反映できなかった取り下げ(検証できない、対象が未着)は、投稿の反映を止めない。
        hydrate_post_withdrawal_from_record(docs_sync, projection_store, replica, record, policy)
            .await?;
    }
    let state_key = stable_key("objects", &format!("{}/state", object_id.as_str()));
    let Some(record) =
        query_replica_with_fetch_policy(docs_sync, replica, DocQuery::Exact(state_key), policy)
            .await?
            .into_iter()
            .next()
    else {
        return Ok(ObjectHydration::Missing);
    };
    hydrate_object_projection_from_record(
        services.blob_service.as_ref(),
        projection_store,
        services.missing_body_ledger.as_ref(),
        replica,
        record,
        body_fetch,
    )
    .await
}
