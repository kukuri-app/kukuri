use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairwise_dm_offline_text_image_video_delivery_and_local_delete() {
    let _serial = acquire_scenario_test_lock().await;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let artifacts = root
        .join("test-results")
        .join("kukuri")
        .join("pairwise-dm-connectivity");
    let result = run_named_scenario(
        root,
        "pairwise_dm_offline_text_image_video_delivery_and_local_delete",
        &artifacts,
    )
    .await
    .expect("scenario");

    assert_eq!(result.status, HarnessStatus::Pass);
    assert!(artifacts.join("result.json").exists());
}
