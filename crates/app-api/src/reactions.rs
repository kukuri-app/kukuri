use crate::service::*;

impl AppService {
    pub async fn toggle_reaction(
        &self,
        target_topic_id: &str,
        target_object_id: &str,
        reaction_key: ReactionKeyV1,
        channel_ref: Option<ChannelRef>,
    ) -> Result<ReactionStateView> {
        let target_topic_id = TopicId::new(target_topic_id);
        self.ensure_topic_subscription(target_topic_id.as_str())
            .await?;
        let scope = match channel_ref.as_ref() {
            Some(ChannelRef::PrivateChannel { channel_id }) => {
                self.private_channel_write_state(target_topic_id.as_str(), channel_id)
                    .await?;
                TimelineScope::Channel {
                    channel_id: channel_id.clone(),
                }
            }
            Some(ChannelRef::Public) | None => TimelineScope::Public,
        };
        let target_object_id = EnvelopeId::from(target_object_id);
        self.ensure_object_projection(
            target_topic_id.as_str(),
            &scope,
            &target_object_id,
            DocFetchPolicy::LocalOnly,
        )
        .await?;
        let target = self
            .services
            .projection_store
            .get_object_projection(&target_object_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("reaction target was not found"))?;
        if !matches!(target.object_kind.as_str(), "post" | "comment") {
            anyhow::bail!("reaction target must be a post or comment");
        }
        if target.topic_id != target_topic_id.as_str() {
            anyhow::bail!("reaction target topic does not match");
        }
        let target_channel_id = channel_id_from_storage(target.channel_id.as_str());
        match (channel_ref.as_ref(), target_channel_id.as_ref()) {
            (Some(ChannelRef::Public), None) | (None, None) => {}
            (Some(ChannelRef::PrivateChannel { channel_id }), Some(target_channel_id))
                if channel_id == target_channel_id => {}
            (None, Some(_)) => {}
            _ => anyhow::bail!("reaction channel does not match the target object"),
        }
        let current_author = Pubkey::from(self.current_author_pubkey());
        let normalized_reaction_key = reaction_key.normalized_key()?;
        let reaction_id = deterministic_reaction_id(
            &target.source_replica_id,
            &target_object_id,
            &current_author,
            normalized_reaction_key.as_str(),
        );
        // #1239: 自分の既存の reaction が projection に無ければ、その key だけを docs から反映する
        // (reaction id は対象・著者・key から決まる)。replica は走査しない。
        if self
            .services
            .projection_store
            .get_reaction_cache(&target.source_replica_id, &target_object_id, &reaction_id)
            .await?
            .is_none()
        {
            hydrate_reaction_cache_from_key(
                self.services.docs_sync.as_ref(),
                self.services.projection_store.as_ref(),
                &target.source_replica_id,
                stable_key(
                    "reactions",
                    &format!(
                        "{}/{}/state",
                        target_object_id.as_str(),
                        reaction_id.as_str()
                    ),
                )
                .as_str(),
                // 利用者の操作は remote 取得で待たせない(ADR 0052 §4)。
                DocFetchPolicy::LocalOnly,
            )
            .await?;
        }
        let next_status = match self
            .services
            .projection_store
            .get_reaction_cache(&target.source_replica_id, &target_object_id, &reaction_id)
            .await?
        {
            Some(existing) if existing.status == ObjectStatus::Active => ObjectStatus::Deleted,
            _ => ObjectStatus::Active,
        };
        let envelope = build_reaction_envelope(
            self.services.keys.as_ref(),
            &target_topic_id,
            target_channel_id.as_ref(),
            &target_object_id,
            reaction_key,
            &reaction_id,
            next_status.clone(),
        )?;
        let reaction = parse_reaction(&envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse reaction envelope"))?;
        persist_reaction_doc(
            self.services.docs_sync.as_ref(),
            &target.source_replica_id,
            &reaction,
            &envelope,
        )
        .await?;
        self.services.store.put_envelope(envelope.clone()).await?;
        self.services
            .projection_store
            .upsert_reaction_cache(reaction_projection_row_from_doc(
                &reaction,
                &target.source_replica_id,
            ))
            .await?;
        if let Err(error) = self
            .services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(target_topic_id.as_str(), target_channel_id.as_ref()),
                GossipHint::TopicObjectsChanged {
                    topic_id: target_topic_id.clone(),
                    objects: vec![HintObjectRef {
                        object_id: target_object_id.as_str().to_string(),
                        object_kind: "reaction".into(),
                    }],
                },
            )
            .await
        {
            warn!(
                topic = %target_topic_id.as_str(),
                object_id = %target_object_id.as_str(),
                error = %error,
                "failed to publish reaction hint; durable docs state was already persisted"
            );
        }
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        self.reaction_state_for_target(&target.source_replica_id, &target_object_id)
            .await
    }

