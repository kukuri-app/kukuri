use crate::service::*;

struct MediaSourceAccess {
    object_id: EnvelopeId,
    topic: String,
    channel: String,
    generation: u64,
    adult_labeled: bool,
}

struct MediaAccess {
    source: Option<MediaSourceAccess>,
}

impl AppService {
    /// #858: 成人向け表現の表示設定(既定 OFF)。desktop-runtime が永続値を起動時に
    /// 反映し、設定変更時にも呼ぶ。
    ///
    /// #1419: OFF では、ON の間に cache へ置いた成人向けの添付を背景で上限つきに消す。起動時の反映でも走るので、
    /// 途中で止まった削除はそこで再開する。tokio runtime の中から呼ぶ(desktop の Tauri command は async)。
    pub fn set_adult_content_display_enabled(&self, enabled: bool) {
        self.adult_content_display_enabled
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
        if enabled {
            return;
        }
        let flag = self.adult_content_display_enabled.clone();
        let blob_service = self.services.blob_service.clone();
        let save_access = self.services.content_save_access.clone();
        tokio::spawn(async move {
            // 設定を見てから保存を終えるまでの表示取得(保存用の lock の中)を待ち、その保存も消す。
            drop(save_access.lock().await);
            while !flag.load(std::sync::atomic::Ordering::SeqCst) {
                match blob_service.forget_adult_media_step().await {
                    Ok(0) => break,
                    Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                    Err(error) => {
                        tracing::warn!(%error, "failed to forget cached adult media");
                        break;
                    }
                }
            }
        });
    }

