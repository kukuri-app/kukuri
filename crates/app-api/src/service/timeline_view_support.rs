//! タイムラインの View 変換(projection row / profile 文書 → PostView 等)。
//! WP-H5 PR2 で timeline_runtime_support.rs から分割。購読・復旧は timeline_subscription_support.rs。

use super::*;

fn post_withdrawal_view_from_row(row: PostWithdrawalRow) -> PostWithdrawalView {
    PostWithdrawalView {
        withdrawn_at: row.withdrawn_at,
        replacement_object_id: row.replacement_object_id.map(|id| id.0),
        reason_visibility: match row.reason_visibility {
            WithdrawalReasonVisibility::Public => "public",
            WithdrawalReasonVisibility::Private => "private",
        }
        .to_string(),
        reason: row.reason.map(|reason| {
            match reason {
                PostWithdrawalReason::AuthorRequest => "author_request",
                PostWithdrawalReason::Correction => "correction",
                PostWithdrawalReason::Privacy => "privacy",
                PostWithdrawalReason::Other => "other",
            }
            .to_string()
        }),
    }
}
use crate::{ContentObservationView, ContentProvenanceView};

fn inherit_post_observation_for_attachments(
    attachments: &mut [AttachmentView],
    post_provenance: Option<&ContentProvenanceView>,
) {
    let Some(post_provenance) = post_provenance else {
        return;
    };
    for attachment in attachments {
        attachment.provenance = Some(ContentProvenanceView {
            canonical_source: "blob".to_string(),
            observed_via: post_provenance.observed_via.clone(),
        });
    }
}

impl AppService {
    pub(crate) async fn content_provenance_view(
        &self,
        subject_kind: &str,
        subject_id: &str,
        canonical_source: &str,
    ) -> Result<Option<ContentProvenanceView>> {
        let observed_via = self
            .services
            .projection_store
            .list_content_observations(subject_kind, subject_id)
            .await?
            .into_iter()
            .map(|row| ContentObservationView {
                node_base_url: row.node_base_url,
                capability: row.capability,
                observed_at: row.observed_at,
            })
            .collect::<Vec<_>>();
        if observed_via.is_empty() {
            return Ok(None);
        }
        Ok(Some(ContentProvenanceView {
            canonical_source: canonical_source.to_string(),
            observed_via,
        }))
    }

    pub(crate) async fn page_to_view(
        &self,
        page: Page<ObjectProjectionRow>,
    ) -> Result<TimelineView> {
        let local_author = self.current_author_pubkey();
        let mut author_pubkeys = BTreeSet::new();
        let mut targets_by_replica = BTreeMap::<String, Vec<EnvelopeId>>::new();
        for row in &page.items {
            author_pubkeys.insert(row.author_pubkey.clone());
            if let Some(repost_of) = row.repost_of.as_ref() {
                author_pubkeys.insert(repost_of.source_author_pubkey.as_str().to_string());
            }
            targets_by_replica
                .entry(row.source_replica_id.as_str().to_string())
                .or_default()
                .push(row.object_id.clone());
        }

        let author_pubkeys = author_pubkeys.into_iter().collect::<Vec<_>>();
        let profiles = self.services.store.get_profiles(&author_pubkeys).await?;
        let relationships = self
            .services
            .projection_store
            .list_author_relationships(local_author.as_str(), &author_pubkeys)
            .await?;
        let mut reactions_by_target = HashMap::<String, Vec<ReactionProjectionRow>>::new();
        for (replica_id, object_ids) in targets_by_replica {
            let grouped = self
                .services
                .projection_store
                .list_reaction_cache_for_targets(&ReplicaId::new(replica_id.clone()), &object_ids)
                .await?;
            for (object_id, rows) in grouped {
                reactions_by_target.insert(format!("{replica_id}:{object_id}"), rows);
            }
        }

        let mut items = Vec::with_capacity(page.items.len());
        for row in page.items {
            items.push(
                self.row_to_view_with_cache(row, &profiles, &relationships, &reactions_by_target)
                    .await?,
            );
        }
        Ok(TimelineView {
            items,
            next_cursor: page.next_cursor,
        })
    }

