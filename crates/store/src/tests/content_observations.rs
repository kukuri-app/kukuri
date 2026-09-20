use super::*;

fn observation_projection_row(object_id: &str) -> ObjectProjectionRow {
    ObjectProjectionRow {
        object_id: EnvelopeId::from(object_id),
        topic_id: "kukuri:topic:observations".to_string(),
        channel_id: "public".to_string(),
        author_pubkey: "a".repeat(64),
        created_at: 1,
        object_kind: "post".to_string(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::BlobText {
            hash: BlobHash::new("1".repeat(64)),
            mime: "text/plain".to_string(),
            bytes: 4,
        },
        content: Some("body".to_string()),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new("topic::observations"),
        source_key: format!("objects/{object_id}/header"),
        source_envelope_id: EnvelopeId::from(object_id),
        source_blob_hash: None,
        source_docs_author: None,
        derived_at: 1,
        projection_version: 2,
    }
}

async fn content_observation_scenario<S>(store: &S)
where
    S: ContentObservationStore + ObjectProjectionStore,
{
    let object_id = EnvelopeId::from("observed-post");
    let observation = ContentObservationRow {
        subject_kind: "post".to_string(),
        subject_id: object_id.as_str().to_string(),
        node_base_url: "https://node.example".to_string(),
        capability: "community_index".to_string(),
        observed_at: 10,
    };

    assert!(
        !store
            .put_content_observation(observation.clone())
            .await
            .unwrap()
    );
    assert!(
        store
            .list_content_observations_at("post", object_id.as_str(), observation.observed_at)
            .await
            .unwrap()
            .is_empty()
    );

    store
        .put_object_projection(observation_projection_row(object_id.as_str()))
        .await
        .unwrap();
    assert!(
        store
            .put_content_observation(observation.clone())
            .await
            .unwrap()
    );

    let mut refreshed = observation.clone();
    refreshed.observed_at = 20;
    assert!(
        store
            .put_content_observation(refreshed.clone())
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .list_content_observations_at("post", object_id.as_str(), refreshed.observed_at)
            .await
            .unwrap(),
        vec![refreshed]
    );

    store.rebuild_object_projections(Vec::new()).await.unwrap();
    assert!(
        store
            .list_content_observations_at("post", object_id.as_str(), 20)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn content_observation_retention_scenario<S>(store: &S)
where
    S: ContentObservationStore + ObjectProjectionStore,
{
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    let object_id = EnvelopeId::from("retained-post");
    store
        .put_object_projection(observation_projection_row(object_id.as_str()))
        .await
        .unwrap();
    let now = 100 * DAY_MS;
    for (node_base_url, observed_at) in [
        ("https://expired.example", now - 90 * DAY_MS - 1),
        ("https://boundary.example", now - 90 * DAY_MS),
        ("https://current.example", now),
    ] {
        assert!(
            store
                .put_content_observation(ContentObservationRow {
                    subject_kind: "post".to_string(),
                    subject_id: object_id.as_str().to_string(),
                    node_base_url: node_base_url.to_string(),
                    capability: "community_index".to_string(),
                    observed_at,
                })
                .await
                .unwrap()
        );
    }
    let rows = store
        .list_content_observations_at("post", object_id.as_str(), now)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|row| row.node_base_url == "https://boundary.example")
    );
    assert!(
        rows.iter()
            .all(|row| row.node_base_url != "https://expired.example")
    );
}

async fn content_observation_limit_scenario<S>(store: &S)
where
    S: ContentObservationStore + ObjectProjectionStore,
{
    let object_id = EnvelopeId::from("limited-post");
    store
        .put_object_projection(observation_projection_row(object_id.as_str()))
        .await
        .unwrap();
    for index in 0..2049 {
        assert!(
            store
                .put_content_observation(ContentObservationRow {
                    subject_kind: "post".to_string(),
                    subject_id: object_id.as_str().to_string(),
                    node_base_url: format!("https://node-{index:04}.example"),
                    capability: "community_index".to_string(),
                    observed_at: index,
                })
                .await
                .unwrap()
        );
    }
    let rows = store
        .list_content_observations_at("post", object_id.as_str(), 2048)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2048);
    assert!(
        rows.iter()
            .all(|row| row.node_base_url != "https://node-0000.example")
    );
    assert!(
        rows.iter()
            .any(|row| row.node_base_url == "https://node-2048.example")
    );
}

async fn content_observation_expires_without_another_write_scenario<S>(store: &S)
where
    S: ContentObservationStore + ObjectProjectionStore,
{
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    let object_id = EnvelopeId::from("time-only-expiring-post");
    let observed_at = 10;
    let observation = ContentObservationRow {
        subject_kind: "post".to_string(),
        subject_id: object_id.as_str().to_string(),
        node_base_url: "https://time-only.example".to_string(),
        capability: "community_index".to_string(),
        observed_at,
    };
    store
        .put_object_projection(observation_projection_row(object_id.as_str()))
        .await
        .unwrap();
    assert!(
        store
            .put_content_observation(observation.clone())
            .await
            .unwrap()
    );

    assert_eq!(
        store
            .list_content_observations_at("post", object_id.as_str(), observed_at + 90 * DAY_MS,)
            .await
            .unwrap(),
        vec![observation]
    );
    assert!(
        store
            .list_content_observations_at(
                "post",
                object_id.as_str(),
                observed_at + 90 * DAY_MS + 1,
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_content_observations_at("post", object_id.as_str(), observed_at)
            .await
            .unwrap()
            .is_empty(),
        "期限切れ記録は読み取り時に物理削除される"
    );
}

#[tokio::test]
async fn content_observation_requires_local_subject_and_refreshes_timestamp() {
    content_observation_scenario(&MemoryStore::default()).await;
    content_observation_scenario(&SqliteStore::connect_memory().await.unwrap()).await;
}

#[tokio::test]
async fn content_observation_removes_rows_older_than_ninety_days() {
    content_observation_retention_scenario(&MemoryStore::default()).await;
    content_observation_retention_scenario(&SqliteStore::connect_memory().await.unwrap()).await;
}

#[tokio::test]
async fn content_observation_keeps_only_the_newest_2048_rows() {
    content_observation_limit_scenario(&MemoryStore::default()).await;
    content_observation_limit_scenario(&SqliteStore::connect_memory().await.unwrap()).await;
}

#[tokio::test]
async fn content_observation_expires_after_time_passes_without_another_write() {
    content_observation_expires_without_another_write_scenario(&MemoryStore::default()).await;
    content_observation_expires_without_another_write_scenario(
        &SqliteStore::connect_memory().await.unwrap(),
    )
    .await;
}