    pub fn adult_content_display_enabled(&self) -> bool {
        self.adult_content_display_enabled
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// #1055: Community Node の content advisory が付いた添付 blob hash を取得ゲートへ登録する
    /// (insert-only)。desktop-runtime が index 応答の advisory を issuer 照合した後に呼ぶ。
    /// self-label 由来の hash と違い永続化しない(ADR 0028 §8.10 の transient 分類)。
    pub async fn register_advisory_media_hashes(&self, hashes: &[String]) {
        if hashes.is_empty() {
            return;
        }
        let mut registered = self.advisory_media_hashes.lock().await;
        for hash in hashes {
            let hash = hash.trim();
            if hash.is_empty() {
                continue;
            }
            registered.insert(hash.to_string());
        }
    }

    /// #1055: 対象 hash が advisory 付き添付として観測済みか。
    pub async fn is_advisory_media_hash(&self, hash: &str) -> bool {
        self.advisory_media_hashes.lock().await.contains(hash)
    }

    pub async fn blob_media_payload(
        &self,
        hash: &str,
        mime: &str,
    ) -> Result<Option<BlobMediaPayload>> {
        self.blob_media_payload_for_post(hash, mime, None).await
    }

    pub async fn blob_media_payload_for_post(
        &self,
        hash: &str,
        mime: &str,
        source_object_id: Option<&str>,
    ) -> Result<Option<BlobMediaPayload>> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(None);
        }
        let blob_hash = kukuri_core::BlobHash::new(hash.to_string());
        let Some(access) = self
            .media_access_for_post(&blob_hash, source_object_id)
            .await?
        else {
            return Ok(None);
        };
        let Some(bytes) = self.fetch_display_media_bytes(&blob_hash, &access).await? else {
            return Ok(None);
        };
        let _save_access = self.services.content_save_access.lock().await;
        if !self.media_access_still_valid(&blob_hash, &access).await? {
            return Ok(None);
        }
        if !self.media_is_adult(&blob_hash, &access).await?
            && self
                .services
                .blob_service
                .local_blob_status(&blob_hash)
                .await?
                == BlobStatus::Missing
        {
            let stored = self
                .services
                .blob_service
                .put_remote_blob(bytes.clone(), mime)
                .await?;
            anyhow::ensure!(
                stored.hash == blob_hash,
                "media hash changed before storage"
            );
        }
        Ok(Some(BlobMediaPayload {
            bytes_base64: BASE64_STANDARD.encode(bytes),
            mime: mime.to_string(),
        }))
    }

    pub async fn blob_media_file_for_post(
        &self,
        hash: &str,
        source_object_id: Option<&str>,
        path: &std::path::Path,
    ) -> Result<Option<u64>> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(None);
        }
        let blob_hash = kukuri_core::BlobHash::new(hash.to_string());
        let Some(access) = self
            .media_access_for_post(&blob_hash, source_object_id)
            .await?
        else {
            return Ok(None);
        };
        let fetch = self
            .services
            .blob_service
            .fetch_blob_ephemeral_to_file(&blob_hash, path);
        let result = if let Some(source) = &access.source {
            self.services
                .until_content_invalid(&source.topic, &source.channel, source.generation, fetch)
                .await
                .unwrap_or(Ok(None))
        } else {
            self.services
                .until_content_invalid("", PUBLIC_CHANNEL_ID, 0, fetch)
                .await
                .unwrap_or(Ok(None))
        }?;
        let Some(length) = result else {
            return Ok(None);
        };
        let _save_access = self.services.content_save_access.lock().await;
        if !self.media_access_still_valid(&blob_hash, &access).await? {
            return Ok(None);
        }
        // #1419: 成人向けの添付は、表示設定 ON の間だけ印を付けて置く(OFF に戻したら消す)。
        let adult = self.media_is_adult(&blob_hash, &access).await?;
        if (!adult || self.adult_content_display_enabled())
            && self
                .services
                .blob_service
                .local_blob_status(&blob_hash)
                .await?
                == BlobStatus::Missing
        {
            self.services
                .blob_service
                .put_remote_blob_file(path, &blob_hash, adult)
                .await?;
        }
        Ok(Some(length))
    }

    async fn media_access_for_post(
        &self,
        hash: &kukuri_core::BlobHash,
        source_object_id: Option<&str>,
    ) -> Result<Option<MediaAccess>> {
        let source = if let Some(object_id) = source_object_id {
            let object_id = EnvelopeId::from(object_id);
            let Some(row) = self
                .services
                .projection_store
                .get_object_projection(&object_id)
                .await?
            else {
                return Ok(None);
            };
            if !row
                .attachments
                .iter()
                .any(|attachment| attachment.hash == *hash)
                || self
                    .services
                    .projection_store
                    .get_post_withdrawal(&object_id)
                    .await?
                    .is_some()
            {
                return Ok(None);
            }
            let Some(generation) = self
                .services
                .active_content_scope_generation(&row.topic_id, &row.channel_id)
                .await
            else {
                return Ok(None);
            };
            Some(MediaSourceAccess {
                object_id,
                topic: row.topic_id,
                channel: row.channel_id,
                generation,
                adult_labeled: kukuri_core::has_adult_content_label(&row.content_labels),
            })
        } else {
            None
        };
        let access = MediaAccess { source };
        if self.media_is_adult(hash, &access).await? && !self.adult_content_display_enabled() {
            return Ok(None);
        }
        Ok(Some(access))
    }

    async fn media_is_adult(
        &self,
        hash: &kukuri_core::BlobHash,
        access: &MediaAccess,
    ) -> Result<bool> {
        Ok(self
            .services
            .projection_store
            .is_adult_media_hash(hash)
            .await?
            || self.is_advisory_media_hash(hash.as_str()).await
            || access
                .source
                .as_ref()
                .is_some_and(|source| source.adult_labeled))
    }

    async fn fetch_display_media_bytes(
        &self,
        hash: &kukuri_core::BlobHash,
        access: &MediaAccess,
    ) -> Result<Option<Vec<u8>>> {
        if let Some(source) = &access.source {
            match self
                .services
                .until_content_invalid(
                    &source.topic,
                    &source.channel,
                    source.generation,
                    self.services.blob_service.prepare_display_fetch(hash),
                )
                .await
            {
                Some(Ok(fetch)) => self
                    .services
                    .until_content_invalid(&source.topic, &source.channel, source.generation, fetch)
                    .await
                    .unwrap_or(Ok(None)),
                Some(Err(error)) => Err(error),
                None => Ok(None),
            }
        } else {
            self.services
                .until_content_invalid(
                    "",
                    PUBLIC_CHANNEL_ID,
                    0,
                    self.services.blob_service.fetch_blob_ephemeral(hash),
                )
                .await
                .unwrap_or(Ok(None))
        }
    }

    async fn media_access_still_valid(
        &self,
        hash: &kukuri_core::BlobHash,
        access: &MediaAccess,
    ) -> Result<bool> {
        if *self.services.content_closed.borrow()
            || (self.media_is_adult(hash, access).await? && !self.adult_content_display_enabled())
        {
            return Ok(false);
        }
        let Some(source) = &access.source else {
            return Ok(true);
        };
        Ok(self
            .services
            .content_scope_is_current(&source.topic, &source.channel, source.generation)
            .await
            && self
                .services
                .projection_store
                .get_post_withdrawal(&source.object_id)
                .await?
                .is_none()
            && self
                .services
                .projection_store
                .get_object_projection(&source.object_id)
                .await?
                .is_some_and(|row| {
                    row.topic_id == source.topic
                        && row.channel_id == source.channel
                        && row
                            .attachments
                            .iter()
                            .any(|attachment| attachment.hash == *hash)
                }))
    }

    pub async fn blob_preview_data_url(&self, hash: &str, mime: &str) -> Result<Option<String>> {
        let Some(payload) = self.blob_media_payload(hash, mime).await? else {
            return Ok(None);
        };
        Ok(Some(format!(
            "data:{};base64,{}",
            payload.mime, payload.bytes_base64
        )))
    }
}