    pub(crate) async fn row_to_view_with_cache(
        &self,
        row: ObjectProjectionRow,
        profiles: &HashMap<String, Profile>,
        relationships: &HashMap<String, AuthorRelationshipProjectionRow>,
        reactions_by_target: &HashMap<String, Vec<ReactionProjectionRow>>,
    ) -> Result<PostView> {
        let withdrawal = self
            .services
            .projection_store
            .get_post_withdrawal(&row.object_id)
            .await?
            .map(post_withdrawal_view_from_row);
        let is_withdrawn = withdrawal.is_some();
        let profile = profiles.get(row.author_pubkey.as_str());
        let relationship = relationships.get(row.author_pubkey.as_str());
        let repost_commentary = normalize_repost_commentary(row.content.clone());
        let content_status = if is_withdrawn || row.object_kind == "repost" {
            BlobViewStatus::Available
        } else {
            blob_view_status_for_payload(self.services.blob_service.as_ref(), &row.payload_ref)
                .await?
        };
        let provenance = self
            .content_provenance_view("post", row.object_id.as_str(), "author_docs")
            .await?;
        let mut attachments = if is_withdrawn {
            Vec::new()
        } else {
            self.attachment_views_for_projection_row(&row).await?
        };
        inherit_post_observation_for_attachments(&mut attachments, provenance.as_ref());
        let repost_of = match (is_withdrawn, row.repost_of.clone()) {
            (true, _) => None,
            (false, Some(snapshot)) => Some(
                self.repost_snapshot_to_view_with_profiles(snapshot, profiles)
                    .await?,
            ),
            (false, None) => None,
        };
        let reply_preview = self
            .reply_preview_for_object_id(
                row.reply_to_object_id.as_ref(),
                Some((&row.source_replica_id, row.topic_id.as_str())),
                profiles,
            )
            .await?;
        let audience_label = self
            .audience_label_for_storage(row.topic_id.as_str(), row.channel_id.as_str())
            .await;
        let reaction_state = reaction_state_view_from_rows(
            &row.source_replica_id,
            &row.object_id,
            reactions_by_target
                .get(reaction_cache_key(&row.source_replica_id, &row.object_id).as_str())
                .cloned()
                .unwrap_or_default(),
            self.current_author_pubkey().as_str(),
        );
        let AuthorViewParts {
            author_name,
            author_display_name,
            author_picture_asset,
            following,
            followed_by,
            mutual,
            friend_of_friend,
        } = AuthorViewParts::new(profile, relationship);

        Ok(PostView {
            object_id: row.object_id.0.clone(),
            envelope_id: row.source_envelope_id.0.clone(),
            author_pubkey: row.author_pubkey.clone(),
            author_name,
            author_display_name,
            author_picture_asset,
            following,
            followed_by,
            mutual,
            friend_of_friend,
            provenance,
            withdrawal,
            content: if is_withdrawn {
                String::new()
            } else {
                row.content.unwrap_or_else(|| "[blob pending]".to_string())
            },
            content_status,
            attachments,
            content_labels: row.content_labels.clone(),
            created_at: row.created_at,
            reply_to: row.reply_to_object_id.clone().map(|id| id.0),
            reply_preview,
            root_id: row.root_object_id.clone().map(|id| id.0),
            object_kind: row.object_kind.clone(),
            published_topic_id: Some(row.topic_id.clone()),
            origin_topic_id: Some(row.topic_id.clone()),
            repost_of,
            repost_commentary: if is_withdrawn {
                None
            } else {
                repost_commentary.clone()
            },
            is_threadable: !is_withdrawn
                && (row.object_kind != "repost" || repost_commentary.is_some()),
            channel_id: channel_id_for_view(row.channel_id.as_str()),
            audience_label,
            reaction_summary: reaction_state.reaction_summary,
            my_reactions: reaction_state.my_reactions,
        })
    }

