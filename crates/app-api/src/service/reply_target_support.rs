//! 返信先の preview の反映(#1239 AC-6、#1277)。view の生成は projection だけを読み、projection に無い返信先は
//! 取得側(タイムラインと thread の取得が view の生成の前に)と背景で、返信と同じ replica から key 指定で反映する。

use super::*;

impl AppService {
    /// 取得側(#1239 AC-6、#1277): ページの行の返信先が projection に無ければ、view の生成の前に反映する。
    ///
    /// 返信と同じ replica から key 指定で読む(`LocalOnly`)。読むのはページの行の数(`limit`)までで、確認先ごとに
    /// 背景の反映と同じ台帳で間隔を空ける(反映できない返信先を、取得のたびに読み直さない)。遡ったページの行も、
    /// その取得で preview が出る。view の生成は docs を読まない。
    ///
    /// 本文は手元の blob だけを読む(取得の経路で remote を待たない)。本文が手元に無い返信先は、ここでは反映せず、
    /// remote から本文を取る背景の反映へ出す。
    pub(crate) async fn reflect_reply_targets_for_rows(&self, rows: &[ObjectProjectionRow]) {
        for row in rows {
            let Some(target) = row.reply_to_object_id.as_ref() else {
                continue;
            };
            let ledger_key = format!("{}\n{}", row.source_replica_id.as_str(), target.as_str());
            let exists = self
                .services
                .projection_store
                .get_object_projection(target)
                .await
                .map_or(true, |row| row.is_some());
            if exists
                || !self
                    .services
                    .reply_target_checks
                    .try_begin(ledger_key.as_str(), Utc::now().timestamp_millis())
            {
                continue;
            }
            match reflect_reply_target_with(
                &self.services,
                target,
                &row.source_replica_id,
                row.topic_id.as_str(),
                ReplyTargetBody::LocalOnly,
            )
            .await
            {
                Ok(ReplyTargetReflection::BodyNotLocal) => spawn_reply_target_reflection(
                    &self.services,
                    &row.source_replica_id,
                    row.topic_id.as_str(),
                    target,
                ),
                Ok(_) => {}
                Err(error) => warn!(
                    object_id = %target.as_str(),
                    error = %error,
                    "failed to reflect a reply target of a listed row"
                ),
            }
        }
    }

    /// 返信先の行を projection から読む(#1239 AC-6)。view の生成中は docs を読まない。
    ///
    /// 返信先が projection に無ければ、返信と同じ replica からの反映を背景へ出し、この回の preview は出さない
    /// (反映できれば、次の取得で出る)。`source` は返信の行の replica と topic。
    pub(crate) async fn reply_target_row(
        &self,
        object_id: &EnvelopeId,
        source: Option<(&ReplicaId, &str)>,
    ) -> Result<Option<ObjectProjectionRow>> {
        if let Some(row) = self
            .services
            .projection_store
            .get_object_projection(object_id)
            .await?
        {
            return Ok(Some(row));
        }
        if let Some((replica_id, topic_id)) = source {
            self.schedule_reply_target_reflection(replica_id, topic_id, object_id);
        }
        Ok(None)
    }

    /// projection に無い返信先の反映を、背景で行う(#1239 AC-6)。呼び出し側は待たない。
    ///
    /// 確認先(replica と object id の組)ごとに間隔を空け、台帳の件数と同時実行の数に上限を置く。
    /// 読むのは返信先の key だけで、replica は走査しない。
    fn schedule_reply_target_reflection(
        &self,
        replica_id: &ReplicaId,
        topic_id: &str,
        object_id: &EnvelopeId,
    ) {
        let ledger_key = format!("{}\n{}", replica_id.as_str(), object_id.as_str());
        if !self
            .services
            .reply_target_checks
            .try_begin(ledger_key.as_str(), Utc::now().timestamp_millis())
        {
            return;
        }
        spawn_reply_target_reflection(&self.services, replica_id, topic_id, object_id);
    }
}

