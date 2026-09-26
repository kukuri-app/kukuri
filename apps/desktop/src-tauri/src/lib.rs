mod app_update;
mod commands;
mod media_previews;
#[cfg(target_os = "linux")]
mod deb_update;
mod desktop_lifecycle;
#[cfg(target_os = "linux")]
mod file_dialog;
mod invoke_gate;
mod restore_lifecycle;
mod state;
mod tracing;

use ::tracing::{error, info};
use kukuri_desktop_runtime::{
    DeviceRestorePhase, ProfileLease, gui_profile, pending_device_restore_phase,
};
use tauri::{
    AppHandle, Manager, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
#[cfg(any(all(windows, not(feature = "microsoft-store")), target_os = "linux"))]
use tauri_plugin_deep_link::DeepLinkExt;

use crate::{
    commands::background_notifications::OsNotificationBackground,
    commands::developer_logs::DeveloperLogState,
    restore_lifecycle::{
        DesktopOperationState, RestoreActivationOrchestrationFailure, RestoreStartupAction,
        activate_pending_restore, advance_committed_restore_to_consent,
        orchestrate_restore_activation, publish_desktop_state,
        recover_device_restore_before_startup, restore_startup_action,
        rollback_pending_restore_and_rebuild,
    },
    state::{
        DesktopStartupState, DesktopStartupStatus, StartupError, app_consent_satisfied,
        build_desktop_state, consent_required_status, failed_status, load_app_consent_store,
        resolve_app_data_dir, resolve_db_path,
    },
    tracing::{desktop_log_buffer, init_tracing},
};

pub(crate) async fn initialize_desktop_state(app_handle: AppHandle) -> DesktopStartupStatus {
    let app_data_dir = match resolve_app_data_dir(&app_handle) {
        Ok(path) => path,
        Err(message) => {
            let status = failed_status(StartupError::unknown(message), None);
            app_handle
                .state::<DesktopStartupState>()
                .set_status(status.clone());
            return status;
        }
    };
    let pending = match pending_device_restore_phase(&app_data_dir) {
        Ok(None) => None,
        Ok(Some(DeviceRestorePhase::AwaitingConsent)) => Some(DeviceRestorePhase::AwaitingConsent),
        Ok(Some(unexpected)) => {
            let error = StartupError::unknown(format!(
                "desktop initialization found unexpected restore phase {unexpected:?}"
            ));
            let status = failed_status(error, resolve_db_path(&app_handle).ok());
            app_handle
                .state::<DesktopStartupState>()
                .set_status(status.clone());
            return status;
        }
        Err(error) => {
            let error = StartupError::unknown(format!(
                "failed to inspect pending restore before runtime initialization: {error:#}"
            ));
            let status = failed_status(error, resolve_db_path(&app_handle).ok());
            app_handle
                .state::<DesktopStartupState>()
                .set_status(status.clone());
            return status;
        }
    };

    let status = if pending == Some(DeviceRestorePhase::AwaitingConsent) {
        let activation = orchestrate_restore_activation(
            || async {
                build_desktop_state(&app_handle).await.map_err(|error| {
                    format!("failed to start restored account after consent: {error}")
                })
            },
            |state| activate_pending_restore(&app_handle, &app_data_dir, state),
            || rollback_pending_restore_and_rebuild(&app_handle, &app_data_dir),
        )
        .await;
        match activation {
            Ok(()) => {
                info!("finished pending restore activation during desktop startup");
                DesktopStartupStatus::Ready
            }
            Err(RestoreActivationOrchestrationFailure::RolledBack(message)) => {
                error!(%message, "pending restore activation failed and was rolled back");
                DesktopStartupStatus::Ready
            }
            Err(
                RestoreActivationOrchestrationFailure::RollbackFailed(message)
                | RestoreActivationOrchestrationFailure::FinishForward(message),
            ) => {
                error!(%message, "pending restore activation failed closed");
                failed_status(
                    StartupError::unknown(message),
                    resolve_db_path(&app_handle).ok(),
                )
            }
        }
    } else {
        match build_desktop_state(&app_handle).await {
            Ok(state) => match publish_desktop_state(&app_handle, state).await {
                Ok(()) => {
                    info!("initialized kukuri desktop runtime");
                    DesktopStartupStatus::Ready
                }
                Err(message) => failed_status(
                    StartupError::unknown(message),
                    resolve_db_path(&app_handle).ok(),
                ),
            },
            Err(error) => {
                error!(%error, "failed to initialize desktop runtime");
                failed_status(error, resolve_db_path(&app_handle).ok())
            }
        }
    };
    app_handle
        .state::<DesktopStartupState>()
        .set_status(status.clone());
    status
}

pub(crate) fn spawn_desktop_initialization(
    app_handle: AppHandle,
) -> tauri::async_runtime::JoinHandle<DesktopStartupStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(initialize_desktop_state(app_handle))
    })
}