    pub(crate) async fn reply_preview_for_object_id(
        &self,
        object_id: Option<&EnvelopeId>,
        source: Option<(&ReplicaId, &str)>,
        profiles: &HashMap<String, Profile>,
    ) -> Result<Option<ReplyPreviewView>> {
        let Some(object_id) = object_id else {
            return Ok(None);
        };
        let is_withdrawn = self
            .services
            .projection_store
            .get_post_withdrawal(object_id)
            .await?
            .is_some();
        let Some(row) = self.reply_target_row(object_id, source).await? else {
            return Ok(None);
        };
        let provenance = self
            .content_provenance_view("post", row.object_id.as_str(), "author_docs")
            .await?;
        let mut attachments = if is_withdrawn {
            Vec::new()
        } else {
            self.attachment_views_for_projection_row(&row).await?
        };
        inherit_post_observation_for_attachments(&mut attachments, provenance.as_ref());
        let profile = match profiles.get(row.author_pubkey.as_str()) {
            Some(profile) => Some(profile.clone()),
            None => {
                self.services
                    .store
                    .get_profile(row.author_pubkey.as_str())
                    .await?
            }
        };
        Ok(Some(ReplyPreviewView {
            object_id: row.object_id.0.clone(),
            topic: row.topic_id.clone(),
            author: ReplyPreviewAuthorView {
                pubkey: row.author_pubkey.clone(),
                name: profile.as_ref().and_then(|value| value.name.clone()),
                display_name: profile
                    .as_ref()
                    .and_then(|value| value.display_name.clone()),
                picture_asset: profile
                    .as_ref()
                    .and_then(|value| profile_asset_view_from_ref(value.picture_asset.as_ref())),
            },
            content: if is_withdrawn {
                String::new()
            } else {
                row.content.unwrap_or_else(|| "[blob pending]".to_string())
            },
            attachments,
            content_labels: row.content_labels.clone(),
            root_id: row.root_object_id.map(|id| id.0),
            reply_to: row.reply_to_object_id.map(|id| id.0),
        }))
    }

    pub(crate) async fn attachment_views_for_projection_row(
        &self,
        row: &ObjectProjectionRow,
    ) -> Result<Vec<AttachmentView>> {
        if row.object_kind == "repost" {
            return Ok(Vec::new());
        }
        // 添付は行から読む。添付の列が無い旧い行(`projection_version < 2`)を docs の `state` で補う fallback は、
        // 未検証の値を表示するので削除した(#1248。旧い行は migration が消す)。
        attachment_views_from_refs(self.services.blob_service.as_ref(), &row.attachments).await
    }

