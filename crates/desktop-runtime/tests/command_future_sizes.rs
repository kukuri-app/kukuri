//! Tauri の async command の future の大きさに上限を置く（#1526）。
//!
//! WebView2 の IPC callback は main thread で動き、Tauri は command の future を
//! dispatcher → `respond_async_serialized_inner` → `tokio::spawn` と値渡しで運ぶ。
//! Windows の main thread の stack reserve は既定 1 MiB（`apps/desktop/src-tauri/build.rs` で
//! 8 MiB に広げた）で、実測の使用量は約 32 KiB + 28 × 最大の future。既定の 1 MiB では
//! 33,760 B の `list_metaverse_room_events` で落ちた。command の future は `DesktopRuntime` の
//! async fn がほぼ全てなので、ここで大きさを測って上限を固定する。
use std::collections::BTreeSet;
use std::mem::size_of;
use std::path::PathBuf;

use kukuri_desktop_runtime::DesktopRuntime;

/// 1 command の future の上限。8 MiB の reserve なら約 290 KiB まで耐えるが、
/// 現在の最大（`move_dome`、約 47 KiB）に成長の余地を残しつつ 4 倍以上の余裕を取る。
const MAX_COMMAND_FUTURE_BYTES: usize = 64 * 1024;

/// `impl DesktopRuntime` の `pub async fn` のうち IPC command ではないもの。
const NOT_COMMANDS: &[&str] = &[
    "from_env",                               // constructor
    "new",                                    // constructor
    "new_with_config",                        // constructor
    "new_with_public_blob_discovery",         // constructor（harness）
    "open_in_memory_node",                    // constructor（Web）
    "start_community_node_session_scheduler", // background task
    "start_legacy_store_retirement",          // background task
    "start_sync_status_observer",             // background task
];

