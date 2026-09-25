use crate::*;

#[cfg(test)]
fn is_retryable_friend_only_grant_import_error(message: &str) -> bool {
    message.contains("mutual relationship")
        || message.contains("friend-only grant epoch does not match the current policy")
        || message.contains("timed out waiting for friend-only channel replica sync")
}

#[cfg(test)]
pub(crate) async fn wait_for_friend_only_grant_import(
    runtime: &DesktopRuntime,
    token: String,
    step_timeout: Duration,
) -> Result<kukuri_core::FriendOnlyGrantPreview> {
    let preview = kukuri_core::parse_friend_only_grant_token(token.as_str())?;
    match timeout(step_timeout, async {
        loop {
            match runtime
                .import_friend_only_grant(kukuri_desktop_runtime::ImportFriendOnlyGrantRequest {
                    token: token.clone(),
                })
                .await
            {
                Ok(preview) => return Ok::<_, anyhow::Error>(preview),
                Err(error)
                    if is_retryable_friend_only_grant_import_error(error.to_string().as_str()) =>
                {
                    sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let social_view = runtime
                .get_author_social_view(kukuri_desktop_runtime::AuthorRequest {
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
            let snapshot = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, preview.topic_id.as_str()))
                .unwrap_or_else(|| "failed to read sync status".to_string());
            anyhow::bail!(
                "friend-only grant import assertion timeout; owner_pubkey={}, {social_view}, {snapshot}",
                preview.owner_pubkey.as_str()
            );
        }
    }
}

#[cfg(test)]
pub(crate) async fn wait_for_friend_plus_share_import(
    runtime: &DesktopRuntime,
    token: String,
    step_timeout: Duration,
) -> Result<kukuri_core::FriendPlusSharePreview> {
    let preview = kukuri_core::parse_friend_plus_share_token(token.as_str())?;
    match timeout(step_timeout, async {
        loop {
            match runtime
                .import_friend_plus_share(kukuri_desktop_runtime::ImportFriendPlusShareRequest {
                    token: token.clone(),
                })
                .await
            {
                Ok(preview) => return Ok::<_, anyhow::Error>(preview),
                Err(error) if error.to_string().contains("mutual relationship") => {
                    sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let social_view = runtime
                .get_author_social_view(kukuri_desktop_runtime::AuthorRequest {
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
            let snapshot = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, preview.topic_id.as_str()))
                .unwrap_or_else(|| "failed to read sync status".to_string());
            anyhow::bail!(
                "friend-plus share import assertion timeout; sponsor_pubkey={}, {social_view}, {snapshot}",
                preview.sponsor_pubkey.as_str()
            );
        }
    }
}

pub(crate) async fn wait_for_joined_private_channel(
    runtime: &DesktopRuntime,
    topic: &str,
    channel_id: &str,
    step_timeout: Duration,
) -> Result<()> {
    timeout(step_timeout, async {
        loop {
            let joined = runtime
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.to_string(),
                })
                .await?;
            if joined.iter().any(|entry| entry.channel_id == channel_id) {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("joined private-channel assertion timeout")?
}

pub(crate) fn private_replication_retry_schedule(step_timeout: Duration) -> (usize, Duration) {
    let attempts = if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
        3
    } else {
        1
    };
    let per_attempt_timeout = if attempts > 1 {
        Duration::from_millis(
            (step_timeout.as_millis() / attempts as u128)
                .max(1)
                .try_into()
                .expect("private replication timeout fits in u64"),
        )
    } else {
        step_timeout
    };
    (attempts, per_attempt_timeout)
}

pub(crate) async fn refresh_private_channel_pair(
    runtime_a: &DesktopRuntime,
    runtime_b: &DesktopRuntime,
    ticket_a: &str,
    ticket_b: &str,
    topic: &str,
    private_scope: &TimelineScope,
) -> Result<()> {
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.to_string(),
        })
        .await?;
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.to_string(),
        })
        .await?;
    let _ = runtime_a
        .list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await;
    let _ = runtime_b
        .list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await;
    Ok(())
}