    pub(crate) async fn bookmarked_post_view_from_row(
        &self,
        row: BookmarkedPostRow,
    ) -> Result<BookmarkedPostView> {
        // #858: BookmarkedPostRow はラベル列を持たないため、object projection から
        // 補完する(未投影なら空。blob 取得ゲートは hash 単位で別途 fail-closed)。
        let content_labels = self
            .services
            .projection_store
            .get_object_projection(&row.source_object_id)
            .await?
            .map(|projection| projection.content_labels)
            .unwrap_or_default();
        let withdrawal = self
            .services
            .projection_store
            .get_post_withdrawal(&row.source_object_id)
            .await?
            .map(post_withdrawal_view_from_row);
        let is_withdrawn = withdrawal.is_some();
        let profile = self
            .services
            .store
            .get_profile(row.author_pubkey.as_str())
            .await?;
        let relationship = self
            .services
            .projection_store
            .get_author_relationship(
                self.current_author_pubkey().as_str(),
                row.author_pubkey.as_str(),
            )
            .await?;
        let content_status = if row.object_kind == "repost" {
            BlobViewStatus::Available
        } else {
            blob_view_status_for_payload(self.services.blob_service.as_ref(), &row.payload_ref)
                .await?
        };
        let mut attachments = if row.object_kind == "repost" {
            Vec::new()
        } else {
            attachment_views_from_refs(self.services.blob_service.as_ref(), &row.attachments)
                .await?
        };
        if is_withdrawn {
            attachments.clear();
        }
        let repost_commentary = normalize_repost_commentary(row.content.clone());
        let repost_of = match (is_withdrawn, row.repost_of.clone()) {
            (true, _) => None,
            (false, Some(snapshot)) => Some(self.repost_snapshot_to_view(snapshot).await?),
            (false, None) => None,
        };
        let audience_label = self
            .audience_label_for_storage(row.topic_id.as_str(), row.channel_id.as_str())
            .await;
        let reaction_state = self
            .reaction_state_for_target(&row.source_replica_id, &row.source_object_id)
            .await?;
        let empty_profiles = HashMap::new();
        let reply_preview = self
            .reply_preview_for_object_id(
                row.reply_to_object_id.as_ref(),
                Some((&row.source_replica_id, row.topic_id.as_str())),
                &empty_profiles,
            )
            .await?;
        let provenance = self
            .content_provenance_view("post", row.source_object_id.as_str(), "author_docs")
            .await?;
        inherit_post_observation_for_attachments(&mut attachments, provenance.as_ref());

        let AuthorViewParts {
            author_name,
            author_display_name,
            author_picture_asset,
            following,
            followed_by,
            mutual,
            friend_of_friend,
        } = AuthorViewParts::new(profile.as_ref(), relationship.as_ref());
        Ok(BookmarkedPostView {
            bookmarked_at: row.bookmarked_at,
            post: PostView {
                object_id: row.source_object_id.as_str().to_string(),
                envelope_id: row.source_envelope_id.as_str().to_string(),
                author_pubkey: row.author_pubkey.clone(),
                author_name,
                author_display_name,
                author_picture_asset,
                following,
                followed_by,
                mutual,
                friend_of_friend,
                provenance,
                withdrawal,
                object_kind: row.object_kind.clone(),
                content: if is_withdrawn {
                    String::new()
                } else {
                    row.content.unwrap_or_else(|| "[blob pending]".to_string())
                },
                content_status,
                attachments,
                content_labels,
                created_at: row.created_at,
                reply_to: row.reply_to_object_id.map(|id| id.0),
                reply_preview,
                root_id: row.root_object_id.map(|id| id.0),
                published_topic_id: Some(row.topic_id.clone()),
                origin_topic_id: Some(row.topic_id.clone()),
                repost_of,
                repost_commentary: repost_commentary.clone(),
                is_threadable: row.object_kind != "repost" || repost_commentary.is_some(),
                channel_id: channel_id_for_view(row.channel_id.as_str()),
                audience_label,
                reaction_summary: reaction_state.reaction_summary,
                my_reactions: reaction_state.my_reactions,
            },
        })
    }

