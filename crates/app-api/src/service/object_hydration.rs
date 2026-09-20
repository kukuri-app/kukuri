//! object を 1 件、key 指定で projection へ反映する(#1239、ADR 0052 §2)。replica は走査しない。
//!
//! docs の event・hint の個別反映、利用者の操作の対象の反映、ページの範囲の照合が、この経路を通る。
//! 行は署名つき envelope から作る(#1248)。

use super::hydration_limits::{
    MissingBodyLedger, fetch_local_projection_blob_text, fetch_projection_blob_text_bounded,
};
use super::*;

/// object を 1 件、key 指定で反映した結果(#1239)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectHydration {
    /// projection へ反映した。
    Hydrated,
    /// `objects/<object id>/envelope` が手元に無い(entry が無い、または本体が未着)。後の反映で拾える。
    Missing,
    /// envelope の record はあるが、検証に通るものが無い。反映し直しても変わらないので、投稿として扱わない。
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

/// 検証済みの投稿を 1 件、projection へ反映する。
pub(crate) async fn hydrate_object_projection_from_post(
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    missing_bodies: &MissingBodyLedger,
    post: VerifiedPost,
    body_fetch: BodyFetch,
) -> Result<()> {
    if projection_store
        .get_post_withdrawal(&post.header().object_id)
        .await?
        .is_some()
    {
        projection_store
            .put_object_projection(projection_row_from_post(
                &post.withdrawn(),
                Some(String::new()),
            ))
            .await?;
        return Ok(());
    }
    let header = post.header();
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
        .put_object_projection(projection_row_from_post(&post, content))
        .await?;
    Ok(())
}

/// object id を 1 つ指定して、その投稿を projection へ反映する(#1239)。replica は走査しない。
/// 戻り値は反映できたか。本文は、手元に無ければ台帳の間隔の内でだけ remote を試す(`BodyFetch::Bounded`)。
///
/// `policy` は docs の key の読み出しに使う。利用者の操作は `LocalOnly`(操作を docs の remote 取得で
/// 待たせない)、event・hint・repost 元の解決は `LocalThenRemote`。
pub(crate) async fn hydrate_object_in_topic(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    policy: DocFetchPolicy,
) -> Result<bool> {
    hydrate_object_in_topic_with_hint(services, topic_id, replica, object_id, None, policy).await
}

/// `hydrate_object_in_topic` に、その投稿の envelope を書いた docs author の手がかり(docs の event、hint の
/// docs author)を足したもの(ADR 0053 §3)。手がかりは読む record を選ぶことだけに使う。
pub(crate) async fn hydrate_object_in_topic_with_hint(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    docs_author_hint: Option<&str>,
    policy: DocFetchPolicy,
) -> Result<bool> {
    Ok(hydrate_object_in_topic_with(
        services,
        topic_id,
        replica,
        object_id,
        docs_author_hint,
        policy,
        BodyFetch::Bounded,
    )
    .await?
        == ObjectHydration::Hydrated)
}

/// `hydrate_object_in_topic` の本体。反映の結果の内訳と、本文の取り方を指定できる。
///
/// envelope を先に読んで検証し、著者が署名つきの tag で申告した docs author で取り下げを読む(ADR 0053 §3)。
/// `docs_author_hint` は envelope を書いた docs author の手がかり(docs の event、hint、索引の entry の docs author)で、
/// 読む record を選ぶことだけに使う。
///
/// 取り下げの反映は、投稿の行を書くより前に行う。投稿の反映は projection の取り下げ表を見て本文と添付を伏せるため、
/// この順にすると、取り下げより後に反映した投稿でも本文が残らない。取り下げの event が対象の envelope より先に届いて
/// 反映できなかった場合も、投稿を反映するときにここで取り直す。検証に通る envelope が無いときは取り下げを読まない
/// (対象が無い取り下げは検証できない。envelope が届いた event でもう一度ここを通る)。
///
/// 行は署名つき envelope から作る(#1248)。`topic_id` は、その replica を読む文脈の topic(private channel の
/// replica id は topic を含まない)。読む docs の record 数は定数で、key に積まれた record 数にも replica の大きさにも
/// 依存しない(#1248、#1250、#1258)。
pub(crate) async fn hydrate_object_in_topic_with(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    docs_author_hint: Option<&str>,
    policy: DocFetchPolicy,
    body_fetch: BodyFetch,
) -> Result<ObjectHydration> {
    let docs_sync = services.docs_sync.as_ref();
    let projection_store = services.projection_store.as_ref();
    let post = match load_post_with_hint(
        docs_sync,
        replica,
        topic_id,
        object_id,
        docs_author_hint,
        policy,
    )
    .await?
    {
        PostLoad::Verified(post) => *post,
        PostLoad::Missing => return Ok(ObjectHydration::Missing),
        PostLoad::Rejected => return Ok(ObjectHydration::Invalid),
    };
    // 反映できなかった取り下げ(検証できない)は、投稿の反映を止めない。読み出しの失敗はエラーとして返し、
    // 「取り下げなし」として本文つきの行を作らない。
    hydrate_post_withdrawal_for_object_with_hints(
        docs_sync,
        projection_store,
        replica,
        object_id,
        WithdrawalReadHints {
            target_docs_author: post.docs_author(),
            writer_docs_author: None,
        },
        policy,
    )
    .await?;
    hydrate_object_projection_from_post(
        services.blob_service.as_ref(),
        projection_store,
        services.missing_body_ledger.as_ref(),
        post,
        body_fetch,
    )
    .await?;
    Ok(ObjectHydration::Hydrated)
}