/// 返信先の反映を背景で行う task を起こす。台帳の確認は呼び出し側が済ませる。同時実行は台帳の permit で抑える。
fn spawn_reply_target_reflection(
    services: &ServiceHandles,
    replica_id: &ReplicaId,
    topic_id: &str,
    object_id: &EnvelopeId,
) {
    let services = services.clone();
    let replica_id = replica_id.clone();
    let topic_id = topic_id.to_string();
    let object_id = object_id.clone();
    tokio::spawn(async move {
        let permits = services.reply_target_checks.permits();
        let Ok(_permit) = permits.acquire().await else {
            return;
        };
        if let Err(error) =
            reflect_reply_target(&services, &object_id, &replica_id, topic_id.as_str()).await
        {
            warn!(
                object_id = %object_id.as_str(),
                error = %error,
                "failed to reflect a reply target in the background"
            );
        }
    });
}

/// 返信先の本文(blob)の取り方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplyTargetBody {
    /// 手元にある本文だけを読む(取得の経路)。
    LocalOnly,
    /// 手元に無ければ remote から取る(背景の反映)。
    Remote,
}

/// 返信先の反映の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
enum ReplyTargetReflection {
    Reflected(Box<ObjectProjectionRow>),
    /// 投稿は検証に通ったが、本文の blob が手元に無い(`ReplyTargetBody::LocalOnly` のときだけ)。
    BodyNotLocal,
    /// 手元の docs に、検証に通る投稿が無い。
    Unavailable,
}

/// 返信先を、返信と同じ replica から key 指定で反映する(`LocalOnly`。#1239 AC-6 の背景の反映)。本文は remote からも取る。
///
/// 返信先も同じ replica にある投稿として、署名つき envelope と replica の scope を確かめてから反映する(#1248)。
/// 既に projection にあれば、その行を返す。
pub(crate) async fn reflect_reply_target(
    services: &ServiceHandles,
    object_id: &EnvelopeId,
    replica_id: &ReplicaId,
    topic_id: &str,
) -> Result<Option<ObjectProjectionRow>> {
    Ok(
        match reflect_reply_target_with(
            services,
            object_id,
            replica_id,
            topic_id,
            ReplyTargetBody::Remote,
        )
        .await?
        {
            ReplyTargetReflection::Reflected(row) => Some(*row),
            ReplyTargetReflection::BodyNotLocal | ReplyTargetReflection::Unavailable => None,
        },
    )
}

async fn reflect_reply_target_with(
    services: &ServiceHandles,
    object_id: &EnvelopeId,
    replica_id: &ReplicaId,
    topic_id: &str,
    body: ReplyTargetBody,
) -> Result<ReplyTargetReflection> {
    if let Some(row) = services
        .projection_store
        .get_object_projection(object_id)
        .await?
    {
        return Ok(ReplyTargetReflection::Reflected(Box::new(row)));
    }
    // 手元の docs が読めないとき(権限を失った private replica など)は、反映せずに終える。
    let Ok(Some(post)) = load_verified_post(
        services.docs_sync.as_ref(),
        replica_id,
        topic_id,
        object_id,
        DocFetchPolicy::LocalOnly,
    )
    .await
    else {
        return Ok(ReplyTargetReflection::Unavailable);
    };
    let is_withdrawn = services
        .projection_store
        .get_post_withdrawal(object_id)
        .await?
        .is_some();
    let row = if is_withdrawn {
        projection_row_from_post(&post.withdrawn(), Some(String::new()))
    } else {
        let content = match (&post.header().payload_ref, body) {
            (PayloadRef::InlineText { text }, _) => Some(text.clone()),
            (PayloadRef::BlobText { hash, .. }, ReplyTargetBody::Remote) => {
                fetch_projection_blob_text(services.blob_service.as_ref(), hash).await
            }
            (PayloadRef::BlobText { hash, .. }, ReplyTargetBody::LocalOnly) => {
                match hydration_limits::fetch_local_projection_blob_text(
                    services.blob_service.as_ref(),
                    hash,
                )
                .await
                {
                    Some(text) => Some(text),
                    None => return Ok(ReplyTargetReflection::BodyNotLocal),
                }
            }
        };
        projection_row_from_post(&post, content)
    };
    services
        .projection_store
        .put_object_projection(row.clone())
        .await?;
    Ok(ReplyTargetReflection::Reflected(Box::new(row)))
}
