use super::*;
use kukuri_core::ChannelAudienceKind;
use kukuri_desktop_runtime::{
    AuthorRequest, ExportFriendOnlyGrantRequest, ExportFriendPlusShareRequest,
    FreezePrivateChannelRequest, ImportFriendOnlyGrantRequest, ImportFriendPlusShareRequest,
    RotatePrivateChannelRequest,
};
use std::sync::{Once, OnceLock};
use tokio::sync::{Mutex, MutexGuard};

fn disable_keyring_for_tests() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        std::env::set_var("KUKURI_DISABLE_KEYRING", "1");
    });
}

async fn acquire_scenario_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

fn social_graph_propagation_timeout() -> Duration {
    kukuri_test_support::constrained_timeout(Duration::from_secs(30), Duration::from_secs(300))
}

fn sync_status_with_topic(
    topic: &str,
    connected_peers: &[&str],
    docs_assist_peer_ids: &[&str],
) -> SyncStatus {
    let connected = !connected_peers.is_empty();
    let delivery_state = if connected {
        kukuri_app_api::DeliveryState::Live
    } else if !docs_assist_peer_ids.is_empty() {
        kukuri_app_api::DeliveryState::DurableRecovering
    } else {
        kukuri_app_api::DeliveryState::Offline
    };
    SyncStatus {
        connected,
        delivery_state,
        last_sync_ts: None,
        peer_count: connected_peers.len(),
        pending_events: 0,
        status_detail: "test".to_string(),
        last_error: None,
        configured_peer_count: 0,
        subscribed_topics: vec![topic.to_string()],
        active_path: Default::default(),
        fallback_peer_count: 0,
        topic_diagnostics: vec![kukuri_app_api::TopicSyncStatus {
            topic: topic.to_string(),
            joined: connected,
            delivery_state,
            peer_count: connected_peers.len(),
            configured_peer_count: 0,
            missing_peer_count: 0,
            active_path: Default::default(),
            rendezvous_peer_count: 0,
            fallback_peer_count: 0,
            last_received_at: None,
            last_docs_activity_at: None,
            status_detail: "test".to_string(),
            last_error: None,
        }],
        local_author_pubkey: "author".to_string(),
        discovery: kukuri_app_api::DiscoveryStatus {
            docs_assist_peer_count: docs_assist_peer_ids.len(),
            ..Default::default()
        },
        gossip_disabled_topics: Vec::new(),
        gossip_disabled_channels: Vec::new(),
    }
}

async fn warm_author_social_view(runtime: &DesktopRuntime, author_pubkey: &str) {
    timeout(social_graph_propagation_timeout(), async {
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
    .expect("author social view warmup timeout");
}

async fn wait_for_mutual_author_view(runtime: &DesktopRuntime, author_pubkey: &str, topic: &str) {
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
            let snapshot = runtime
                .get_sync_status()
                .await
                .ok()
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|| "failed to read sync status".to_string());
            panic!("mutual relationship timeout for {author_pubkey}; {social_view}, {snapshot}");
        }
    }
}

mod desktop_smoke;
mod direct_messages;
mod private_channels;
mod waiters;

mod account_lifecycle;