    pub(crate) async fn profile_post_to_view(&self, profile_post: ProfilePost) -> Result<PostView> {
        // #1239: view の生成中に docs を読まない。取り下げは projection の表だけで判定し、
        // 購読していない topic の投稿の取り下げは、背景の上限つきの確認で追いつく。
        self.schedule_withdrawal_check(
            profile_post.published_topic_id.as_str(),
            &profile_post.object_id,
        );
        // 返信先の preview も同じ topic の投稿で、取り下げの event が届かないことがある。
        // 返信先の本文と添付を出し続けないよう、返信先の取り下げも確認する(ADR 0032 §2)。
        if let Some(reply_to_object_id) = profile_post.reply_to_object_id.as_ref() {
            self.schedule_withdrawal_check(
                profile_post.published_topic_id.as_str(),
                reply_to_object_id,
            );
        }
        let withdrawal = self
            .services
            .projection_store
            .get_post_withdrawal(&profile_post.object_id)
            .await?
            .map(post_withdrawal_view_from_row);
        let is_withdrawn = withdrawal.is_some();
        let profile = self
            .services
            .store
            .get_profile(profile_post.author_pubkey.as_str())
            .await?;
        let relationship = self
            .services
            .projection_store
            .get_author_relationship(
                self.current_author_pubkey().as_str(),
                profile_post.author_pubkey.as_str(),
            )
            .await?;
        let empty_profiles = HashMap::new();
        let source_replica_id = topic_replica_id(profile_post.published_topic_id.as_str());
        let reply_preview = self
            .reply_preview_for_object_id(
                profile_post.reply_to_object_id.as_ref(),
                Some((&source_replica_id, profile_post.published_topic_id.as_str())),
                &empty_profiles,
            )
            .await?;
        let provenance = self
            .content_provenance_view("post", profile_post.object_id.as_str(), "author_docs")
            .await?;

        let AuthorViewParts {
            author_name,
            author_display_name,
            author_picture_asset,
            following,
            followed_by,
            mutual,
            friend_of_friend,
        } = AuthorViewParts::new(profile.as_ref(), relationship.as_ref());
        let mut attachments = attachment_views_from_refs(
            self.services.blob_service.as_ref(),
            &profile_post.attachments,
        )
        .await?;
        if is_withdrawn {
            attachments.clear();
        }
        // #858: profile timeline は object projection を経由しないため、成人向け
        // ラベル付き添付の hash をここで取得ゲート用に記録する。
        if kukuri_core::has_adult_content_label(&profile_post.content_labels) {
            let hashes = profile_post
                .attachments
                .iter()
                .map(|attachment| attachment.hash.clone())
                .collect::<Vec<_>>();
            self.services
                .projection_store
                .mark_adult_media_hashes(&hashes)
                .await?;
        }
        inherit_post_observation_for_attachments(&mut attachments, provenance.as_ref());
        Ok(PostView {
            object_id: profile_post.object_id.0.clone(),
            envelope_id: profile_post.object_id.0.clone(),
            author_pubkey: profile_post.author_pubkey.as_str().to_string(),
            author_name,
            author_display_name,
            author_picture_asset,
            following,
            followed_by,
            mutual,
            friend_of_friend,
            provenance,
            withdrawal,
            object_kind: profile_post.object_kind,
            content: if is_withdrawn {
                String::new()
            } else {
                profile_post.content
            },
            content_status: BlobViewStatus::Available,
            attachments,
            content_labels: profile_post.content_labels.clone(),
            created_at: profile_post.created_at,
            reply_to: profile_post.reply_to_object_id.map(|id| id.0),
            reply_preview,
            root_id: profile_post.root_id.map(|id| id.0),
            published_topic_id: Some(profile_post.published_topic_id.as_str().to_string()),
            origin_topic_id: Some(profile_post.published_topic_id.as_str().to_string()),
            repost_of: None,
            repost_commentary: None,
            is_threadable: true,
            channel_id: None,
            audience_label: "Public".into(),
            reaction_summary: Vec::new(),
            my_reactions: Vec::new(),
        })
    }