fn size0<F, R>(_: F) -> usize
where
    F: FnOnce(&'static DesktopRuntime) -> R,
{
    size_of::<R>()
}

fn size1<F, A, R>(_: F) -> usize
where
    F: FnOnce(&'static DesktopRuntime, A) -> R,
{
    size_of::<R>()
}

fn size2<F, A, B, R>(_: F) -> usize
where
    F: FnOnce(&'static DesktopRuntime, A, B) -> R,
{
    size_of::<R>()
}

/// 各 command の future の大きさ。実行環境は要らない（型の大きさだけを見る）。
fn command_future_sizes() -> Vec<(&'static str, usize)> {
    vec![
        (
            "abort_dome_transition",
            size1(DesktopRuntime::abort_dome_transition),
        ),
        (
            "accept_community_node_consents",
            size2(DesktopRuntime::accept_community_node_consents),
        ),
        (
            "accept_dome_connection_proposal",
            size1(DesktopRuntime::accept_dome_connection_proposal),
        ),
        (
            "account_transfer_status",
            size0(DesktopRuntime::account_transfer_status),
        ),
        (
            "authenticate_community_node",
            size1(DesktopRuntime::authenticate_community_node),
        ),
        ("block_author", size1(DesktopRuntime::block_author)),
        (
            "bookmark_custom_reaction",
            size1(DesktopRuntime::bookmark_custom_reaction),
        ),
        ("bookmark_post", size1(DesktopRuntime::bookmark_post)),
        (
            "bookmarked_post_ids",
            size1(DesktopRuntime::bookmarked_post_ids),
        ),
        (
            "cancel_account_transfer",
            size0(DesktopRuntime::cancel_account_transfer),
        ),
        (
            "clear_community_node_config",
            size0(DesktopRuntime::clear_community_node_config),
        ),
        (
            "clear_community_node_relation_optout",
            size1(DesktopRuntime::clear_community_node_relation_optout),
        ),
        (
            "clear_community_node_token",
            size1(DesktopRuntime::clear_community_node_token),
        ),
        (
            "clear_direct_message",
            size1(DesktopRuntime::clear_direct_message),
        ),
        (
            "close_dome_hosting",
            size1(DesktopRuntime::close_dome_hosting),
        ),
        (
            "commit_dome_layout",
            size1(DesktopRuntime::commit_dome_layout),
        ),
        (
            "commit_dome_transition",
            size1(DesktopRuntime::commit_dome_transition),
        ),
        (
            "create_account_transfer_invite",
            size0(DesktopRuntime::create_account_transfer_invite),
        ),
        (
            "create_custom_reaction_asset",
            size1(DesktopRuntime::create_custom_reaction_asset),
        ),
        (
            "create_dome_connection_proposal",
            size1(DesktopRuntime::create_dome_connection_proposal),
        ),
        ("create_game_room", size1(DesktopRuntime::create_game_room)),
        (
            "create_live_session",
            size1(DesktopRuntime::create_live_session),
        ),
        (
            "create_metaverse_room",
            size1(DesktopRuntime::create_metaverse_room),
        ),
        ("create_post", size1(DesktopRuntime::create_post)),
        (
            "create_private_channel",
            size1(DesktopRuntime::create_private_channel),
        ),
        ("create_repost", size1(DesktopRuntime::create_repost)),
        (
            "decide_account_transfer",
            size1(DesktopRuntime::decide_account_transfer),
        ),
        (
            "delegate_dome_hosting",
            size1(DesktopRuntime::delegate_dome_hosting),
        ),
        (
            "delete_direct_message_message",
            size1(DesktopRuntime::delete_direct_message_message),
        ),
        ("delete_dome", size1(DesktopRuntime::delete_dome)),
        (
            "disable_community_node_observation_sharing",
            size1(DesktopRuntime::disable_community_node_observation_sharing),
        ),
        (
            "discover_community_node_index",
            size1(DesktopRuntime::discover_community_node_index),
        ),
        (
            "enable_community_node_observation_sharing",
            size2(DesktopRuntime::enable_community_node_observation_sharing),
        ),
        ("end_live_session", size1(DesktopRuntime::end_live_session)),
        (
            "evaluate_author_trust_gates",
            size1(DesktopRuntime::evaluate_author_trust_gates),
        ),
        (
            "export_account_key",
            size1(DesktopRuntime::export_account_key),
        ),
        (
            "export_channel_access_token",
            size1(DesktopRuntime::export_channel_access_token),
        ),
        (
            "export_friend_only_grant",
            size1(DesktopRuntime::export_friend_only_grant),
        ),
        (
            "export_friend_plus_share",
            size1(DesktopRuntime::export_friend_plus_share),
        ),
        (
            "export_private_channel_invite",
            size1(DesktopRuntime::export_private_channel_invite),
        ),
        (
            "fetch_community_node_manifest",
            size1(DesktopRuntime::fetch_community_node_manifest),
        ),
        (
            "fetch_community_node_policies",
            size1(DesktopRuntime::fetch_community_node_policies),
        ),
        (
            "finish_protected_migration",
            size0(DesktopRuntime::finish_protected_migration),
        ),
        ("follow_author", size1(DesktopRuntime::follow_author)),
        (
            "freeze_private_channel",
            size1(DesktopRuntime::freeze_private_channel),
        ),
        (
            "get_author_social_view",
            size1(DesktopRuntime::get_author_social_view),
        ),
        (
            "get_blob_media_file",
            size2(DesktopRuntime::get_blob_media_file),
        ),
        (
            "get_blob_media_payload",
            size1(DesktopRuntime::get_blob_media_payload),
        ),
        (
            "get_blob_preview_url",
            size1(DesktopRuntime::get_blob_preview_url),
        ),
        (
            "get_community_node_config",
            size0(DesktopRuntime::get_community_node_config),
        ),
        (
            "get_community_node_observation_sharing",
            size1(DesktopRuntime::get_community_node_observation_sharing),
        ),
        (
            "get_community_node_relation_optout",
            size1(DesktopRuntime::get_community_node_relation_optout),
        ),
        (
            "get_community_node_statuses",
            size0(DesktopRuntime::get_community_node_statuses),
        ),
        (
            "get_direct_message_status",
            size1(DesktopRuntime::get_direct_message_status),
        ),
        (
            "get_discovery_config",
            size0(DesktopRuntime::get_discovery_config),
        ),
        ("get_dome_hosting", size1(DesktopRuntime::get_dome_hosting)),
        ("get_my_profile", size0(DesktopRuntime::get_my_profile)),
        (
            "get_notification_status",
            size0(DesktopRuntime::get_notification_status),
        ),
        ("get_sync_status", size0(DesktopRuntime::get_sync_status)),
        (
            "has_topic_timeline_doc_index_entry",
            size2(DesktopRuntime::has_topic_timeline_doc_index_entry),
        ),
        (
            "import_channel_access_token",
            size1(DesktopRuntime::import_channel_access_token),
        ),
        (
            "import_friend_only_grant",
            size1(DesktopRuntime::import_friend_only_grant),
        ),
        (
            "import_friend_plus_share",
            size1(DesktopRuntime::import_friend_plus_share),
        ),
        (
            "import_metaverse_room_asset",
            size1(DesktopRuntime::import_metaverse_room_asset),
        ),
        (
            "import_peer_ticket",
            size1(DesktopRuntime::import_peer_ticket),
        ),
        (
            "import_private_channel_invite",
            size1(DesktopRuntime::import_private_channel_invite),
        ),
        (
            "join_live_session",
            size1(DesktopRuntime::join_live_session),
        ),
        (
            "leave_live_session",
            size1(DesktopRuntime::leave_live_session),
        ),
        (
            "leave_private_channel",
            size1(DesktopRuntime::leave_private_channel),
        ),
        (
            "list_author_trust_display_exceptions",
            size0(DesktopRuntime::list_author_trust_display_exceptions),
        ),
        (
            "list_bookmarked_custom_reactions",
            size0(DesktopRuntime::list_bookmarked_custom_reactions),
        ),
        (
            "list_bookmarked_posts_page",
            size1(DesktopRuntime::list_bookmarked_posts_page),
        ),
        (
            "list_community_node_relation_neighbors",
            size1(DesktopRuntime::list_community_node_relation_neighbors),
        ),
        (
            "list_connectivity_peers",
            size1(DesktopRuntime::list_connectivity_peers),
        ),
        (
            "list_direct_message_messages",
            size1(DesktopRuntime::list_direct_message_messages),
        ),
        (
            "list_direct_messages",
            size0(DesktopRuntime::list_direct_messages),
        ),
        (
            "list_dome_connection_topology",
            size1(DesktopRuntime::list_dome_connection_topology),
        ),
        ("list_game_rooms", size1(DesktopRuntime::list_game_rooms)),
        (
            "list_joined_private_channels",
            size1(DesktopRuntime::list_joined_private_channels),
        ),
        (
            "list_live_sessions",
            size1(DesktopRuntime::list_live_sessions),
        ),
        (
            "list_metaverse_room_events",
            size1(DesktopRuntime::list_metaverse_room_events),
        ),
        (
            "list_my_custom_reaction_assets",
            size0(DesktopRuntime::list_my_custom_reaction_assets),
        ),
        (
            "list_notification_dispatch_after",
            size1(DesktopRuntime::list_notification_dispatch_after),
        ),
        (
            "list_notifications",
            size0(DesktopRuntime::list_notifications),
        ),
        (
            "list_notifications_page",
            size1(DesktopRuntime::list_notifications_page),
        ),
        (
            "list_pending_dome_deletions",
            size1(DesktopRuntime::list_pending_dome_deletions),
        ),
        (
            "list_profile_timeline",
            size1(DesktopRuntime::list_profile_timeline),
        ),
        (
            "list_recent_reactions",
            size1(DesktopRuntime::list_recent_reactions),
        ),
        (
            "list_session_candidates",
            size1(DesktopRuntime::list_session_candidates),
        ),
        (
            "list_social_connections",
            size1(DesktopRuntime::list_social_connections),
        ),
        ("list_thread", size1(DesktopRuntime::list_thread)),
        ("list_timeline", size1(DesktopRuntime::list_timeline)),
        (
            "local_peer_ticket",
            size0(DesktopRuntime::local_peer_ticket),
        ),
        (
            "lookup_community_node_content_advisories",
            size1(DesktopRuntime::lookup_community_node_content_advisories),
        ),
        (
            "mark_all_notifications_read",
            size0(DesktopRuntime::mark_all_notifications_read),
        ),
        (
            "mark_notification_read",
            size1(DesktopRuntime::mark_notification_read),
        ),
        ("move_dome", size1(DesktopRuntime::move_dome)),
        ("mute_author", size1(DesktopRuntime::mute_author)),
        (
            "notification_dispatch_head",
            size0(DesktopRuntime::notification_dispatch_head),
        ),
        (
            "open_direct_message",
            size1(DesktopRuntime::open_direct_message),
        ),
        (
            "post_display_retry_at",
            size1(DesktopRuntime::post_display_retry_at),
        ),
        (
            "prepare_dome_transition",
            size1(DesktopRuntime::prepare_dome_transition),
        ),
        (
            "preview_channel_access_token",
            size1(DesktopRuntime::preview_channel_access_token),
        ),
        (
            "preview_dome_transition_access",
            size1(DesktopRuntime::preview_dome_transition_access),
        ),
        (
            "publish_metaverse_room_event",
            size1(DesktopRuntime::publish_metaverse_room_event),
        ),
        (
            "read_community_node_indexing_status",
            size1(DesktopRuntime::read_community_node_indexing_status),
        ),
        (
            "read_community_node_relation_user",
            size1(DesktopRuntime::read_community_node_relation_user),
        ),
        (
            "read_community_node_trust_user",
            size1(DesktopRuntime::read_community_node_trust_user),
        ),
        (
            "read_link_preview_record",
            size2(DesktopRuntime::read_link_preview_record),
        ),
        (
            "recommend_community_node_index",
            size1(DesktopRuntime::recommend_community_node_index),
        ),
        (
            "record_link_preview",
            size2(DesktopRuntime::record_link_preview),
        ),
        (
            "refresh_community_node_metadata",
            size1(DesktopRuntime::refresh_community_node_metadata),
        ),
        (
            "remove_bookmarked_custom_reaction",
            size1(DesktopRuntime::remove_bookmarked_custom_reaction),
        ),
        (
            "remove_bookmarked_post",
            size1(DesktopRuntime::remove_bookmarked_post),
        ),
        (
            "resolve_community_index_posts",
            size1(DesktopRuntime::resolve_community_index_posts),
        ),
        (
            "resync_dome_snapshots",
            size1(DesktopRuntime::resync_dome_snapshots),
        ),
        (
            "retry_post_elements",
            size1(DesktopRuntime::retry_post_elements),
        ),
        (
            "revoke_community_node_indexing_request",
            size1(DesktopRuntime::revoke_community_node_indexing_request),
        ),
        (
            "revoke_dome_connection",
            size1(DesktopRuntime::revoke_dome_connection),
        ),
        (
            "rotate_private_channel",
            size1(DesktopRuntime::rotate_private_channel),
        ),
        (
            "search_community_node_index",
            size1(DesktopRuntime::search_community_node_index),
        ),
        (
            "send_direct_message",
            size1(DesktopRuntime::send_direct_message),
        ),
        (
            "set_adult_content_display_enabled",
            size1(DesktopRuntime::set_adult_content_display_enabled),
        ),
        (
            "set_author_trust_display_exception",
            size1(DesktopRuntime::set_author_trust_display_exception),
        ),
        (
            "set_channel_gossip_enabled",
            size1(DesktopRuntime::set_channel_gossip_enabled),
        ),
        (
            "set_community_node_config",
            size1(DesktopRuntime::set_community_node_config),
        ),
        (
            "set_community_node_invite_code",
            size1(DesktopRuntime::set_community_node_invite_code),
        ),
        (
            "set_community_node_relation_optout",
            size1(DesktopRuntime::set_community_node_relation_optout),
        ),
        (
            "set_discovery_seeds",
            size1(DesktopRuntime::set_discovery_seeds),
        ),
        (
            "set_public_blob_discovery",
            size1(DesktopRuntime::set_public_blob_discovery),
        ),
        ("set_my_profile", size1(DesktopRuntime::set_my_profile)),
        (
            "set_private_channel_entry_dome",
            size1(DesktopRuntime::set_private_channel_entry_dome),
        ),
        (
            "set_scope_display",
            size1(DesktopRuntime::set_scope_display),
        ),
        (
            "set_session_display",
            size1(DesktopRuntime::set_session_display),
        ),
        (
            "set_topic_gossip_enabled",
            size1(DesktopRuntime::set_topic_gossip_enabled),
        ),
        ("shutdown", size0(DesktopRuntime::shutdown)),
        ("shutdown_checked", size0(DesktopRuntime::shutdown_checked)),
        (
            "start_owner_dome_hosting",
            size1(DesktopRuntime::start_owner_dome_hosting),
        ),
        (
            "submit_community_node_indexing_request",
            size1(DesktopRuntime::submit_community_node_indexing_request),
        ),
        (
            "submit_community_node_report",
            size1(DesktopRuntime::submit_community_node_report),
        ),
        (
            "submit_community_node_tester_feedback",
            size1(DesktopRuntime::submit_community_node_tester_feedback),
        ),
        (
            "submit_dome_session_input",
            size1(DesktopRuntime::submit_dome_session_input),
        ),
        (
            "take_private_channel_controller",
            size1(DesktopRuntime::take_private_channel_controller),
        ),
        ("toggle_reaction", size1(DesktopRuntime::toggle_reaction)),
        ("unblock_author", size1(DesktopRuntime::unblock_author)),
        ("unfollow_author", size1(DesktopRuntime::unfollow_author)),
        ("unmute_author", size1(DesktopRuntime::unmute_author)),
        (
            "unsubscribe_topic",
            size1(DesktopRuntime::unsubscribe_topic),
        ),
        ("update_game_room", size1(DesktopRuntime::update_game_room)),
        (
            "update_metaverse_room",
            size1(DesktopRuntime::update_metaverse_room),
        ),
        (
            "withdraw_community_node_consents",
            size1(DesktopRuntime::withdraw_community_node_consents),
        ),
        (
            "withdraw_dome_connection_proposal",
            size1(DesktopRuntime::withdraw_dome_connection_proposal),
        ),
        ("withdraw_post", size1(DesktopRuntime::withdraw_post)),
    ]
}

/// `impl DesktopRuntime` を含む source の `pub async fn` 名。一覧の漏れを検出する。
fn desktop_runtime_async_fns() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut dirs = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            if !source.contains("impl DesktopRuntime") {
                continue;
            }
            for (index, marker) in source.match_indices("pub async fn ") {
                let name: String = source[index + marker.len()..]
                    .chars()
                    .take_while(|character| character.is_alphanumeric() || *character == '_')
                    .collect();
                names.insert(name);
            }
        }
    }
    names
}

#[test]
fn command_futures_stay_under_the_main_thread_budget() {
    let sizes = command_future_sizes();
    if let Some((name, size)) = sizes.iter().max_by_key(|(_, size)| *size) {
        println!("largest command future: {name} = {size} B");
    }
    let too_large: Vec<_> = sizes
        .iter()
        .filter(|(_, size)| *size > MAX_COMMAND_FUTURE_BYTES)
        .collect();
    assert!(
        too_large.is_empty(),
        "command の future が上限 {MAX_COMMAND_FUTURE_BYTES} B を超えている: {too_large:?}"
    );

    let listed: BTreeSet<&str> = sizes
        .iter()
        .map(|(name, _)| *name)
        .chain(NOT_COMMANDS.iter().copied())
        .collect();
    let in_source = desktop_runtime_async_fns();
    let missing: Vec<_> = in_source
        .iter()
        .filter(|name| !listed.contains(name.as_str()))
        .collect();
    let stale: Vec<_> = listed
        .iter()
        .filter(|name| !in_source.contains(**name))
        .collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "一覧と source がずれている。missing: {missing:?}, stale: {stale:?}"
    );
}
