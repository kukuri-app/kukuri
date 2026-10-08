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
                self.services.blob_service.as_ref(),
                target_topic_id.as_str(),
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
        // 切替後は作成時の bucket へ書く(元投稿の bucket へは追記しない、ADR 0054 §2)。
        let write_replica = if self.services.writes_buckets() {
            let private = match target_channel_id.as_ref() {
                Some(channel_id) => Some(
                    self.private_channel_write_state(target_topic_id.as_str(), channel_id)
                        .await?,
                ),
                None => None,
            };
            self.services.scope_write_replica(
                target_topic_id.as_str(),
                private.as_ref(),
                // reaction の envelope の時刻はミリ秒。
                envelope.created_at / 1_000,
            )?
        } else {
            target.source_replica_id.clone()
        };
        // 自分の reaction も、docs から反映するときと同じ検証を通す(#1252)。docs は読まない。
        let verified =
            VerifiedReaction::verify_local(&envelope, &target.source_replica_id, &write_replica)
                .map_err(|reason| anyhow::anyhow!("reaction was rejected: {}", reason.as_str()))?;
        let reaction = verified.doc().clone();
        persist_reaction_doc(
            self.services.docs_sync.as_ref(),
            &write_replica,
            &reaction,
            &envelope,
        )
        .await?;
        self.services.store.put_envelope(envelope.clone()).await?;
        self.services
            .projection_store
            .upsert_reaction_cache(reaction_projection_row(&verified))
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
                        docs_author: None,
                        sent_at: Some(Utc::now().timestamp_millis()),
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
        self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
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
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        let envelope = build_custom_reaction_asset_envelope_with_docs_author(
            self.services.keys.as_ref(),
            stored_blob.hash.clone(),
            input.search_key,
            input.mime,
            stored_blob.bytes,
            input.width,
            input.height,
            docs_author.as_deref(),
        )?;
        let asset = parse_custom_reaction_asset(&envelope)?
            .ok_or_else(|| anyhow::anyhow!("failed to parse custom reaction asset envelope"))?;
        persist_custom_reaction_asset_doc(self.services.docs_sync.as_ref(), &asset, &envelope)
            .await?;
        self.services
            .persist_author_event(asset.author_pubkey.as_str(), &envelope)
            .await?;
        self.services.store.put_envelope(envelope).await?;
        self.last_sync_ts.set(Utc::now().timestamp_millis()).await;
        Ok(custom_reaction_asset_view_from_doc(&asset))
    }

    pub async fn list_my_custom_reaction_assets(&self) -> Result<Vec<CustomReactionAssetView>> {
        let author_pubkey = self.current_author_pubkey();
        let docs_author = self.services.docs_sync.local_docs_author().await?;
        let mut items = load_custom_reaction_assets_from_author_replica(
            self.services.docs_sync.as_ref(),
            &author_pubkey,
            docs_author.as_deref(),
        )
        .await?;
        items.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.asset_id.cmp(&left.asset_id))
        });
        // 同じ画像＋検索名を作り直した asset は、同じ ID の 1 件にまとめる（#1232 D1）。
        let mut seen = BTreeSet::new();
        Ok(items
            .into_iter()
            .map(|asset| custom_reaction_asset_view_from_doc(&asset))
            .filter(|asset| seen.insert(asset.asset_id.clone()))
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
            let item = recent_reaction_view_from_projection(&row);
            if !seen.insert(item.normalized_reaction_key.clone()) {
                continue;
            }
            items.push(item);
            if items.len() >= limit {
                break;
            }
        }
        Ok(items)
    }

    pub async fn list_bookmarked_custom_reactions(
        &self,
    ) -> Result<Vec<BookmarkedCustomReactionView>> {
        // 旧い ID で置いた同じ内容の行は、新しい方の 1 件にまとめる（#1232 D1）。
        let mut seen = BTreeSet::new();
        Ok(self
            .services
            .projection_store
            .list_bookmarked_custom_reactions()
            .await?
            .into_iter()
            .map(bookmarked_custom_reaction_view_from_row)
            .filter(|asset| seen.insert(asset.asset_id.clone()))
            .collect())
    }

    pub async fn bookmark_custom_reaction(
        &self,
        asset: CustomReactionAssetSnapshotV1,
    ) -> Result<BookmarkedCustomReactionView> {
        if asset.owner_pubkey.as_str() == self.current_author_pubkey() {
            anyhow::bail!("bookmarking your own custom reaction is not supported");
        }
        let search_key = search_key_or_asset_id(asset.search_key.as_str(), asset.asset_id.as_str());
        // 投稿のリアクションの写しが旧い ID でも、保存は画像＋検索名の ID で置く（#1232 D1）。
        let row = BookmarkedCustomReactionRow {
            asset_id: kukuri_core::custom_reaction_id(asset.blob_hash.as_str(), &search_key),
            owner_pubkey: asset.owner_pubkey.as_str().to_string(),
            blob_hash: asset.blob_hash,
            search_key,
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
        // 一覧は旧い ID で置いた同じ内容の行も同じ ID で示すので、その行も外す。読むのは一覧と同じ新しい順の窓だけ。
        let mut keys = BTreeSet::from([asset_id.to_string()]);
        for row in self
            .services
            .projection_store
            .list_bookmarked_custom_reactions()
            .await?
        {
            if bookmarked_custom_reaction_view_from_row(row.clone()).asset_id == asset_id {
                keys.insert(row.asset_id);
            }
        }
        for key in keys {
            self.services
                .projection_store
                .remove_bookmarked_custom_reaction(&key)
                .await?;
        }
        Ok(())
    }
}