    pub async fn create_custom_reaction_asset(
        &self,
        input: CreateCustomReactionAssetInput,
    ) -> Result<CustomReactionAssetView> {
        let stored_blob = self
            .services
            .blob_service
            .put_blob(input.bytes, input.mime.as_str())
            .await?;
        let envelope = build_custom_reaction_asset_envelope(
            self.services.keys.as_ref(),
            stored_blob.hash.clone(),
            input.search_key,
            input.mime,
            stored_blob.bytes,
            input.width,
            input.height,
        )?;
        let asset = parse_custom_reaction_asset(&envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse custom reaction asset envelope"))?;
        persist_custom_reaction_asset_doc(self.services.docs_sync.as_ref(), &asset, &envelope)
            .await?;
        self.services.store.put_envelope(envelope).await?;
        self.services
            .projection_store
            .mark_blob_status(&stored_blob.hash, BlobCacheStatus::Available)
            .await?;
        *self.last_sync_ts.lock().await = Some(Utc::now().timestamp_millis());
        Ok(custom_reaction_asset_view_from_doc(&asset))
    }

    pub async fn list_my_custom_reaction_assets(&self) -> Result<Vec<CustomReactionAssetView>> {
        let author_pubkey = self.current_author_pubkey();
        let mut items = load_custom_reaction_assets_from_author_replica(
            self.services.docs_sync.as_ref(),
            &author_pubkey,
        )
        .await?;
        items.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.asset_id.cmp(&left.asset_id))
        });
        Ok(items
            .into_iter()
            .map(|asset| custom_reaction_asset_view_from_doc(&asset))
            .collect())
    }

    pub async fn list_recent_reactions(&self, limit: usize) -> Result<Vec<RecentReactionView>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let author_pubkey = self.current_author_pubkey();
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        for row in self
            .services
            .projection_store
            .list_recent_reaction_cache_by_author(author_pubkey.as_str())
            .await?
        {
            if !seen.insert(row.normalized_reaction_key.clone()) {
                continue;
            }
            items.push(recent_reaction_view_from_projection(&row));
            if items.len() >= limit {
                break;
            }
        }
        Ok(items)
    }

    pub async fn list_bookmarked_custom_reactions(
        &self,
    ) -> Result<Vec<BookmarkedCustomReactionView>> {
        Ok(self
            .services
            .projection_store
            .list_bookmarked_custom_reactions()
            .await?
            .into_iter()
            .map(bookmarked_custom_reaction_view_from_row)
            .collect())
    }

    pub async fn bookmark_custom_reaction(
        &self,
        asset: CustomReactionAssetSnapshotV1,
    ) -> Result<BookmarkedCustomReactionView> {
        if asset.owner_pubkey.as_str() == self.current_author_pubkey() {
            anyhow::bail!("bookmarking your own custom reaction is not supported");
        }
        let row = BookmarkedCustomReactionRow {
            asset_id: asset.asset_id.clone(),
            owner_pubkey: asset.owner_pubkey.as_str().to_string(),
            blob_hash: asset.blob_hash,
            search_key: search_key_or_asset_id(asset.search_key.as_str(), asset.asset_id.as_str()),
            mime: asset.mime,
            bytes: asset.bytes,
            width: asset.width,
            height: asset.height,
            bookmarked_at: Utc::now().timestamp_millis(),
        };
        self.services
            .projection_store
            .put_bookmarked_custom_reaction(row.clone())
            .await?;
        Ok(bookmarked_custom_reaction_view_from_row(row))
    }

    pub async fn remove_bookmarked_custom_reaction(&self, asset_id: &str) -> Result<()> {
        self.services
            .projection_store
            .remove_bookmarked_custom_reaction(asset_id)
            .await
    }
}
