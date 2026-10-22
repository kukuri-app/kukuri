//! 保持する topic の対象ごとの取り込み(#1909)。記録、投稿の本文・添付・リンクプレビュー・リアクション、session と Dome、
//! 取り直しの予定の読み直し。保持した blob と、topic の bucket の外の record(Dome の owner の制御領域)には、対象を見つけた
//! 日(`bucket`)の保護参照を付ける。

use super::*;
use kukuri_core::{DomeHostingRecordV1, DomePresetStateDocV1};

impl AppService {
    /// 現在と前日の bucket で見つけた key 1 件を `reader` から反映する。`sessions` は、この一覧で読んだ session(state と
    /// locator の key で同じ session を 2 度読まない)。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn retain_scanned(
        &self,
        topic: &str,
        bucket: u64,
        replica: &ReplicaId,
        prefix: &str,
        entry: &DocKeyEntry,
        reader: &Arc<dyn DocsSync>,
        sessions: &mut HashSet<String>,
    ) -> Result<Scanned> {
        let projection_store = self.services.projection_store.as_ref();
        let key = entry.key.as_str();
        match prefix {
            "withdrawals/" => {
                let Some(id) = key
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_suffix("/state"))
                else {
                    return Ok(Scanned::Done);
                };
                let object_id = EnvelopeId::from(id);
                let Some(row) = projection_store.get_object_projection(&object_id).await? else {
                    return Ok(Scanned::Pending);
                };
                if projection_store
                    .get_post_withdrawal(&object_id)
                    .await?
                    .is_none()
                {
                    let read = n0_future::time::timeout(
                        REMOTE_READ_DEADLINE,
                        hydrate_post_withdrawal_for_object_with_hints(
                            reader.as_ref(),
                            projection_store,
                            replica,
                            &object_id,
                            WithdrawalReadHints {
                                target_docs_author: row.source_docs_author.as_deref(),
                                writer_docs_author: entry.docs_author.as_deref(),
                            },
                            DocFetchPolicy::LocalThenRemote,
                        ),
                    )
                    .await;
                    reader.finish_remote_object().await;
                    read.context("withdrawal read deadline elapsed")??;
                }
                Ok(Scanned::Done)
            }
            "reactions/" => {
                let Some(target) = ReactionKey::from_doc_key(key)
                    .filter(|_| key.ends_with("/envelope"))
                    .map(|reaction| reaction.target_object_id)
                else {
                    return Ok(Scanned::Done);
                };
                if projection_store
                    .get_object_projection(&target)
                    .await?
                    .is_none()
                {
                    return Ok(Scanned::Pending);
                }
                let reaction = self.read_reaction(topic, replica, key, reader).await?;
                Ok(
                    if self
                        .retain_reaction_images(topic, bucket, reaction.as_slice())
                        .await
                    {
                        Scanned::Done
                    } else {
                        Scanned::Retry(RETAIN_REACTION, key.to_owned())
                    },
                )
            }
            _ => {
                let kind = if prefix == "sessions/live/" {
                    "live-session"
                } else {
                    "game-session"
                };
                let Some(id) = session_key_id(prefix, key) else {
                    return Ok(Scanned::Done);
                };
                if !sessions.insert(id.to_owned()) {
                    return Ok(Scanned::Done);
                }
                Ok(if self.retain_session(topic, bucket, kind, id).await? {
                    Scanned::Done
                } else {
                    Scanned::Retry(kind, id.to_owned())
                })
            }
        }
    }

    /// 取り直しの予定にある対象を読み直す。戻り値は、対象と参照する blob をすべて取れたか。
    pub(super) async fn retain_retry(
        &self,
        topic: &str,
        retry: &TopicRetentionRetry,
    ) -> Result<bool> {
        let (kind, id) = (retry.object_kind.as_str(), retry.object_id.as_str());
        let replica = topic_bucket(topic, TimeBucket::from_index(retry.bucket)?)?;
        match kind {
            RETAIN_POST | RETAIN_CHANNEL => {
                let mut present = self.record_present(topic, kind, id).await?;
                if !present {
                    for reader in self
                        .remote_post_readers(topic, None, &replica, None)
                        .await?
                    {
                        let read = n0_future::time::timeout(
                            REMOTE_READ_DEADLINE,
                            self.retain_record(topic, &replica, kind, id, None, reader.clone()),
                        )
                        .await;
                        reader.finish_remote_object().await;
                        if matches!(read, Ok(Ok(true))) {
                            present = true;
                            break;
                        }
                    }
                }
                Ok(present && self.retain_content(topic, retry.bucket, kind, id).await)
            }
            RETAIN_REACTION => {
                for reader in self
                    .remote_post_readers(topic, None, &replica, None)
                    .await?
                {
                    if let Ok(reaction) = self.read_reaction(topic, &replica, id, &reader).await {
                        return Ok(self
                            .retain_reaction_images(topic, retry.bucket, reaction.as_slice())
                            .await);
                    }
                }
                Ok(false)
            }
            _ => self.retain_session(topic, retry.bucket, kind, id).await,
        }
    }

    /// 対象の記録が手元にあるか(公開スレッドは一覧の行)。
    async fn record_present(&self, topic: &str, kind: &str, id: &str) -> Result<bool> {
        let projection_store = self.services.projection_store.as_ref();
        Ok(if kind == RETAIN_CHANNEL {
            projection_store
                .get_public_channel_row(id)
                .await?
                .is_some_and(|row| row.topic_id == topic)
        } else {
            projection_store
                .get_object_projection(&EnvelopeId::from(id))
                .await?
                .is_some()
        })
    }

    /// 対象の記録(投稿の envelope と取り下げ、公開スレッドの metadata)を `reader` から取り込む。手元にある投稿は読まない。
    /// 公開スレッドは、手元の行より新しい版だけを反映する。戻り値は、対象が手元にあるか。
    pub(super) async fn retain_record(
        &self,
        topic: &str,
        replica: &ReplicaId,
        kind: &str,
        id: &str,
        docs_author: Option<&str>,
        reader: Arc<dyn DocsSync>,
    ) -> Result<bool> {
        if kind == RETAIN_CHANNEL {
            let known = self
                .services
                .projection_store
                .get_public_channel_row(id)
                .await?
                .filter(|row| row.topic_id == topic)
                .map(|row| row.revision);
            let read = self
                .read_public_channel_from(
                    reader,
                    topic,
                    &ChannelId::new(id),
                    DocFetchPolicy::LocalThenRemote,
                    known,
                )
                .await?;
            return Ok(read.is_some() || known.is_some());
        }
        if !self.record_present(topic, kind, id).await? {
            let mut services = self.services.clone();
            services.docs_sync = reader;
            hydrate_object_in_topic_with(
                &services,
                topic,
                replica,
                &EnvelopeId::from(id),
                docs_author,
                DocFetchPolicy::LocalThenRemote,
                BodyFetch::Bounded,
            )
            .await?;
        }
        self.record_present(topic, kind, id).await
    }

    /// 取り込んだ対象が参照する blob などを保持する(投稿だけ)。戻り値は、すべて取れたか。
    pub(super) async fn retain_content(
        &self,
        topic: &str,
        bucket: u64,
        kind: &str,
        id: &str,
    ) -> bool {
        if kind != RETAIN_POST {
            return true;
        }
        self.retain_post_content(topic, bucket, &EnvelopeId::from(id))
            .await
            .unwrap_or_else(|error| {
                warn!(topic, %error, "retention will retry a post's content later");
                false
            })
    }

    /// 投稿の本文・添付・リンクプレビュー・リアクション(custom の画像)を取って保持する。取り下げた投稿は何も取らない。
    /// 戻り値は、参照する blob をすべて取れたか。
    async fn retain_post_content(
        &self,
        topic: &str,
        bucket: u64,
        object_id: &EnvelopeId,
    ) -> Result<bool> {
        let projection_store = self.services.projection_store.as_ref();
        let Some(row) = projection_store.get_object_projection(object_id).await? else {
            return Ok(false);
        };
        if projection_store
            .get_post_withdrawal(object_id)
            .await?
            .is_some()
        {
            return Ok(true);
        }
        // 本文の blob には、取る前に日の保護参照を付ける(手元に本文があっても付ける)。
        if let PayloadRef::BlobText { hash, .. } = &row.payload_ref {
            self.claim_retained_blob(topic, bucket, hash.as_str())
                .await?;
        }
        let body = match &row.content {
            Some(text) => Some(text.clone()),
            None => self.retain_post_body(&row).await?,
        };
        let mut complete = body.is_some();
        for asset in &row.attachments {
            complete &= self
                .retain_attachment(topic, bucket, object_id, asset)
                .await?;
        }
        // リンクプレビューの record は、本文にある URL についてだけ書かれる。
        if body
            .as_deref()
            .is_some_and(|body| body.contains("http://") || body.contains("https://"))
        {
            let preview = self.read_link_preview(object_id, None).await?;
            if let Some(image) = preview
                .as_ref()
                .and_then(|(content, _)| content.image.as_ref())
            {
                self.claim_retained_blob(topic, bucket, image.hash.as_str())
                    .await?;
            }
            complete &=
                preview.is_none_or(|(content, image)| content.image.is_none() || image.is_some());
        }
        complete &= self.retain_post_reactions(topic, bucket, &row).await?;
        Ok(complete)
    }

    /// 本文の blob を取って行へ反映する(表示の取り直しと同じ保存)。戻り値は、反映した本文。
    async fn retain_post_body(&self, row: &ObjectProjectionRow) -> Result<Option<String>> {
        let PayloadRef::BlobText { hash, .. } = &row.payload_ref else {
            return Ok(None);
        };
        let services = &self.services;
        let Some(generation) = services
            .active_content_scope_generation(&row.topic_id, &row.channel_id)
            .await
        else {
            return Ok(None);
        };
        let Some(bytes) = fetch_manifest_blob(services.blob_service.as_ref(), hash).await else {
            return Ok(None);
        };
        let _save_access = services.content_save_access.lock().await;
        if !services
            .content_scope_is_current(&row.topic_id, &row.channel_id, generation)
            .await
        {
            return Ok(None);
        }
        store_recovered_body(services, &row.object_id, hash, bytes).await?;
        Ok(services
            .projection_store
            .get_object_projection(&row.object_id)
            .await?
            .and_then(|row| row.content))
    }

    /// 投稿の添付を取って cache に置く。成人向けで表示設定が OFF の添付は取らない。大きな添付(動画)も扱えるよう、cache と
    /// 同じ volume の dir で file として受け取る(native だけ。Web は保持しない)。戻り値は、取らない添付を含めて済んだか。
    async fn retain_attachment(
        &self,
        topic: &str,
        bucket: u64,
        object_id: &EnvelopeId,
        asset: &kukuri_core::AssetRef,
    ) -> Result<bool> {
        if self
            .post_media_withheld(&asset.hash, object_id.as_str())
            .await?
        {
            return Ok(true);
        }
        // 取る前に日の保護参照を付ける(動画のような大きな blob も、cache の容量の計数の外に置く)。
        self.claim_retained_blob(topic, bucket, asset.hash.as_str())
            .await?;
        if self
            .services
            .blob_service
            .local_blob_status(&asset.hash)
            .await?
            != BlobStatus::Missing
        {
            return Ok(true);
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let staging = &self.topic_retention_handles()?.staging;
            tokio::fs::create_dir_all(staging).await?;
            let path = staging.join(asset.hash.as_str());
            let fetched = n0_future::time::timeout(
                blob_deadline(asset.bytes),
                self.blob_media_file_for_post(asset.hash.as_str(), Some(object_id.as_str()), &path),
            )
            .await;
            let _ = tokio::fs::remove_file(&path).await;
            Ok(matches!(fetched, Ok(Ok(Some(_)))))
        }
        #[cfg(target_family = "wasm")]
        Ok(false)
    }

    /// 投稿の日と翌日の bucket から、投稿へのリアクションを上限つきで反映し、custom の画像を保持する。戻り値は、画像を
    /// すべて取れたか。
    async fn retain_post_reactions(
        &self,
        topic: &str,
        bucket: u64,
        row: &ObjectProjectionRow,
    ) -> Result<bool> {
        let Ok(posted) =
            BucketReplica::parse(&row.source_replica_id).map(|replica| replica.bucket())
        else {
            return Ok(true);
        };
        let current = TimeBucket::from_unix_seconds(Utc::now().timestamp())?;
        let mut complete = true;
        for day in [
            Some(posted),
            TimeBucket::from_index(posted.index() + 1).ok(),
        ]
        .into_iter()
        .flatten()
        .filter(|day| *day <= current)
        {
            let replica = topic_bucket(topic, day)?;
            for reader in self
                .remote_post_readers(topic, None, &replica, None)
                .await?
            {
                let read = n0_future::time::timeout(
                    REMOTE_READ_DEADLINE,
                    hydrate_reaction_cache_for_target_bounded(
                        reader.as_ref(),
                        self.services.projection_store.as_ref(),
                        self.services.blob_service.as_ref(),
                        &self.services.reaction_changes,
                        topic,
                        &replica,
                        &row.object_id,
                        DocFetchPolicy::LocalThenRemote,
                        RETENTION_POST_REACTIONS,
                    ),
                )
                .await;
                reader.finish_remote_object().await;
                if let Ok(Ok(reactions)) = read {
                    complete &= self.retain_reaction_images(topic, bucket, &reactions).await;
                    break;
                }
            }
        }
        Ok(complete)
    }

    /// reaction の key 1 件を `reader` から反映する。戻り値は、反映した reaction。
    async fn read_reaction(
        &self,
        topic: &str,
        replica: &ReplicaId,
        key: &str,
        reader: &Arc<dyn DocsSync>,
    ) -> Result<Option<VerifiedReaction>> {
        let read = n0_future::time::timeout(
            REMOTE_READ_DEADLINE,
            hydrate_reaction_cache_from_key(
                reader.as_ref(),
                self.services.projection_store.as_ref(),
                self.services.blob_service.as_ref(),
                &self.services.reaction_changes,
                topic,
                replica,
                key,
                DocFetchPolicy::LocalThenRemote,
            ),
        )
        .await;
        reader.finish_remote_object().await;
        read.context("reaction read deadline elapsed")?
    }

    /// reaction の custom の画像を保持する。戻り値は、すべて取れたか。
    async fn retain_reaction_images(
        &self,
        topic: &str,
        bucket: u64,
        reactions: &[VerifiedReaction],
    ) -> bool {
        let mut complete = true;
        for reaction in reactions {
            if let Some(asset) = &reaction.doc().custom_asset_snapshot {
                complete &= self
                    .retain_blob(topic, bucket, &asset.blob_hash, asset.bytes)
                    .await
                    .unwrap_or(false);
            }
        }
        complete
    }

    /// session を読んで保持する(読み直しと同じ読み。新しい版だけを保持する)。Dome の session は、Instance・hosting・
    /// Preset と資材も保持する。戻り値は、session と参照する blob をすべて取れたか。
    async fn retain_session(&self, topic: &str, bucket: u64, kind: &str, id: &str) -> Result<bool> {
        if !self
            .read_session(topic, &TimelineScope::Public, id, kind)
            .await?
        {
            return Ok(false);
        }
        if kind != "game-session" {
            // session の manifest の blob に日の保護参照を付ける(Dome は `retain_dome` で付ける)。
            if let Some(row) = self
                .services
                .projection_store
                .get_live_session(topic, id)
                .await?
            {
                self.claim_retained_blob(topic, bucket, row.manifest_blob_hash.as_str())
                    .await?;
            }
            return Ok(true);
        }
        self.retain_dome(topic, bucket, id).await
    }

    /// Dome の session が指す、owner の制御領域の Instance・hosting・Preset と資材を保持する。session と Preset の
    /// manifest、資材の blob と、owner の制御領域の record に日の保護参照を付ける。private の context の Dome は保持しない。
    /// 戻り値は、Preset と資材を取れたか。
    async fn retain_dome(&self, topic: &str, bucket: u64, room_id: &str) -> Result<bool> {
        let Some(row) = self
            .services
            .projection_store
            .get_game_room(topic, room_id)
            .await?
        else {
            return Ok(true);
        };
        self.claim_retained_blob(topic, bucket, row.manifest_blob_hash.as_str())
            .await?;
        let Some(metaverse) = row.metaverse else {
            return Ok(true);
        };
        let context = &metaverse.spatial_context;
        if public_dome_topic(context).is_none() {
            return Ok(true);
        }
        // room の ID は Dome の Instance の ID。
        let owner = Pubkey::from(row.host_pubkey.as_str());
        let mut owner_keys = vec![
            dome_instance_state_key(room_id),
            crate::dome_hosting::hosting_state_key(room_id),
        ];
        if let Some((state, instance)) = self
            .fetch_dome_instance_manifest_kept(context, &owner, true)
            .await?
        {
            owner_keys.push(stable_key("envelopes", state.last_envelope_id.as_str()));
            if instance.status == kukuri_core::DomeInstanceStatusV1::Active
                && instance.relationship_detach.is_none()
            {
                self.retain_dome_hosting(&instance).await?;
            }
        }
        self.claim_retained_records(
            topic,
            bucket,
            &author_replica_id(owner.as_str()),
            &owner_keys,
        )
        .await?;
        let preset = &metaverse.preset_ref;
        let mut complete = self
            .fetch_dome_preset_manifest(preset, context, true)
            .await?
            .is_some();
        // Preset の envelope の key は、保持した版の state から読む。
        let preset_replica = author_replica_id(preset.owner_pubkey.as_str());
        let preset_key = dome_preset_state_key(preset);
        let envelope = self
            .services
            .docs_sync
            .query_local_source(&preset_replica, &preset_key, None, 8)
            .await
            .unwrap_or_default()
            .iter()
            .find_map(|record| serde_json::from_slice::<DomePresetStateDocV1>(&record.value).ok())
            .map(|state| stable_key("envelopes", state.last_envelope_id.as_str()));
        let preset_keys = std::iter::once(preset_key)
            .chain(envelope)
            .collect::<Vec<_>>();
        self.claim_retained_records(topic, bucket, &preset_replica, &preset_keys)
            .await?;
        self.claim_retained_blob(topic, bucket, &preset.manifest_blob_hash)
            .await?;
        for asset in &metaverse.asset_refs {
            let hash = kukuri_core::BlobHash::new(asset.blob_hash.clone());
            complete &= self
                .retain_blob(topic, bucket, &hash, asset.size_bytes.unwrap_or(0))
                .await?;
        }
        Ok(complete)
    }

    /// hosting の記録を、手元に保持した記録で済ませずに読み先から読み、検証した記録を、手元に保持した記録より古くなければ
    /// 保持して他の参加者へ提供する。`instance` は Active の Instance(非 Active の Instance は記録を検証しない)。
    async fn retain_dome_hosting(&self, instance: &DomeInstanceManifestV1) -> Result<()> {
        let key = crate::dome_hosting::hosting_state_key(&instance.instance_id);
        self.read_dome_record(
            &instance.spatial_context,
            &instance.owner_pubkey,
            |docs, replica, policy| {
                let key = key.clone();
                async move {
                    if policy == DocFetchPolicy::LocalOnly {
                        return Ok(None);
                    }
                    let Some(record) = docs
                        .query_replica_with_policy(&replica, DocQuery::Exact(key.clone()), policy)
                        .await?
                        .into_iter()
                        .next()
                    else {
                        return Ok(None);
                    };
                    let records: Vec<DomeHostingRecordV1> = serde_json::from_slice(&record.value)?;
                    let now = Utc::now().timestamp_millis();
                    kukuri_core::resolve_dome_hosting_state(instance, &records, now, Some(now))?;
                    // 手元に保持した記録(他の名義の record を含む)と比べる。
                    let kept = self
                        .services
                        .docs_sync
                        .query_local_source(&replica, &key, None, 8)
                        .await?
                        .iter()
                        .filter_map(|record| {
                            serde_json::from_slice::<Vec<DomeHostingRecordV1>>(&record.value).ok()
                        })
                        .map(|kept| hosting_progress(&kept))
                        .max();
                    if kept.is_some_and(|kept| kept > hosting_progress(&records)) {
                        return Ok(None);
                    }
                    docs.persist_verified_record(
                        &replica,
                        &key,
                        record.docs_author.as_deref(),
                        &[],
                    )
                    .await?;
                    Ok(Some(()))
                }
            },
        )
        .await?;
        Ok(())
    }

    /// 保持する日の保護参照を blob に付ける。取る前に付けると、届いた内容は cache の容量の計数と回収の外に置かれる。
    async fn claim_retained_blob(&self, topic: &str, bucket: u64, hash: &str) -> Result<()> {
        self.topic_retention_handles()?
            .store
            .claim_topic_retention_blob(topic, bucket, hash)
            .await
    }

    /// 保持する日の保護参照を、`replica` の `keys` の record(すべての名義)に付ける。
    async fn claim_retained_records(
        &self,
        topic: &str,
        bucket: u64,
        replica: &ReplicaId,
        keys: &[String],
    ) -> Result<()> {
        let store = &self.topic_retention_handles()?.store;
        for key in keys {
            store
                .claim_topic_retention_records(topic, bucket, replica.as_str(), key)
                .await?;
        }
        Ok(())
    }

    /// blob に日の保護参照を付け、取って cache に置く(他の参加者へ提供する)。手元にあれば取らない。戻り値は、手元に
    /// あるか。
    async fn retain_blob(
        &self,
        topic: &str,
        bucket: u64,
        hash: &kukuri_core::BlobHash,
        bytes: u64,
    ) -> Result<bool> {
        self.claim_retained_blob(topic, bucket, hash.as_str())
            .await?;
        let blobs = self.services.blob_service.as_ref();
        if blobs.local_blob_status(hash).await? != BlobStatus::Missing {
            return Ok(true);
        }
        let Ok(Ok(Some(data))) =
            n0_future::time::timeout(blob_deadline(bytes), blobs.fetch_blob(hash)).await
        else {
            return Ok(false);
        };
        let stored = blobs
            .put_remote_blob(data, "application/octet-stream")
            .await?;
        anyhow::ensure!(stored.hash == *hash, "retained blob hash changed");
        Ok(true)
    }
}

/// hosting の記録の進み具合(最後の epoch と、その epoch の記録の数)。owner は epoch を進めながら記録を足す。
fn hosting_progress(records: &[DomeHostingRecordV1]) -> (u64, usize) {
    let epoch =
        |record: &DomeHostingRecordV1| crate::dome_hosting::hosting_record_identity(record).0;
    let latest = records.iter().map(epoch).max().unwrap_or(0);
    (
        latest,
        records
            .iter()
            .filter(|record| epoch(record) == latest)
            .count(),
    )
}
