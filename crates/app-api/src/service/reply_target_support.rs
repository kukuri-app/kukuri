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
            let ledger_key = hydration_limits::display_retry_key(
                "reply",
                source.source_replica_id.as_str(),
                target.as_str(),
            );
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
                .missing_body_ledger
                .fetch_permits()
                .try_acquire_owned()
            else {
                continue;
            };
            let Some(attempt) = self
                .services
                .missing_body_ledger
                .try_begin_key(ledger_key.as_str(), Utc::now().timestamp_millis())
            else {
                continue;
            };
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
                    attempt,
                ),
                Ok(ReplyTargetReflection::Reflected(_)) => attempt.succeed(),
                Ok(ReplyTargetReflection::Unavailable) => {
                    let epoch = if source.channel_id == PUBLIC_CHANNEL_ID {
                        None
                    } else {
                        self.private_epoch_for_source(
                            source.topic_id.as_str(),
                            source.channel_id.as_str(),
                            &source.source_replica_id,
                        )
                        .await
                        .ok()
                        .flatten()
                    };
                    let readers = self
                        .remote_post_readers(
                            source.topic_id.as_str(),
                            (source.channel_id != PUBLIC_CHANNEL_ID)
                                .then_some(source.channel_id.as_str()),
                            &source.source_replica_id,
                            epoch
                                .as_ref()
                                .map(|(id, secret)| (id.as_str(), secret.as_str())),
                        )
                        .await
                        .unwrap_or_default();
                    if readers.is_empty() {
                        attempt.fail();
                    } else {
                        spawn_remote_reply_target_reflection(
                            &self.services,
                            source,
                            target,
                            readers,
                            permit,
                            attempt,
                        );
                    }
                }
                Ok(ReplyTargetReflection::Deferred) => attempt.defer(),
                Err(error) => {
                    warn!(object_id = %target.as_str(), error = %error, "failed to reflect a reply target of a listed row");
                    attempt.fail();
                }
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
            .missing_body_ledger
            .fetch_permits()
            .try_acquire_owned()
        else {
            return;
        };
        let ledger_key =
            hydration_limits::display_retry_key("reply", replica_id.as_str(), object_id.as_str());
        let Some(attempt) = self
            .services
            .missing_body_ledger
            .try_begin_key(ledger_key.as_str(), Utc::now().timestamp_millis())
        else {
            return;
        };
        spawn_reply_target_reflection(
            &self.services,
            replica_id,
            topic_id,
            object_id,
            permit,
            attempt,
        );
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
                if container.reply_to_object_id.as_ref() == Some(&body_id) {
                    let key = hydration_limits::display_retry_key(
                        "reply",
                        container.source_replica_id.as_str(),
                        body_id.as_str(),
                    );
                    if let Ok(_permit) = self
                        .services
                        .missing_body_ledger
                        .fetch_permits()
                        .try_acquire_owned()
                        && (!manual
                            || self
                                .services
                                .missing_body_ledger
                                .request_manual_retry_key(&key))
                        && let Some(attempt) = self
                            .services
                            .missing_body_ledger
                            .try_begin_key(&key, Utc::now().timestamp_millis())
                    {
                        let _ = attempt_reply_target_reflection(
                            &self.services,
                            &container.source_replica_id,
                            container.topic_id.as_str(),
                            &body_id,
                            attempt,
                        )
                        .await;
                    }
                }
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

    /// 表示中の次回通知時刻。実取得の台帳を正本にし、IPC呼び出し自体は試行に数えない。
    pub async fn post_display_retry_at(&self, post: &PostView) -> Result<Option<i64>> {
        let Some(container) = self
            .services
            .projection_store
            .get_object_projection(&EnvelopeId::from(post.object_id.as_str()))
            .await?
        else {
            return Ok(None);
        };
        let now = Utc::now().timestamp_millis();
        let ledger = &self.services.missing_body_ledger;
        let mut next = None;
        if post.content_status == BlobViewStatus::Missing
            && let PayloadRef::BlobText { hash, .. } = &container.payload_ref
        {
            next = ledger.display_retry_at_key(hash.as_str(), now);
        }
        if let Some(reply_id) = &container.reply_to_object_id
            && post
                .reply_preview
                .as_ref()
                .is_none_or(|preview| preview.content_status == BlobViewStatus::Missing)
        {
            let reply = self
                .services
                .projection_store
                .get_object_projection(reply_id)
                .await?;
            let key = match reply.as_ref().map(|row| &row.payload_ref) {
                Some(PayloadRef::BlobText { hash, .. }) => hash.as_str().to_owned(),
                None => hydration_limits::display_retry_key(
                    "reply",
                    container.source_replica_id.as_str(),
                    reply_id.as_str(),
                ),
                _ => return Ok(next),
            };
            if let Some(due) = ledger.display_retry_at_key(&key, now) {
                next = Some(next.map_or(due, |prior: i64| prior.min(due)));
            }
        }
        Ok(next)
    }
}

