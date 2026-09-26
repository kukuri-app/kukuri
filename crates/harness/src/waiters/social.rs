use super::*;
use crate::*;

pub(crate) async fn wait_for_author_social_view(
    runtime: &DesktopRuntime,
    author_pubkey: &str,
    step_timeout: Duration,
) -> Result<()> {
    // 相手の profile を開く(#1221 R2-C。author は profile/DM を表示している間だけ購読し、follow の edge もその購読で届く)。
    runtime
        .set_scope_display(kukuri_desktop_runtime::ScopeDisplayRequest {
            observer: format!("harness-profile:{author_pubkey}"),
            target: kukuri_desktop_runtime::ScopeDisplayTarget::Author {
                pubkey: author_pubkey.to_string(),
            },
            visible: true,
        })
        .await?;
    match timeout(step_timeout, async {
        loop {
            if runtime
                .get_author_social_view(kukuri_desktop_runtime::AuthorRequest {
                    pubkey: author_pubkey.to_string(),
                })
                .await
                .is_ok()
            {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => anyhow::bail!("author social view warmup timeout for {author_pubkey}"),
    }
}

pub(crate) async fn wait_for_mutual_author_view_result(
    runtime: &DesktopRuntime,
    author_pubkey: &str,
    topic: &str,
    step_timeout: Duration,
) -> Result<()> {
    match timeout(step_timeout, async {
        loop {
            let view = runtime
                .get_author_social_view(kukuri_desktop_runtime::AuthorRequest {
                    pubkey: author_pubkey.to_string(),
                })
                .await
                .context("author social view")?;
            if view.mutual {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let social_view = runtime
                .get_author_social_view(kukuri_desktop_runtime::AuthorRequest {
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
            let snapshot = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read sync status".to_string());
            anyhow::bail!(
                "mutual relationship timeout for {author_pubkey}; {social_view}, {snapshot}"
            );
        }
    }
}
