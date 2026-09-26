use crate::*;

pub(crate) async fn run_pairwise_direct_message_connectivity(
    scenario: &ScenarioSpec,
    artifacts_dir: &Path,
) -> Result<HarnessResult> {
    unsafe { std::env::set_var("KUKURI_DISABLE_KEYRING", "1") };

    let db_a = artifacts_dir.join("pairwise-dm-a.db");
    let db_b = artifacts_dir.join("pairwise-dm-b.db");
    cleanup_runtime_artifacts(&db_a)?;
    cleanup_runtime_artifacts(&db_b)?;

    let runtime_a = DesktopRuntime::new_with_config(&db_a, TransportNetworkConfig::loopback())
        .await
        .context("failed to launch desktop a for direct-message scenario")?;
    let runtime_b = DesktopRuntime::new_with_config(&db_b, TransportNetworkConfig::loopback())
        .await
        .context("failed to launch desktop b for direct-message scenario")?;
    let overall_timeout =
        if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
            Duration::from_millis(scenario.timeouts.overall_ms).max(Duration::from_secs(600))
        } else {
            Duration::from_millis(scenario.timeouts.overall_ms)
        };
    let step_timeout = ci_timeout_floor(
        Duration::from_millis(scenario.timeouts.step_ms),
        // GitHub Actions uses a 180s transport topic-join timeout, so pairwise DM
        // delivery after restart can legitimately need several extra retry cycles
        // before the outbox drains even when the scenario eventually succeeds.
        Duration::from_secs(360),
    );

    timeout(overall_timeout, async move {
        let mut steps = Vec::new();
        let topic = scenario.fixtures.topic.as_str();
        let public_scope = TimelineScope::Public;
        let mut runtime_a = runtime_a;
        let mut runtime_b = runtime_b;

        let started_at = Instant::now();
        let ticket_a = runtime_a
            .local_peer_ticket()
            .await
            .context("failed to export ticket for desktop a")?
            .context("missing ticket for desktop a")?;
        let ticket_b = runtime_b
            .local_peer_ticket()
            .await
            .context("failed to export ticket for desktop b")?
            .context("missing ticket for desktop b")?;
        runtime_a
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_b.clone(),
            })
            .await
            .context("failed to import desktop b ticket into desktop a")?;
        runtime_b
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_a.clone(),
            })
            .await
            .context("failed to import desktop a ticket into desktop b")?;
        open_topic_column(&runtime_a, topic, &public_scope).await.context("failed to subscribe desktop a to public topic")?;
        open_topic_column(&runtime_b, topic, &public_scope).await.context("failed to subscribe desktop b to public topic")?;
        wait_for_topic_delivery(&runtime_a, topic, 1, step_timeout)
            .await
            .context("desktop a did not observe public topic connectivity")?;
        wait_for_topic_delivery(&runtime_b, topic, 1, step_timeout)
            .await
            .context("desktop b did not observe public topic connectivity")?;
        push_named_step(&mut steps, "connect", started_at);

        let started_at = Instant::now();
        let a_pubkey = runtime_a
            .get_sync_status()
            .await
            .context("status a")?
            .local_author_pubkey;
        let b_pubkey = runtime_b
            .get_sync_status()
            .await
            .context("status b")?
            .local_author_pubkey;
        wait_for_author_social_view(&runtime_a, b_pubkey.as_str(), step_timeout)
            .await
            .context("desktop a did not warm author social view for desktop b")?;
        wait_for_author_social_view(&runtime_b, a_pubkey.as_str(), step_timeout)
            .await
            .context("desktop b did not warm author social view for desktop a")?;
        runtime_a
            .follow_author(kukuri_desktop_runtime::AuthorRequest {
                pubkey: b_pubkey.clone(),
            })
            .await
            .context("desktop a failed to follow desktop b")?;
        runtime_b
            .follow_author(kukuri_desktop_runtime::AuthorRequest {
                pubkey: a_pubkey.clone(),
            })
            .await
            .context("desktop b failed to follow desktop a")?;
        wait_for_mutual_author_view_result(&runtime_a, b_pubkey.as_str(), topic, step_timeout)
            .await
            .context("desktop a did not observe mutual relationship")?;
        wait_for_mutual_author_view_result(&runtime_b, a_pubkey.as_str(), topic, step_timeout)
            .await
            .context("desktop b did not observe mutual relationship")?;
        runtime_a
            .open_direct_message(DirectMessageRequest {
                pubkey: b_pubkey.clone(),
            })
            .await
            .context("desktop a failed to open direct message")?;
        runtime_b
            .open_direct_message(DirectMessageRequest {
                pubkey: a_pubkey.clone(),
            })
            .await
            .context("desktop b failed to open direct message")?;
        wait_for_direct_message_pair_ready_with_refresh(
            &runtime_a,
            &runtime_b,
            ticket_a.as_str(),
            ticket_b.as_str(),
            a_pubkey.as_str(),
            b_pubkey.as_str(),
            step_timeout,
        )
        .await
        .context("desktops did not connect direct message peers")?;
        push_named_step(&mut steps, "mutual_ready", started_at);

        let started_at = Instant::now();
        let image_bytes = b"pairwise-dm-image";
        let image_message_id = runtime_a
            .send_direct_message(SendDirectMessageRequest {
                pubkey: b_pubkey.clone(),
                text: Some("image caption".to_string()),
                reply_to_message_id: None,
                attachments: vec![image_attachment_request(
                    "pairwise-dm-image.png",
                    "image/png",
                    image_bytes,
                )],
            })
            .await
            .context("desktop a failed to send image direct message")?;
        let delivered_image_conversation = wait_for_direct_message_conversation_result(
            &runtime_b,
            a_pubkey.as_str(),
            image_message_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b did not surface image direct message in the conversation list")?;
        assert_eq!(
            delivered_image_conversation.last_message_id.as_deref(),
            Some(image_message_id.as_str())
        );
        let delivered_image = wait_for_direct_message_result(
            &runtime_b,
            a_pubkey.as_str(),
            image_message_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b did not receive image direct message")?;
        assert_eq!(delivered_image.text, "image caption");
        assert_eq!(delivered_image.attachments.len(), 1);
        assert_eq!(delivered_image.attachments[0].role, "image_original");
        assert_eq!(delivered_image.attachments[0].mime, "image/png");
        let image_payload = runtime_b
            .get_blob_media_payload(GetBlobMediaRequest {
                hash: delivered_image.attachments[0].hash.clone(),
                mime: delivered_image.attachments[0].mime.clone(),
                source_object_id: None,
            })
            .await
            .context("desktop b failed to load image attachment payload")?
            .context("desktop b missing image attachment payload")?;
        assert_eq!(image_payload.mime, "image/png");
        assert_eq!(
            image_payload.bytes_base64,
            BASE64_STANDARD.encode(image_bytes)
        );
        wait_for_direct_message_outbox_count(&runtime_a, b_pubkey.as_str(), 0, step_timeout)
            .await
            .context("desktop a image direct message outbox did not drain")?;
        push_named_step(&mut steps, "image_delivery", started_at);

        let started_at = Instant::now();
        shutdown_runtime(runtime_b, "desktop b direct-message restart pre-shutdown")
            .await
            .context("failed to shut down desktop b before offline direct message")?;
        let queued_video_message_id = runtime_a
            .send_direct_message(SendDirectMessageRequest {
                pubkey: b_pubkey.clone(),
                text: Some("offline video".to_string()),
                reply_to_message_id: None,
                attachments: vec![
                    video_attachment_request(
                        "pairwise-dm-video.mp4",
                        "video/mp4",
                        b"pairwise-dm-video",
                        "video_manifest",
                    ),
                    video_attachment_request(
                        "pairwise-dm-video-poster.jpg",
                        "image/jpeg",
                        b"pairwise-dm-video-poster",
                        "video_poster",
                    ),
                ],
            })
            .await
            .context("desktop a failed to queue offline video direct message")?;
        let queued_status =
            wait_for_direct_message_outbox_count(&runtime_a, b_pubkey.as_str(), 1, step_timeout)
                .await
                .context("desktop a did not retain queued video direct message in outbox")?;
        assert_eq!(queued_status.pending_outbox_count, 1);
        shutdown_runtime(runtime_a, "desktop a direct-message outbox restart")
            .await
            .context("failed to shut down desktop a before outbox restart")?;
        runtime_b = DesktopRuntime::new_with_config(&db_b, TransportNetworkConfig::loopback())
            .await
            .context("failed to restart desktop b for direct-message scenario")?;
        runtime_a = DesktopRuntime::new_with_config(&db_a, TransportNetworkConfig::loopback())
            .await
            .context("failed to restart desktop a for direct-message scenario")?;
        let ticket_a_after_restart = runtime_a
            .local_peer_ticket()
            .await
            .context("failed to export restarted desktop a ticket")?
            .context("missing restarted desktop a ticket")?;
        let ticket_b_after_restart = runtime_b
            .local_peer_ticket()
            .await
            .context("failed to export restarted desktop b ticket")?
            .context("missing restarted desktop b ticket")?;
        runtime_a
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_b_after_restart.clone(),
            })
            .await
            .context("failed to import restarted desktop b ticket into desktop a")?;
        runtime_b
            .import_peer_ticket(ImportPeerTicketRequest {
                ticket: ticket_a_after_restart.clone(),
            })
            .await
            .context("failed to import desktop a ticket into restarted desktop b")?;
        open_topic_column(&runtime_a, topic, &public_scope).await.context("failed to refresh desktop a public topic after restart")?;
        open_topic_column(&runtime_b, topic, &public_scope).await.context("failed to refresh desktop b public topic after restart")?;
        wait_for_topic_delivery(&runtime_a, topic, 1, step_timeout)
            .await
            .context("desktop a did not reconnect to public topic after restart")?;
        wait_for_topic_delivery(&runtime_b, topic, 1, step_timeout)
            .await
            .context("desktop b did not reconnect to public topic after restart")?;
        wait_for_author_social_view(&runtime_a, b_pubkey.as_str(), step_timeout)
            .await
            .context("desktop a did not rewarm author social view after restart")?;
        wait_for_author_social_view(&runtime_b, a_pubkey.as_str(), step_timeout)
            .await
            .context("desktop b did not rewarm author social view after restart")?;
        wait_for_mutual_author_view_result(&runtime_a, b_pubkey.as_str(), topic, step_timeout)
            .await
            .context("desktop a did not restore mutual relationship after restart")?;
        wait_for_mutual_author_view_result(&runtime_b, a_pubkey.as_str(), topic, step_timeout)
            .await
            .context("desktop b did not restore mutual relationship after restart")?;
        runtime_a
            .open_direct_message(DirectMessageRequest {
                pubkey: b_pubkey.clone(),
            })
            .await
            .context("desktop a failed to reopen direct message after restart")?;
        runtime_b
            .open_direct_message(DirectMessageRequest {
                pubkey: a_pubkey.clone(),
            })
            .await
            .context("desktop b failed to reopen direct message after restart")?;
        wait_for_direct_message_pair_ready_with_refresh(
            &runtime_a,
            &runtime_b,
            ticket_a_after_restart.as_str(),
            ticket_b_after_restart.as_str(),
            a_pubkey.as_str(),
            b_pubkey.as_str(),
            step_timeout,
        )
        .await
        .context("desktops did not reconnect direct message peers after restart")?;
        let delivered_video = wait_for_direct_message_result_with_sender_refresh(
            &runtime_a,
            b_pubkey.as_str(),
            &runtime_b,
            a_pubkey.as_str(),
            queued_video_message_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b did not receive queued video direct message after restart")?;
        let delivered_video_conversation = wait_for_direct_message_conversation_result(
            &runtime_b,
            a_pubkey.as_str(),
            queued_video_message_id.as_str(),
            step_timeout,
        )
        .await
        .context(
            "desktop b did not surface queued video direct message in the conversation list after restart",
        )?;
        assert_eq!(
            delivered_video_conversation.last_message_id.as_deref(),
            Some(queued_video_message_id.as_str())
        );
        assert_eq!(delivered_video.text, "offline video");
        assert_eq!(delivered_video.attachments.len(), 2);
        let manifest = delivered_video
            .attachments
            .iter()
            .find(|attachment| attachment.role == "video_manifest")
            .context("missing delivered video manifest attachment")?;
        let poster = delivered_video
            .attachments
            .iter()
            .find(|attachment| attachment.role == "video_poster")
            .context("missing delivered video poster attachment")?;
        let manifest_payload = runtime_b
            .get_blob_media_payload(GetBlobMediaRequest {
                hash: manifest.hash.clone(),
                mime: manifest.mime.clone(),
                source_object_id: None,
            })
            .await
            .context("desktop b failed to load video manifest payload")?
            .context("desktop b missing video manifest payload")?;
        assert_eq!(manifest_payload.mime, "video/mp4");
        assert_eq!(
            manifest_payload.bytes_base64,
            BASE64_STANDARD.encode(b"pairwise-dm-video")
        );
        let poster_payload = runtime_b
            .get_blob_media_payload(GetBlobMediaRequest {
                hash: poster.hash.clone(),
                mime: poster.mime.clone(),
                source_object_id: None,
            })
            .await
            .context("desktop b failed to load video poster payload")?
            .context("desktop b missing video poster payload")?;
        assert_eq!(poster_payload.mime, "image/jpeg");
        assert_eq!(
            poster_payload.bytes_base64,
            BASE64_STANDARD.encode(b"pairwise-dm-video-poster")
        );
        wait_for_direct_message_outbox_count_with_pair_refresh(
            DirectMessagePairRefreshContext {
                sender_runtime: &runtime_a,
                sender_ticket: ticket_a_after_restart.as_str(),
                sender_peer_pubkey: b_pubkey.as_str(),
                receiver_runtime: &runtime_b,
                receiver_ticket: ticket_b_after_restart.as_str(),
                receiver_peer_pubkey: a_pubkey.as_str(),
            },
            0,
            step_timeout,
        )
        .await
        .context("desktop a queued video direct message outbox did not drain")?;
        push_named_step(&mut steps, "offline_video_delivery", started_at);

        let started_at = Instant::now();
        runtime_b
            .delete_direct_message_message(DeleteDirectMessageMessageRequest {
                pubkey: a_pubkey.clone(),
                message_id: queued_video_message_id.clone(),
            })
            .await
            .context("desktop b failed to delete video direct message locally")?;
        wait_for_direct_message_absence(
            &runtime_b,
            a_pubkey.as_str(),
            queued_video_message_id.as_str(),
            step_timeout,
        )
        .await
        .context("desktop b local delete did not remove video direct message")?;
        let sender_timeline = runtime_a
            .list_direct_message_messages(ListDirectMessageMessagesRequest {
                pubkey: b_pubkey.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("desktop a failed to list sender direct messages after recipient delete")?;
        assert!(
            sender_timeline
                .items
                .iter()
                .any(|message| message.message_id == queued_video_message_id),
            "recipient local delete should not remove sender history",
        );
        shutdown_runtime(
            runtime_b,
            "desktop b direct-message delete persistence restart",
        )
        .await
        .context("failed to shut down desktop b before delete persistence restart")?;
        runtime_b = DesktopRuntime::new_with_config(&db_b, TransportNetworkConfig::loopback())
            .await
            .context("failed to restart desktop b after local delete")?;
        runtime_b
            .open_direct_message(DirectMessageRequest {
                pubkey: a_pubkey.clone(),
            })
            .await
            .context("desktop b failed to reopen direct message after delete restart")?;
        let restored_timeline = runtime_b
            .list_direct_message_messages(ListDirectMessageMessagesRequest {
                pubkey: a_pubkey.clone(),
                cursor: None,
                limit: Some(20),
            })
            .await
            .context("desktop b failed to list restored direct message timeline")?;
        assert!(
            restored_timeline
                .items
                .iter()
                .any(|message| message.message_id == image_message_id),
            "desktop b should retain the earlier image direct message",
        );
        assert!(
            restored_timeline
                .items
                .iter()
                .all(|message| message.message_id != queued_video_message_id),
            "desktop b should persist the local delete across restart",
        );
        push_named_step(&mut steps, "local_delete_persisted", started_at);

        let metrics_snapshot = if scenario.artifacts.metrics_snapshot {
            Some(
                runtime_a
                    .get_sync_status()
                    .await
                    .context("failed to collect final direct-message sync status")?,
            )
        } else {
            None
        };
        shutdown_runtime(runtime_a, "desktop a final shutdown")
            .await
            .context("desktop a final shutdown timed out")?;
        shutdown_runtime(runtime_b, "desktop b final shutdown")
            .await
            .context("desktop b final shutdown timed out")?;

        let result = HarnessResult {
            status: HarnessStatus::Pass,
            scenario: scenario.name.clone(),
            steps,
            artifacts: vec![artifacts_dir.join("result.json").display().to_string()],
            metrics_snapshot,
        };
        write_result_artifact(Path::new("."), artifacts_dir, &result)?;
        Ok::<HarnessResult, anyhow::Error>(result)
    })
    .await
    .context("scenario exceeded overall timeout")?
}
