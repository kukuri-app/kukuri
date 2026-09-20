use crate::service::*;

impl AppService {
    pub async fn resolve_community_index_posts(
        &self,
        inputs: Vec<CommunityIndexPostResolveInput>,
    ) -> Result<CommunityIndexPostResolveResponse> {
        if inputs.len() > 100 {
            anyhow::bail!("community index post resolve accepts at most 100 entries");
        }

        let mut ordered_keys = Vec::new();
        let mut seen_keys = BTreeSet::new();
        let mut results = HashMap::<String, CommunityIndexResolvedPostView>::new();
        let mut groups =
            BTreeMap::<String, (String, TimelineScope, Vec<CommunityIndexPostResolveInput>)>::new();
        for input in inputs {
            if !seen_keys.insert(input.key.clone()) {
                continue;
            }
            ordered_keys.push(input.key.clone());
            let topic = input.topic.trim().to_string();
            let object_id = input.object_id.trim();
            let author_pubkey = input.author_pubkey.trim();
            if input.key.is_empty()
                || topic.is_empty()
                || object_id.is_empty()
                || author_pubkey.is_empty()
            {
                results.insert(
                    input.key.clone(),
                    CommunityIndexResolvedPostView {
                        key: input.key,
                        post: None,
                        capabilities: CommunityIndexPostActionCapabilitiesView::default(),
                    },
                );
                continue;
            }
            let (group_key, scope) = match &input.channel_ref {
                ChannelRef::Public => (format!("{topic}\0public"), TimelineScope::Public),
                ChannelRef::PrivateChannel { channel_id } => (
                    format!("{topic}\0private\0{}", channel_id.as_str()),
                    TimelineScope::Channel {
                        channel_id: channel_id.clone(),
                    },
                ),
            };
            groups
                .entry(group_key)
                .or_insert_with(|| (topic, scope, Vec::new()))
                .2
                .push(input);
        }

        for (_, (topic, scope, entries)) in groups {
            // #1239: scope を走査しない。解決対象の object だけを、projection に無いときに key 指定で反映する。
            let mut scope_ready = self
                .ensure_scope_subscriptions(topic.as_str(), &scope)
                .await
                .is_ok();
            if scope_ready {
                for input in &entries {
                    if self
                        .ensure_object_projection(
                            topic.as_str(),
                            &scope,
                            &EnvelopeId::from(input.object_id.clone()),
                        )
                        .await
                        .is_err()
                    {
                        scope_ready = false;
                        break;
                    }
                }
            }
            if !scope_ready {
                for input in entries {
                    results.insert(
                        input.key.clone(),
                        CommunityIndexResolvedPostView {
                            key: input.key,
                            post: None,
                            capabilities: CommunityIndexPostActionCapabilitiesView::default(),
                        },
                    );
                }
                continue;
            }

            let write_allowed = match &scope {
                TimelineScope::Channel { channel_id } => self
                    .private_channel_write_state(topic.as_str(), channel_id)
                    .await
                    .is_ok(),
                TimelineScope::Public | TimelineScope::AllJoined => true,
            };
            for input in entries {
                let unresolved = || CommunityIndexResolvedPostView {
                    key: input.key.clone(),
                    post: None,
                    capabilities: CommunityIndexPostActionCapabilitiesView::default(),
                };
                let projection = match self
                    .services
                    .projection_store
                    .get_object_projection(&EnvelopeId::from(input.object_id.clone()))
                    .await
                {
                    Ok(Some(projection)) => projection,
                    Ok(None) | Err(_) => {
                        results.insert(input.key.clone(), unresolved());
                        continue;
                    }
                };
                let channel_matches = match &input.channel_ref {
                    ChannelRef::Public => projection.channel_id == PUBLIC_CHANNEL_ID,
                    ChannelRef::PrivateChannel { channel_id } => {
                        projection.channel_id == channel_id.as_str()
                    }
                };
                if projection.topic_id != topic
                    || projection.author_pubkey != input.author_pubkey
                    || !channel_matches
                    || !matches!(
                        projection.object_kind.as_str(),
                        "post" | "comment" | "repost"
                    )
                {
                    results.insert(input.key.clone(), unresolved());
                    continue;
                }
                let mut view = match self
                    .page_to_view(Page {
                        items: vec![projection],
                        next_cursor: None,
                    })
                    .await
                {
                    Ok(view) => view,
                    Err(_) => {
                        results.insert(input.key.clone(), unresolved());
                        continue;
                    }
                };
                let Some(post) = view.items.pop() else {
                    results.insert(input.key.clone(), unresolved());
                    continue;
                };
                let active = post.withdrawal.is_none();
                let post_or_comment = matches!(post.object_kind.as_str(), "post" | "comment");
                let public = post.channel_id.is_none();
                let capabilities = CommunityIndexPostActionCapabilitiesView {
                    open_thread: active && post.is_threadable,
                    reply: active && post.is_threadable && write_allowed,
                    repost: active && public && post_or_comment,
                    quote_repost: active && public && post_or_comment,
                    react: active && post_or_comment && write_allowed,
                    copy_link: true,
                    bookmark: active
                        && matches!(post.object_kind.as_str(), "post" | "comment" | "repost"),
                    withdraw: active
                        && write_allowed
                        && post.author_pubkey == self.current_author_pubkey(),
                };
                results.insert(
                    input.key.clone(),
                    CommunityIndexResolvedPostView {
                        key: input.key,
                        post: Some(post),
                        capabilities,
                    },
                );
            }
        }

        Ok(CommunityIndexPostResolveResponse {
            entries: ordered_keys
                .into_iter()
                .filter_map(|key| results.remove(key.as_str()))
                .collect(),
        })
    }
}