fn spawn_remote_reply_target_reflection(
    services: &ServiceHandles,
    source: &ReplyTargetSource,
    target: &EnvelopeId,
    readers: Vec<Arc<dyn DocsSync>>,
    permit: tokio::sync::OwnedSemaphorePermit,
    attempt: hydration_limits::MissingBodyAttempt,
) {
    let services = services.clone();
    let source = source.clone();
    let target = target.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let Some(generation) = services
            .active_content_scope_generation(&source.topic_id, &source.channel_id)
            .await
        else {
            attempt.defer();
            return;
        };
        for reader in readers {
            let mut view = services.clone();
            view.docs_sync = reader;
            let read = services.until_content_invalid(
                &source.topic_id,
                &source.channel_id,
                generation,
                hydrate_object_in_topic_with(
                    &view,
                    &source.topic_id,
                    &source.source_replica_id,
                    &target,
                    None,
                    DocFetchPolicy::LocalThenRemote,
                    BodyFetch::LocalOnly,
                ),
            );
            let result = match tokio::time::timeout_at(deadline, read).await {
                Ok(result) => result,
                Err(_) => break,
            };
            match result {
                Some(Ok(ObjectHydration::Hydrated)) => {
                    attempt.succeed();
                    return;
                }
                Some(Ok(ObjectHydration::Missing | ObjectHydration::Invalid)) => {}
                Some(Err(error)) => warn!(%error, "remote reply target read failed"),
                None => {
                    attempt.defer();
                    return;
                }
            }
        }
        attempt.fail();
    });
}

async fn attempt_reply_target_reflection(
    services: &ServiceHandles,
    replica_id: &ReplicaId,
    topic_id: &str,
    object_id: &EnvelopeId,
    attempt: hydration_limits::MissingBodyAttempt,
) -> Result<Option<ObjectProjectionRow>> {
    match reflect_reply_target_with(
        services,
        object_id,
        replica_id,
        topic_id,
        ReplyTargetBody::Remote,
    )
    .await
    {
        Ok(ReplyTargetReflection::Reflected(row)) if row.content.is_some() => {
            attempt.succeed();
            Ok(Some(*row))
        }
        Ok(ReplyTargetReflection::Deferred) => {
            attempt.defer();
            Ok(None)
        }
        Ok(_) => {
            attempt.fail();
            Ok(None)
        }
        Err(error) => {
            attempt.fail();
            Err(error)
        }
    }
}

