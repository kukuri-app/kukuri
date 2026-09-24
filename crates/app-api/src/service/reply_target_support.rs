//! 返信先の preview の反映(#1239 AC-6、#1277)。view の生成は projection だけを読み、projection に無い返信先は
//! 取得側(タイムラインと thread の取得が view の生成の前に)と背景で、返信と同じ replica から key 指定で反映する。

use super::*;

#[derive(Clone, Debug)]
struct ReplyTargetSource {
    target: EnvelopeId,
    source_replica_id: ReplicaId,
    topic_id: String,
    channel_id: String,
}

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
        let sources = rows
            .iter()
            .filter_map(|row| {
                row.reply_to_object_id
                    .as_ref()
                    .map(|target| ReplyTargetSource {
                        target: target.clone(),
                        source_replica_id: row.source_replica_id.clone(),
                        topic_id: row.topic_id.clone(),
                        channel_id: row.channel_id.clone(),
                    })
            })
            .collect::<Vec<_>>();
        self.reflect_reply_targets_for_sources(&sources).await;
    }

    pub(crate) async fn reflect_reply_targets_for_profile_items(
        &self,
        items: &[ProfileTimelineItem],
    ) {
        let sources = items
            .iter()
            .filter_map(|item| match item {
                ProfileTimelineItem::Post(post) => {
                    post.reply_to_object_id
                        .as_ref()
                        .map(|target| ReplyTargetSource {
                            target: target.clone(),
                            source_replica_id: topic_replica_id(post.published_topic_id.as_str()),
                            topic_id: post.published_topic_id.as_str().to_string(),
                            channel_id: PUBLIC_CHANNEL_ID.to_string(),
                        })
                }
                ProfileTimelineItem::Repost(_) => None,
            })
            .collect::<Vec<_>>();
        self.reflect_reply_targets_for_sources(&sources).await;
    }

    /// Timeline以外のbounded view（Profile）も、表示する行から作った参照だけを
    /// 同じ有限retryへ渡す。private channelは現在の参加状態をI/O前に確認する。
    async fn reflect_reply_targets_for_sources(&self, sources: &[ReplyTargetSource]) {
        let mut missing_body_targets = Vec::new();
        let mut seen_targets = HashSet::new();
        for source in sources {
            if source.channel_id != PUBLIC_CHANNEL_ID
                && self
                    .ensure_private_channel_access(
                        source.topic_id.as_str(),
                        &ChannelId::new(source.channel_id.clone()),
                    )
                    .await
                    .is_err()
            {
                continue;
            }
            let target = &source.target;
            match self
                .services
                .projection_store
                .get_post_withdrawal(target)
                .await
            {
                Ok(None) => {}
                Ok(Some(_)) | Err(_) => continue,
            }
            let ledger_key = format!("{}\n{}", source.source_replica_id.as_str(), target.as_str());
            let Ok(existing) = self
                .services
                .projection_store
                .get_object_projection(target)
                .await
            else {
                continue;
            };
            if let Some(target_row) = existing {
                if target_row.content.is_some()
                    || target_row.topic_id != source.topic_id
                    || target_row.channel_id != source.channel_id
                    || (kukuri_core::has_adult_content_label(&target_row.content_labels)
                        && !self.adult_content_display_enabled())
                    || !seen_targets.insert(target_row.object_id.clone())
                {
                    continue;
                }
                missing_body_targets.push(target_row);
                continue;
            }
            let Ok(permit) = self
                .services
                .reply_target_checks
                .permits()
                .try_acquire_owned()
            else {
                continue;
            };
            if !self
                .services
                .reply_target_checks
                .try_begin(ledger_key.as_str(), Utc::now().timestamp_millis())
            {
                continue;
            }
            match reflect_reply_target_with(
                &self.services,
                target,
                &source.source_replica_id,
                source.topic_id.as_str(),
                ReplyTargetBody::LocalOnly,
            )
            .await
            {
                Ok(ReplyTargetReflection::BodyNotLocal) => spawn_reply_target_reflection(
                    &self.services,
                    &source.source_replica_id,
                    source.topic_id.as_str(),
                    target,
                    permit,
                ),
                Ok(_) => {}
                Err(error) => warn!(
                    object_id = %target.as_str(),
                    error = %error,
                    "failed to reflect a reply target of a listed row"
                ),
            }
        }
        if !missing_body_targets.is_empty() {
            self.recover_missing_bodies(&mut missing_body_targets).await;
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
        let Ok(permit) = self
            .services
            .reply_target_checks
            .permits()
            .try_acquire_owned()
        else {
            return;
        };
        let ledger_key = format!("{}\n{}", replica_id.as_str(), object_id.as_str());
        if !self
            .services
            .reply_target_checks
            .try_begin(ledger_key.as_str(), Utc::now().timestamp_millis())
        {
            return;
        }
        spawn_reply_target_reflection(&self.services, replica_id, topic_id, object_id, permit);
    }

    /// #1284: 投稿カードの明示再読み込み。対象投稿自身か直前の返信先にある欠損本文だけを
    /// 1 回再試行し、更新後のカード view を返す。projection / replica の走査は行わない。
    pub async fn retry_post_elements(
        &self,
        object_id: &str,
        body_object_id: Option<&str>,
        manual: bool,
    ) -> Result<Option<PostView>> {
        let object_id = EnvelopeId::from(object_id);
        let Some(container) = self
            .services
            .projection_store
            .get_object_projection(&object_id)
            .await?
        else {
            return Ok(None);
        };
        if container.channel_id != PUBLIC_CHANNEL_ID {
            self.ensure_private_channel_access(
                container.topic_id.as_str(),
                &ChannelId::new(container.channel_id.clone()),
            )
            .await?;
        }
        let allowed_body_ids = match body_object_id {
            Some(requested) => {
                let requested = EnvelopeId::from(requested);
                if requested != container.object_id
                    && container.reply_to_object_id.as_ref() != Some(&requested)
                {
                    return Ok(None);
                }
                vec![requested]
            }
            None => {
                let mut ids = vec![container.object_id.clone()];
                if let Some(reply_to) = container.reply_to_object_id.clone() {
                    ids.push(reply_to);
                }
                ids
            }
        };
        let mut rows = Vec::with_capacity(allowed_body_ids.len());
        for body_id in allowed_body_ids {
            if self
                .services
                .projection_store
                .get_post_withdrawal(&body_id)
                .await?
                .is_some()
            {
                continue;
            }
            let Some(row) = self
                .services
                .projection_store
                .get_object_projection(&body_id)
                .await?
            else {
                continue;
            };
            if row.topic_id != container.topic_id || row.channel_id != container.channel_id {
                continue;
            }
            let PayloadRef::BlobText { hash, .. } = &row.payload_ref else {
                continue;
            };
            if kukuri_core::has_adult_content_label(&row.content_labels)
                && !self.adult_content_display_enabled()
            {
                continue;
            }
            if row.content.is_none()
                && (!manual || self.services.missing_body_ledger.request_manual_retry(hash))
            {
                rows.push(row);
            }
        }
        self.recover_missing_bodies(&mut rows).await;

        let Some(container) = self
            .services
            .projection_store
            .get_object_projection(&object_id)
            .await?
        else {
            return Ok(None);
        };
        let mut view = self
            .page_to_view(Page {
                items: vec![container],
                next_cursor: None,
            })
            .await?;
        Ok(view.items.pop())
    }
}

/// 返信先の反映を背景で行う task を起こす。台帳の確認は呼び出し側が済ませる。同時実行は台帳の permit で抑える。
fn spawn_reply_target_reflection(
    services: &ServiceHandles,
    replica_id: &ReplicaId,
    topic_id: &str,
    object_id: &EnvelopeId,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    let services = services.clone();
    let replica_id = replica_id.clone();
    let topic_id = topic_id.to_string();
    let object_id = object_id.clone();
    tokio::spawn(async move {
        let _permit = permit;
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

/// 返信先を、返信と同じ replica から key 指定で反映する(#1239 AC-6 の背景の反映)。docs は手元だけを読み
/// (`DocFetchPolicy::LocalOnly`)、本文の blob は手元に無ければ remote から取る(`ReplyTargetBody::Remote`)。
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