    pub(crate) async fn profile_repost_to_view(
        &self,
        profile_repost: ProfileRepost,
    ) -> Result<PostView> {
        // #1239: view の生成中に docs を読まない。取り下げは projection の表だけで判定し、
        // 購読していない topic の投稿の取り下げは、背景の上限つきの確認で追いつく。
        self.schedule_withdrawal_check(
            profile_repost.published_topic_id.as_str(),
            &profile_repost.object_id,
        );
        let withdrawal = self
            .services
            .projection_store
            .get_post_withdrawal(&profile_repost.object_id)
            .await?
            .map(post_withdrawal_view_from_row);
        let is_withdrawn = withdrawal.is_some();
        let profile = self
            .services
            .store
            .get_profile(profile_repost.author_pubkey.as_str())
            .await?;
        let relationship = self
            .services
            .projection_store
            .get_author_relationship(
                self.current_author_pubkey().as_str(),
                profile_repost.author_pubkey.as_str(),
            )
            .await?;

        let AuthorViewParts {
            author_name,
            author_display_name,
            author_picture_asset,
            following,
            followed_by,
            mutual,
            friend_of_friend,
        } = AuthorViewParts::new(profile.as_ref(), relationship.as_ref());
        let provenance = self
            .content_provenance_view("post", profile_repost.object_id.as_str(), "author_docs")
            .await?;
        Ok(PostView {
            object_id: profile_repost.object_id.0.clone(),
            envelope_id: profile_repost.envelope_id.0.clone(),
            author_pubkey: profile_repost.author_pubkey.as_str().to_string(),
            author_name,
            author_display_name,
            author_picture_asset,
            following,
            followed_by,
            mutual,
            friend_of_friend,
            provenance,
            withdrawal,
            object_kind: "repost".into(),
            content: if is_withdrawn {
                String::new()
            } else {
                profile_repost.commentary.clone().unwrap_or_default()
            },
            content_status: BlobViewStatus::Available,
            attachments: Vec::new(),
            content_labels: profile_repost.repost_of.content_labels.clone(),
            created_at: profile_repost.created_at,
            reply_to: None,
            reply_preview: None,
            root_id: None,
            published_topic_id: Some(profile_repost.published_topic_id.as_str().to_string()),
            origin_topic_id: Some(profile_repost.published_topic_id.as_str().to_string()),
            repost_of: if is_withdrawn {
                None
            } else {
                Some(
                    self.repost_snapshot_to_view(profile_repost.repost_of)
                        .await?,
                )
            },
            repost_commentary: if is_withdrawn {
                None
            } else {
                profile_repost.commentary.clone()
            },
            is_threadable: !is_withdrawn && profile_repost.commentary.is_some(),
            channel_id: None,
            audience_label: "Public".into(),
            reaction_summary: Vec::new(),
            my_reactions: Vec::new(),
        })
    }

    pub(crate) async fn repost_snapshot_to_view(
        &self,
        snapshot: RepostSourceSnapshotV1,
    ) -> Result<RepostSourceView> {
        let profiles = self
            .services
            .store
            .get_profiles(&[snapshot.source_author_pubkey.as_str().to_string()])
            .await?;
        self.repost_snapshot_to_view_with_profiles(snapshot, &profiles)
            .await
    }

    pub(crate) async fn repost_snapshot_to_view_with_profiles(
        &self,
        snapshot: RepostSourceSnapshotV1,
        profiles: &HashMap<String, Profile>,
    ) -> Result<RepostSourceView> {
        // #1239: view の生成中に docs を読まない。取り下げは projection の表だけで判定し、
        // 購読していない topic の投稿の取り下げは、背景の上限つきの確認で追いつく。
        self.schedule_withdrawal_check(
            snapshot.source_topic_id.as_str(),
            &snapshot.source_object_id,
        );
        let is_withdrawn = self
            .services
            .projection_store
            .get_post_withdrawal(&snapshot.source_object_id)
            .await?
            .is_some();
        // #858: 引用 snapshot も取得ゲート用の hash 記録の対象にする。
        if kukuri_core::has_adult_content_label(&snapshot.content_labels) {
            let hashes = snapshot
                .attachments
                .iter()
                .map(|attachment| attachment.hash.clone())
                .collect::<Vec<_>>();
            self.services
                .projection_store
                .mark_adult_media_hashes(&hashes)
                .await?;
        }
        let source_profile = profiles.get(snapshot.source_author_pubkey.as_str());
        let author = AuthorViewParts::new(source_profile, None);
        Ok(RepostSourceView {
            source_object_id: snapshot.source_object_id.as_str().to_string(),
            source_topic_id: snapshot.source_topic_id.as_str().to_string(),
            source_author_pubkey: snapshot.source_author_pubkey.as_str().to_string(),
            source_author_name: author.author_name,
            source_author_display_name: author.author_display_name,
            source_author_picture_asset: author.author_picture_asset,
            source_object_kind: snapshot.source_object_kind,
            content: if is_withdrawn {
                String::new()
            } else {
                snapshot.content
            },
            attachments: if is_withdrawn {
                Vec::new()
            } else {
                attachment_views_from_refs(
                    self.services.blob_service.as_ref(),
                    &snapshot.attachments,
                )
                .await?
            },
            content_labels: snapshot.content_labels.clone(),
            reply_to: snapshot.reply_to_object_id.map(|id| id.0),
            root_id: snapshot.root_id.map(|id| id.0),
        })
    }
}