/// Bring the main window back from the tray.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn shutdown_and_exit(app: &AppHandle) {
    desktop_lifecycle::request_exit(app, desktop_lifecycle::ExitAction::Quit);
}

/// トレイの登録を試みる。close時は表示先の利用可能性を別途確認する。
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "Open kukuri", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("kukuri")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => shutdown_and_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            info!("received kukuri desktop single-instance activation");
            // The app may be resident in the tray with its window hidden
            // (issue #304); a re-launch should bring it back to the front.
            show_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            // 利用可能なトレイがある場合だけ隠す。それ以外は停止処理へ進む。
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                desktop_lifecycle::close_window(window.clone());
            }
        })
        .setup(|app| {
            #[cfg(target_os = "linux")]
            file_dialog::install(app.handle())?;
            app.manage(app_update::AppUpdateState::default());
            app.manage(desktop_lifecycle::DesktopLifecycle::default());
            app.manage(desktop_lifecycle::WindowCloseState::new(
                resolve_app_data_dir(app.handle()).ok(),
            ));
            // runtimeが無い同意待ちでもrestore activation/account switchと同じlockを使う。
            app.manage(DesktopOperationState::default());
            #[cfg(unix)]
            desktop_lifecycle::watch_signals(app.handle().clone())?;

            // Restore journal recoveryは、同意fileの読取やruntime構築より必ず先に行う。
            // 回復不能ならruntimeを開始せずFailedへ閉じる。
            let resolved_profile = resolve_app_data_dir(app.handle())
                .map_err(StartupError::unknown)
                .and_then(|app_data_dir| {
                    ProfileLease::acquire(gui_profile("desktop", app_data_dir.clone()))
                        .map(|lease| (app_data_dir, lease))
                        .map_err(StartupError::from_profile_error)
                });
            let startup = match resolved_profile {
                Err(error) => {
                    error!(%error, "failed to acquire desktop profile before restore recovery");
                    (failed_status(error, None), false)
                }
                Ok((app_data_dir, profile_lease)) => {
                    app.manage(profile_lease);
                    match recover_device_restore_before_startup(&app_data_dir) {
                        Err(error) => {
                            error!(%error, "failed to recover pending device restore");
                            (failed_status(error, None), false)
                        }
                        Ok(pending_phase) => match resolve_db_path(app.handle()) {
                        Err(error) => {
                            let error = StartupError::unknown(error);
                            error!(%error, "failed to resolve consent path after restore recovery");
                            (failed_status(error, None), false)
                        }
                        Ok(db_path) => {
                            // Committedは旧同意を読む前に必ずresetする。それ以外は回復後の
                            // active registryから解決したpathだけを読む。
                            let consent_store = if pending_phase
                                == Some(DeviceRestorePhase::Committed)
                            {
                                None
                            } else {
                                Some(load_app_consent_store(&db_path))
                            };
                            let consent_satisfied = consent_store
                                .as_ref()
                                .map(app_consent_satisfied)
                                .unwrap_or(false);
                            let action =
                                restore_startup_action(pending_phase, consent_satisfied);
                            let initialize_runtime = action.initializes_runtime();
                            let status = match action {
                            RestoreStartupAction::ResetConsent => {
                                match advance_committed_restore_to_consent(
                                    &app_data_dir,
                                    &db_path,
                                ) {
                                    Ok(status) => {
                                        info!("device restore awaits explicit app consent; deferring runtime startup");
                                        status
                                    }
                                    Err(error) => {
                                        error!(%error, "failed to complete restore consent reset");
                                        failed_status(error, Some(db_path))
                                    }
                                }
                            }
                            RestoreStartupAction::Activate | RestoreStartupAction::Normal => {
                                // accept保存後〜activation完了前に停止した場合は、再同意を
                                // 繰り返さずinitialize側でactivationをfinish-forwardする。
                                DesktopStartupStatus::Initializing
                            }
                            RestoreStartupAction::AwaitConsent => {
                                let consent_store = consent_store.unwrap_or_default();
                                info!("app-level legal consent required; deferring runtime startup");
                                consent_required_status(&consent_store)
                            }
                            RestoreStartupAction::Reject(unexpected) => {
                                let error = StartupError::unknown(format!(
                                    "device restore recovery left unexpected phase {unexpected:?}"
                                ));
                                error!(%error, "device restore recovery did not reach a startup-safe phase");
                                failed_status(error, Some(db_path))
                            }
                            };
                            (status, initialize_runtime)
                        }
                        },
                    }
                }
            };
            let (initial_status, initialize_runtime) = startup;
            let startup_state = DesktopStartupState::initializing();
            startup_state.set_status(initial_status);
            app.manage(startup_state);
            app.manage(OsNotificationBackground::new(app.handle()));
            // #978: 開発者向けログ閲覧。buffer は init_tracing が組んだ process 全体の1つ。
            app.manage(DeveloperLogState::new(desktop_log_buffer()));
            app.manage(commands::link_preview::LinkPreviewState::default());
            if let Err(error) = build_tray(app.handle()) {
                error!(%error, "failed to build system tray");
            } else {
                app.state::<desktop_lifecycle::DesktopLifecycle>().set_tray_created(true);
            }
            #[cfg(target_os = "linux")]
            desktop_lifecycle::watch_hidden_tray(app.handle().clone());
            commands::background_notifications::spawn(app.handle().clone());
            // MSIX owns protocol registration through its package manifest.
            #[cfg(any(all(windows, not(feature = "microsoft-store")), target_os = "linux"))]
            app.deep_link().register_all()?;
            if initialize_runtime {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let operations = app_handle.state::<DesktopOperationState>();
                    let _guard = operations.switch_guard.lock().await;
                    if desktop_lifecycle::require_running(&app_handle).is_ok() {
                        let _ = spawn_desktop_initialization(app_handle.clone()).await;
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(invoke_gate::with_desktop_startup_gate(
            tauri::generate_handler![
            commands::startup::get_desktop_startup_status,
            commands::system_locale::get_system_locales,
            desktop_lifecycle::get_window_close_preference,
            desktop_lifecycle::set_window_close_preference,
            desktop_lifecycle::get_pending_window_close_request,
            desktop_lifecycle::respond_window_close_request,
            commands::developer_logs::set_developer_mode_enabled,
            commands::developer_logs::read_desktop_logs,
            desktop_lifecycle::restart_after_update,
            app_update::check_app_update,
            app_update::download_app_update,
            app_update::install_app_update,
            commands::external_url::open_external_url,
            commands::link_preview::fetch_link_preview,
            commands::app_consent::get_app_consent_status,
            commands::app_consent::accept_app_consents,
            commands::identity::export_account_key,
            commands::identity::preview_account_key_import,
            commands::identity::import_account_key,
            commands::identity::list_accounts,
            commands::identity::switch_account,
            commands::identity::logout_account,
            commands::identity::create_account,
            commands::identity::get_profile_setup_required,
            commands::identity::get_account_display,
            commands::identity::save_initial_profile,
            commands::device_backup::create_device_backup_command,
            commands::device_backup::preview_device_backup_command,
            commands::device_backup::restore_device_backup_command,
            commands::device_backup::cancel_device_backup,
            commands::device_backup::get_pending_device_restore_frontend_state,
            commands::device_backup::acknowledge_pending_device_restore_frontend_state,
            commands::posts::create_post,
            commands::posts::withdraw_post,
            commands::posts::create_repost,
            commands::reactions::toggle_reaction,
            commands::reactions::list_my_custom_reaction_assets,
            commands::reactions::list_recent_reactions,
            commands::reactions::create_custom_reaction_asset,
            commands::reactions::list_bookmarked_custom_reactions,
            commands::reactions::bookmark_custom_reaction,
            commands::reactions::remove_bookmarked_custom_reaction,
            commands::posts::list_bookmarked_posts_page,
            commands::posts::bookmarked_post_ids,
            commands::posts::bookmark_post,
            commands::posts::remove_bookmarked_post,
            commands::community_node::create_private_channel,
            commands::community_node::export_private_channel_invite,
            commands::community_node::import_private_channel_invite,
            commands::community_node::export_channel_access_token,
            commands::community_node::preview_channel_access_token,
            commands::community_node::import_channel_access_token,
            commands::community_node::export_friend_only_grant,
            commands::community_node::import_friend_only_grant,
            commands::community_node::export_friend_plus_share,
            commands::community_node::import_friend_plus_share,
            commands::community_node::freeze_private_channel,
            commands::community_node::rotate_private_channel,
            commands::community_node::set_private_channel_entry_dome,
            commands::community_node::leave_private_channel,
            commands::community_node::list_joined_private_channels,
            commands::posts::list_timeline,
            commands::posts::resolve_community_index_posts,
            commands::posts::list_thread,
            commands::posts::list_profile_timeline,
            commands::profile::get_my_profile,
            commands::profile::set_my_profile,
            commands::profile::follow_author,
            commands::profile::unfollow_author,
            commands::profile::get_author_social_view,
            commands::profile::mute_author,
            commands::profile::unmute_author,
            commands::profile::block_author,
            commands::profile::unblock_author,
            commands::profile::list_social_connections,
            commands::profile::list_notifications_page,
            commands::profile::mark_notification_read,
            commands::profile::mark_all_notifications_read,
            commands::profile::get_notification_status,
            commands::direct_messages::open_direct_message,
            commands::direct_messages::list_direct_messages,
            commands::direct_messages::list_direct_message_messages,
            commands::direct_messages::send_direct_message,
            commands::direct_messages::delete_direct_message_message,
            commands::direct_messages::clear_direct_message,
            commands::direct_messages::get_direct_message_status,
            commands::community_node::get_sync_status,
            commands::community_node::get_discovery_config,
            commands::live_game::list_live_sessions,
            commands::live_game::list_session_candidates,
            commands::live_game::set_session_display,
            commands::live_game::create_live_session,
            commands::live_game::end_live_session,
            commands::live_game::join_live_session,
            commands::live_game::leave_live_session,
            commands::live_game::list_game_rooms,
            commands::live_game::create_game_room,
            commands::live_game::update_game_room,
            commands::live_game::create_metaverse_room,
            commands::live_game::update_metaverse_room,
            commands::live_game::get_dome_hosting,
            commands::live_game::start_owner_dome_hosting,
            commands::live_game::delegate_dome_hosting,
            commands::live_game::close_dome_hosting,
            commands::live_game::delete_dome,
            commands::live_game::list_pending_dome_deletions,
            commands::live_game::submit_dome_session_input,
            commands::live_game::prepare_dome_transition,
            commands::live_game::preview_dome_transition_access,
            commands::live_game::commit_dome_transition,
            commands::live_game::abort_dome_transition,
            commands::live_game::commit_dome_layout,
            commands::live_game::resync_dome_snapshots,
            commands::live_game::move_dome,
            commands::live_game::list_dome_connection_topology,
            commands::live_game::create_dome_connection_proposal,
            commands::live_game::accept_dome_connection_proposal,
            commands::live_game::withdraw_dome_connection_proposal,
            commands::live_game::revoke_dome_connection,
            commands::live_game::publish_metaverse_room_event,
            commands::live_game::list_metaverse_room_events,
            commands::live_game::import_metaverse_room_asset,
            commands::community_node::import_peer_ticket,
            commands::community_node::set_discovery_seeds,
            commands::community_node::set_scope_display,
            commands::community_node::unsubscribe_topic,
            commands::community_node::set_topic_gossip_enabled,
            commands::community_node::set_channel_gossip_enabled,
            commands::community_node::get_local_peer_ticket,
            commands::posts::get_blob_media_payload,
            commands::posts::get_blob_media_file,
            commands::posts::release_blob_media_file,
            commands::posts::retry_post_elements,
            commands::posts::get_blob_preview_url,
            commands::posts::get_content_display_settings,
            commands::posts::set_adult_content_display_enabled,
            commands::community_node::get_community_node_config,
            commands::community_node::get_community_node_statuses,
            commands::community_node::set_community_node_config,
            commands::community_node::clear_community_node_config,
            commands::community_node::authenticate_community_node,
            commands::community_node::set_community_node_invite_code,
            commands::community_node::clear_community_node_token,
            commands::community_node::fetch_community_node_policies,
            commands::community_node::accept_community_node_consents,
            commands::community_node::withdraw_community_node_consents,
            commands::community_node::evaluate_author_trust_gates,
            commands::community_node::set_author_trust_display_exception,
            commands::community_node::list_author_trust_display_exceptions,
            commands::community_node::get_community_node_observation_sharing,
            commands::community_node::enable_community_node_observation_sharing,
            commands::community_node::disable_community_node_observation_sharing,
            commands::community_node::refresh_community_node_metadata,
            commands::community_node::fetch_community_node_manifest,
            commands::community_node::submit_community_node_report,
            commands::community_node::submit_community_node_tester_feedback,
            commands::community_node::submit_community_node_indexing_request,
            commands::community_node::revoke_community_node_indexing_request,
            commands::community_node::read_community_node_indexing_status,
            commands::community_node::search_community_node_index,
            commands::community_node::discover_community_node_index,
            commands::community_node::recommend_community_node_index,
            commands::community_node::lookup_community_node_content_advisories,
            commands::community_node::read_community_node_trust_user,
            commands::community_node::read_community_node_relation_user,
            commands::community_node::list_community_node_relation_neighbors,
            commands::community_node::get_community_node_relation_optout,
            commands::community_node::set_community_node_relation_optout,
            commands::community_node::clear_community_node_relation_optout,
            commands::os_notification::show_os_notification,
            commands::os_notification::get_os_notification_permission,
            commands::os_notification::request_os_notification_permission,
            commands::background_notifications::set_os_notification_settings
            ],
        ))
        .build(tauri::generate_context!())
        .expect("failed to build kukuri desktop tauri app")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event
                && !app.state::<desktop_lifecycle::DesktopLifecycle>().completed()
            {
                api.prevent_exit();
                shutdown_and_exit(app);
            }
        });
}
