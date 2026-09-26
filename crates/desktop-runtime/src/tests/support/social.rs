use super::super::*;

pub(crate) async fn wait_for_mutual_author_view(
    runtime: &DesktopRuntime,
    author_pubkey: &str,
    topic: &str,
) {
    match timeout(social_graph_propagation_timeout(), async {
        loop {
            let view = runtime
                .get_author_social_view(AuthorRequest {
                    pubkey: author_pubkey.to_string(),
                })
                .await
                .expect("author social view");
            if view.mutual {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            let social_view = runtime
                    .get_author_social_view(AuthorRequest {
                        pubkey: author_pubkey.to_string(),
                    })
                    .await
                    .ok()
                    .map(|value| {
                        format!(
                            "following={}, followed_by={}, mutual={}, friend_of_friend={}, fof_via={:?}",
                            value.following,
                            value.followed_by,
                            value.mutual,
                            value.friend_of_friend,
                            value.friend_of_friend_via_pubkeys
                        )
                    })
                    .unwrap_or_else(|| "social_view=unavailable".to_string());
            let status = runtime.get_sync_status().await.expect("sync status");
            panic!(
                "mutual author view timeout for {author_pubkey}; {social_view}; {}",
                format_sync_snapshot(&status, topic)
            );
        }
    }
}

pub(crate) async fn warm_author_social_view(
    runtime: &DesktopRuntime,
    author_pubkey: &str,
    timeout_label: &str,
) {
    match timeout(social_graph_propagation_timeout(), async {
        loop {
            if runtime
                .get_author_social_view(AuthorRequest {
                    pubkey: author_pubkey.to_string(),
                })
                .await
                .is_ok()
            {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            let status = runtime.get_sync_status().await.expect("sync status");
            panic!("{timeout_label}: {}", format_sync_snapshot(&status, ""));
        }
    }
}

// リトライ判定は app-api の typed 契約(WP-B11 で pub 化)を使う。DesktopRuntime は
// app_service の anyhow チェーンを素通しするため downcast がそのまま届く。
// 旧実装のエラー文言 contains 判定(文言 = 暗黙契約)はここで撤去した。
use kukuri_app_api::{
    is_retryable_friend_only_grant_import_error, is_retryable_friend_plus_share_import_error,
};

pub(crate) async fn wait_for_friend_only_grant_import(
    runtime: &DesktopRuntime,
    token: &str,
    step_timeout: Duration,
    timeout_label: &str,
) -> kukuri_core::FriendOnlyGrantPreview {
    let preview = kukuri_core::parse_friend_only_grant_token(token).expect("parse grant token");
    let last_retryable_error = Arc::new(Mutex::new(None::<String>));
    let retry_error_slot = Arc::clone(&last_retryable_error);
    match timeout(step_timeout, async {
        loop {
            match runtime
                .import_friend_only_grant(ImportFriendOnlyGrantRequest {
                    token: token.to_string(),
                })
                .await
            {
                Ok(preview) => return preview,
                Err(error) if is_retryable_friend_only_grant_import_error(&error) => {
                    *retry_error_slot.lock().await = Some(error.to_string());
                    sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("{timeout_label}: {error:#}"),
            }
        }
    })
    .await
    {
        Ok(preview) => preview,
        Err(_) => {
            let last_error = last_retryable_error
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| "none".to_string());
            let social_view = runtime
                .get_author_social_view(AuthorRequest {
                    pubkey: preview.owner_pubkey.as_str().to_string(),
                })
                .await
                .ok()
                .map(|value| {
                    format!(
                        "following={}, followed_by={}, mutual={}, friend_of_friend={}, fof_via={:?}",
                        value.following,
                        value.followed_by,
                        value.mutual,
                        value.friend_of_friend,
                        value.friend_of_friend_via_pubkeys
                    )
                })
                .unwrap_or_else(|| "social_view=unavailable".to_string());
            let status = runtime.get_sync_status().await.expect("sync status");
            panic!(
                "{timeout_label}: owner_pubkey={}, last_retryable_error={}, {social_view}, {}",
                preview.owner_pubkey.as_str(),
                last_error,
                format_sync_snapshot(&status, preview.topic_id.as_str())
            );
        }
    }
}

pub(crate) async fn wait_for_friend_plus_share_import(
    runtime: &DesktopRuntime,
    token: &str,
    step_timeout: Duration,
    timeout_label: &str,
) -> kukuri_core::FriendPlusSharePreview {
    let preview = kukuri_core::parse_friend_plus_share_token(token).expect("parse share token");
    let last_retryable_error = Arc::new(Mutex::new(None::<String>));
    let retry_error_slot = Arc::clone(&last_retryable_error);
    match timeout(step_timeout, async {
        loop {
            match runtime
                .import_friend_plus_share(ImportFriendPlusShareRequest {
                    token: token.to_string(),
                })
                .await
            {
                Ok(preview) => return preview,
                Err(error) if is_retryable_friend_plus_share_import_error(&error) => {
                    *retry_error_slot.lock().await = Some(error.to_string());
                    sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("{timeout_label}: {error:#}"),
            }
        }
    })
    .await
    {
        Ok(preview) => preview,
        Err(_) => {
            let last_error = last_retryable_error
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| "none".to_string());
            let social_view = runtime
                .get_author_social_view(AuthorRequest {
                    pubkey: preview.sponsor_pubkey.as_str().to_string(),
                })
                .await
                .ok()
                .map(|value| {
                    format!(
                        "following={}, followed_by={}, mutual={}, friend_of_friend={}, fof_via={:?}",
                        value.following,
                        value.followed_by,
                        value.mutual,
                        value.friend_of_friend,
                        value.friend_of_friend_via_pubkeys
                    )
                })
                .unwrap_or_else(|| "social_view=unavailable".to_string());
            let status = runtime.get_sync_status().await.expect("sync status");
            panic!(
                "{timeout_label}: sponsor_pubkey={}, last_retryable_error={}, {social_view}, {}",
                preview.sponsor_pubkey.as_str(),
                last_error,
                format_sync_snapshot(&status, preview.topic_id.as_str())
            );
        }
    }
}
pub(crate) async fn wait_for_joined_private_channel_epoch_result(
    runtime: &DesktopRuntime,
    topic: &str,
    channel_id: &str,
    expected_epoch_id: &str,
    min_owner_participant_count: Option<usize>,
    step_timeout: Duration,
) -> Result<JoinedPrivateChannelView> {
    match timeout(step_timeout, async {
        let private_scope = TimelineScope::Channel {
            channel_id: kukuri_core::ChannelId::new(channel_id.to_string()),
        };
        loop {
            let _ = runtime
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: TimelineScope::Public,
                    cursor: None,
                    limit: Some(20),
                })
                .await;
            let joined = runtime
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.into(),
                })
                .await
                .context("joined channels query failed")?;
            let Some(entry) = joined.iter().find(|item| item.channel_id == channel_id) else {
                sleep(Duration::from_millis(50)).await;
                continue;
            };
            let _ = runtime
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await;
            // 人数は owner の端末だけが返す(#1221 R5-H)。owner 以外を待つときは人数を条件にしない。
            if entry.current_epoch_id == expected_epoch_id
                && min_owner_participant_count
                    .is_none_or(|min| entry.participant_count.is_some_and(|count| count >= min))
            {
                return Ok::<JoinedPrivateChannelView, anyhow::Error>(entry.clone());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let status = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|value| format_sync_snapshot(&value, topic))
                .unwrap_or_else(|| "failed to read sync status".to_string());
            let joined = runtime
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.into(),
                })
                .await
                .unwrap_or_default();
            bail!("joined private channel epoch timeout; {status}; joined={joined:?}");
        }
    }
}

pub(crate) async fn joined_private_channel_epoch_result(
    runtime: &DesktopRuntime,
    topic: &str,
    channel_id: &str,
) -> Result<Option<JoinedPrivateChannelView>> {
    let joined = runtime
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .context("joined channels query failed")?;
    Ok(joined
        .into_iter()
        .find(|entry| entry.channel_id == channel_id))
}
pub(crate) async fn wait_for_joined_private_channel_epoch(
    runtime: &DesktopRuntime,
    topic: &str,
    channel_id: &str,
    expected_epoch_id: &str,
    min_owner_participant_count: Option<usize>,
    timeout_label: &str,
) -> JoinedPrivateChannelView {
    match wait_for_joined_private_channel_epoch_result(
        runtime,
        topic,
        channel_id,
        expected_epoch_id,
        min_owner_participant_count,
        runtime_replication_timeout(),
    )
    .await
    {
        Ok(entry) => entry,
        Err(error) => panic!("{timeout_label}: {error:#}"),
    }
}