/// Upper bounds (in Unicode scalar values) for user-authored text that gets signed
/// into envelopes and replicated to other peers. They bound the gossip/docs payload
/// size and keep a single client from flooding the topic with oversized objects.
pub(crate) const MAX_POST_CONTENT_CHARS: usize = 10_000;
pub(crate) const MAX_REPOST_COMMENTARY_CHARS: usize = 2_000;
pub(crate) const MAX_PROFILE_NAME_CHARS: usize = 64;
pub(crate) const MAX_PROFILE_DISPLAY_NAME_CHARS: usize = 128;
pub(crate) const MAX_PROFILE_ABOUT_CHARS: usize = 2_000;

/// PostView の作者まわり共通フィールド(プロフィール由来 4 + フォロー関係 4)の組み立て部品。
/// 5 箇所で重複していた導出ロジックの単一実装(WP-H5 PR2)。フィールド名は PostView と
/// 同名にしてあり、呼び出し側は分配束縛 + フィールド省略記法でそのまま流し込める。
pub(crate) struct AuthorViewParts {
    pub(crate) author_name: Option<String>,
    pub(crate) author_display_name: Option<String>,
    pub(crate) author_picture_asset: Option<ProfileAssetView>,
    pub(crate) following: bool,
    pub(crate) followed_by: bool,
    pub(crate) mutual: bool,
    pub(crate) friend_of_friend: bool,
}

impl AuthorViewParts {
    pub(crate) fn new(
        profile: Option<&Profile>,
        relationship: Option<&AuthorRelationshipProjectionRow>,
    ) -> Self {
        Self {
            author_name: profile.and_then(|value| value.name.clone()),
            author_display_name: profile.and_then(|value| value.display_name.clone()),
            author_picture_asset: profile
                .and_then(|value| profile_asset_view_from_ref(value.picture_asset.as_ref())),
            following: relationship.is_some_and(|value| value.following),
            followed_by: relationship.is_some_and(|value| value.followed_by),
            mutual: relationship.is_some_and(|value| value.mutual),
            friend_of_friend: relationship.is_some_and(|value| value.friend_of_friend),
        }
    }
}

pub(crate) fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

/// Reject user text whose character count exceeds `max_chars`. Counting `chars()`
/// keeps the limit user-facing (it matches what a person types) while still bounding
/// the byte payload, since a scalar value is at most 4 bytes.
pub(crate) fn ensure_text_within_limit(field: &str, value: &str, max_chars: usize) -> Result<()> {
    let count = value.chars().count();
    if count > max_chars {
        anyhow::bail!("{field} must be at most {max_chars} characters (got {count})");
    }
    Ok(())
}

pub(crate) fn ensure_optional_text_within_limit(
    field: &str,
    value: Option<&str>,
    max_chars: usize,
) -> Result<()> {
    match value {
        Some(value) => ensure_text_within_limit(field, value, max_chars),
        None => Ok(()),
    }
}

pub(crate) fn profile_asset_view_from_ref(
    asset: Option<&kukuri_core::AssetRef>,
) -> Option<ProfileAssetView> {
    asset.map(|asset| ProfileAssetView {
        hash: asset.hash.as_str().to_string(),
        mime: asset.mime.clone(),
        bytes: asset.bytes,
        role: "profile_avatar".into(),
    })
}

pub(crate) fn normalize_repost_commentary(value: Option<String>) -> Option<String> {
    normalize_optional_text(value)
}

pub(crate) fn content_from_payload_ref(payload_ref: &PayloadRef) -> Option<String> {
    match payload_ref {
        PayloadRef::InlineText { text } => Some(text.clone()),
        PayloadRef::BlobText { .. } => None,
    }
}
