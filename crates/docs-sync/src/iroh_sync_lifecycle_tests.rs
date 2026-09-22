use super::*;

#[tokio::test]
async fn bucket_close_retries_leave_failure_without_repolling_the_event_task() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let replica = crate::topic_replica_id("bucket-close-retry");
    docs.open_replica(&replica).await?;
    *docs.close_hook.lock().await = Some(TestHook::FailLeave);
    assert!(docs.close_replica(&replica).await.is_err());
    assert!(
        docs.query_replica_with_policy(&replica, DocQuery::All, DocFetchPolicy::LocalOnly)
            .await
            .is_err()
    );
    assert!(
        docs.query_local_source(&replica, "key", None, 8)
            .await
            .is_err()
    );
    docs.close_replica(&replica).await?;
    assert!(
        docs.query_replica_with_policy(&replica, DocQuery::All, DocFetchPolicy::LocalOnly)
            .await?
            .is_empty()
    );
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn bucket_close_lost_ack_does_not_leave_a_closed_handle_in_the_cache() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let replica = crate::topic_replica_id("bucket-close-lost-ack");
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: "kept".into(),
            value: b"body".to_vec(),
        },
    )
    .await?;
    *docs.close_hook.lock().await = Some(TestHook::LoseCloseAck);
    assert!(docs.close_replica(&replica).await.is_err());
    docs.close_replica(&replica).await?;
    let rows = docs
        .query_replica_with_policy(&replica, DocQuery::All, DocFetchPolicy::LocalOnly)
        .await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value, b"body");
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn bucket_close_and_revoke_finish_after_the_caller_is_cancelled() -> Result<()> {
    for revoke in [false, true] {
        let node = IrohDocsNode::memory().await?;
        let docs = IrohDocsSync::new(node.clone());
        let replica = crate::private_channel_epoch_replica_id("bucket-cancel", "e1");
        let secret = NamespaceSecret::from_bytes(&[5; 32]);
        docs.register_private_replica_secret(&replica, &hex::encode(secret.to_bytes()))
            .await?;
        docs.open_replica(&replica).await?;
        let probe = node.docs().open(secret.id()).await?.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *docs.close_hook.lock().await = Some(TestHook::Pause {
            entered: entered.clone(),
            resume: resume.clone(),
        });
        let caller = tokio::spawn({
            let docs = docs.clone();
            let replica = replica.clone();
            async move {
                if revoke {
                    docs.remove_private_replica_secret(&replica).await
                } else {
                    docs.close_replica(&replica).await
                }
            }
        });
        entered.notified().await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let restart = tokio::spawn({
            let docs = docs.clone();
            let replica = replica.clone();
            async move { docs.restart_replica_sync(&replica).await }
        });
        resume.notify_one();
        restart.await??;
        assert!(
            !probe.status().await?.sync,
            "a queued restart resurrected the closed replica"
        );
        let read = docs
            .query_replica_with_policy(&replica, DocQuery::All, DocFetchPolicy::LocalOnly)
            .await;
        assert_eq!(read.is_err(), revoke);
        assert!(!probe.status().await?.sync);
        probe.close().await?;
        docs.shutdown().await;
        node.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn bucket_revoke_cancelled_before_ownership_does_not_remove_the_capability() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let docs = IrohDocsSync::new(node.clone());
    let replica = crate::private_channel_epoch_replica_id("bucket-before-owner", "e1");
    docs.register_private_replica_secret(&replica, &hex::encode([8; 32]))
        .await?;
    docs.open_replica(&replica).await?;
    let guard = docs.close_tasks.lock().await;
    {
        let removal = docs.remove_private_replica_secret(&replica);
        tokio::pin!(removal);
        assert!(futures_util::poll!(removal.as_mut()).is_pending());
        // futureをこのscopeでdrop。sleepではなく最初の待ち地点まで実際にpollする。
    }
    drop(guard);
    docs.open_replica(&replica).await?;
    docs.remove_private_replica_secret(&replica).await?;
    assert!(docs.open_replica(&replica).await.is_err());
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}
