use super::*;

#[tokio::test]
async fn desktop_account_lifecycle() {
    let _guard = acquire_scenario_test_lock().await;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let artifacts = tempfile::tempdir().unwrap();
    let result = run_named_scenario(root, "desktop_account_lifecycle", artifacts.path())
        .await
        .unwrap();
    assert_eq!(result.status, HarnessStatus::Pass);
    assert_eq!(result.steps.len(), 3);
}