/// 返信先の反映を背景で行う task を起こす。台帳の確認は呼び出し側が済ませる。同時実行は台帳の permit で抑える。
fn spawn_reply_target_reflection(
    services: &ServiceHandles,
    replica_id: &ReplicaId,
    topic_id: &str,
    object_id: &EnvelopeId,
    permit: tokio::sync::OwnedSemaphorePermit,
    attempt: hydration_limits::MissingBodyAttempt,
) {
    let services = services.clone();
    let replica_id = replica_id.clone();
    let topic_id = topic_id.to_string();
    let object_id = object_id.clone();
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(error) = attempt_reply_target_reflection(
            &services,
            &replica_id,
            topic_id.as_str(),
            &object_id,
            attempt,
        )
        .await
        {
            warn!(object_id = %object_id.as_str(), error = %error, "failed to reflect a reply target in the background");
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
    /// 共有network受付前に容量・期限で延期された。
    Deferred,
}

/// 返信先を、返信と同じ replica から key 指定で反映する(#1239 AC-6 の背景の反映)。docs は手元だけを読み
/// (`DocFetchPolicy::LocalOnly`)、本文の blob は手元に無ければ remote から取る(`ReplyTargetBody::Remote`)。
///
/// 返信先も同じ replica にある投稿として、署名つき envelope と replica の scope を確かめてから反映する(#1248)。
/// 既に projection にあれば、その行を返す。
#[cfg(test)]
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
            ReplyTargetReflection::BodyNotLocal
            | ReplyTargetReflection::Unavailable
            | ReplyTargetReflection::Deferred => None,
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
    // A missing public source is an existing-namespace read only. Never let a
    // reply preview import a bucket merely to check whether its parent exists.
    let local_reader = (replica_id.as_str().starts_with("bucket::")
        && kukuri_docs_sync::post_replica_kind(replica_id)
            == Some(kukuri_docs_sync::PostReplicaKind::PublicTopic {
                topic_id: topic_id.to_owned(),
            }))
    .then(|| LocalSourceReader {
        docs: services.docs_sync.clone(),
        replica: replica_id.clone(),
    });
    if replica_id.as_str().starts_with("bucket::") && local_reader.is_none() {
        return Ok(ReplyTargetReflection::Unavailable);
    }
    let docs = local_reader
        .as_ref()
        .map_or(services.docs_sync.as_ref(), |reader| {
            reader as &dyn DocsSync
        });
    let Ok(Some(post)) = load_verified_post(
        docs,
        replica_id,
        topic_id,
        object_id,
        DocFetchPolicy::LocalOnly,
    )
    .await
    else {
        return Ok(ReplyTargetReflection::Unavailable);
    };
    let channel = post
        .header()
        .channel_id
        .as_ref()
        .map_or(PUBLIC_CHANNEL_ID, ChannelId::as_str)
        .to_owned();
    let Some(scope_generation) = services
        .active_content_scope_generation(topic_id, &channel)
        .await
    else {
        return Ok(ReplyTargetReflection::Deferred);
    };
    let is_withdrawn = services
        .projection_store
        .get_post_withdrawal(object_id)
        .await?
        .is_some();
    let mut remote_bytes = None;
    let row = if is_withdrawn {
        projection_row_from_post(&post.clone().withdrawn(), Some(String::new()))
    } else {
        let content = match (&post.header().payload_ref, body) {
            (PayloadRef::InlineText { text }, _) => Some(text.clone()),
            (PayloadRef::BlobText { hash, .. }, ReplyTargetBody::Remote) => {
                let deadline = tokio::time::Instant::now() + projection_blob_fetch_timeout();
                let Some(Ok(Ok(fetch))) = services
                    .until_content_invalid(
                        topic_id,
                        &channel,
                        scope_generation,
                        tokio::time::timeout_at(
                            deadline,
                            services.blob_service.prepare_retry_fetch(hash),
                        ),
                    )
                    .await
                else {
                    return Ok(ReplyTargetReflection::Deferred);
                };
                remote_bytes = services
                    .until_content_invalid(
                        topic_id,
                        &channel,
                        scope_generation,
                        tokio::time::timeout_at(deadline, fetch),
                    )
                    .await
                    .and_then(Result::ok)
                    .and_then(Result::ok)
                    .flatten();
                remote_bytes
                    .as_ref()
                    .map(|bytes| String::from_utf8_lossy(bytes).to_string())
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
    let _save_access = services.content_save_access.lock().await;
    if !services
        .content_scope_is_current(topic_id, &channel, scope_generation)
        .await
    {
        return Ok(ReplyTargetReflection::Deferred);
    }
    let row = if services
        .projection_store
        .get_post_withdrawal(object_id)
        .await?
        .is_some()
    {
        projection_row_from_post(&post.withdrawn(), Some(String::new()))
    } else {
        if let Some(bytes) = remote_bytes
            && let PayloadRef::BlobText { hash, .. } = &post.header().payload_ref
        {
            let stored = services
                .blob_service
                .put_remote_blob(bytes, "text/plain")
                .await?;
            anyhow::ensure!(stored.hash == *hash, "reply target body hash changed");
        }
        row
    };
    services.put_post_projection(row.clone()).await?;
    Ok(ReplyTargetReflection::Reflected(Box::new(row)))
}
