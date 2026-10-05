use crate::*;
use kukuri_desktop_runtime::{
    ClientHost, CreateAccountRequest, ExportAccountKeyRequest, InitialProfileRequest,
    SetMyProfileRequest, ensure_accounts_initialized_from_env, list_accounts,
};

pub(crate) async fn run_account_lifecycle(
    root: &Path,
    scenario: &ScenarioSpec,
    artifacts_dir: &Path,
) -> Result<HarnessResult> {
    let _env = RestoreEnvironment(
        [
            "KUKURI_DISABLE_KEYRING",
            "KUKURI_BIND_ADDR",
            "KUKURI_DISCOVERY_MODE",
            "KUKURI_DISCOVERY_SEEDS",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect(),
    );
    // Harness scenarios run serially. Keep account lifecycle traffic on loopback.
    unsafe {
        std::env::set_var("KUKURI_DISABLE_KEYRING", "1");
        std::env::set_var("KUKURI_BIND_ADDR", "127.0.0.1:0");
        std::env::set_var("KUKURI_DISCOVERY_MODE", "static_peer");
        std::env::set_var("KUKURI_DISCOVERY_SEEDS", "");
    }
    let run = tempfile::Builder::new()
        .prefix("account-lifecycle-")
        .tempdir_in(artifacts_dir)?;
    let dir = run.path().join("client");
    std::fs::create_dir_all(&dir)?;
    let mut steps = Vec::new();
    timeout(Duration::from_millis(scenario.timeouts.overall_ms), async {
        let started = Instant::now();
        let db = ensure_accounts_initialized_from_env(&dir).await?;
        let runtime = DesktopRuntime::new(&db).await?;
        let host = ClientHost::from_runtime(dir.clone(), Arc::new(runtime))
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let a = list_accounts(&dir).await?.active_account_id;
        host.save_initial_profile(InitialProfileRequest {
            account_id: a.clone(),
            profile: SetMyProfileRequest {
                display_name: Some("Retained account A".into()),
                ..Default::default()
            },
        })
        .await?;
        let exported = host
            .runtime()
            .export_account_key(ExportAccountKeyRequest {
                passphrase: "account-lifecycle-fixture".into(),
            })
            .await?;
        let post = host
            .runtime()
            .create_post(kukuri_desktop_runtime::CreatePostRequest {
                topic: scenario.fixtures.topic.clone(),
                content: "Retain this account's post".into(),
                reply_to: None,
                channel_ref: ChannelRef::Public,
                attachments: vec![],
                content_labels: vec![],
            })
            .await?;
        push_named_step(&mut steps, "account_a_profile_and_post", started);

        let started = Instant::now();
        let b = host
            .create_account(CreateAccountRequest {
                account_id: a.clone(),
                operation_id: "00000000-0000-4000-8000-000000001005".to_string(),
            })
            .await?;
        anyhow::ensure!(
            list_accounts(&dir).await?.accounts.len() == 2,
            "creation removed an account"
        );
        let next = host.logout_account(&b.id).await?;
        anyhow::ensure!(next.id == a, "logout did not return to A");
        anyhow::ensure!(
            host.runtime()
                .get_my_profile()
                .await?
                .display_name
                .as_deref()
                == Some("Retained account A"),
            "wrong profile after logout"
        );
        push_named_step(&mut steps, "create_b_logout_returns_a", started);

        let started = Instant::now();
        let c = host.logout_account(&a).await?;
        anyhow::ensure!(
            c.id != a && c.id != b.id,
            "last logout did not generate a new identity"
        );
        anyhow::ensure!(
            list_accounts(&dir).await?.accounts.len() == 1,
            "logout registration mismatch"
        );
        host.shutdown().await;
        drop(host);
        let restarted_db = ensure_accounts_initialized_from_env(&dir).await?;
        let runtime = DesktopRuntime::new(&restarted_db).await?;
        let host = ClientHost::from_runtime(dir.clone(), Arc::new(runtime))
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        anyhow::ensure!(
            list_accounts(&dir).await?.active_account_id == c.id,
            "restart generated another identity"
        );
        anyhow::ensure!(
            host.profile_setup_required(&c.id).await?,
            "new account lost initial setup"
        );
        let restored = host
            .import_account_key(exported.export, "account-lifecycle-fixture".into(), None)
            .await?;
        anyhow::ensure!(restored.id == a, "import changed original account ID");
        host.switch_account(&a).await?;
        let timeline = host
            .runtime()
            .list_timeline(kukuri_desktop_runtime::ListTimelineRequest {
                topic: scenario.fixtures.topic.clone(),
                scope: TimelineScope::Public,
                cursor: None,
                limit: Some(20),
            })
            .await?;
        anyhow::ensure!(
            timeline.items.iter().any(|item| item.object_id == post),
            "retained post missing after import"
        );
        anyhow::ensure!(
            host.runtime()
                .get_my_profile()
                .await?
                .display_name
                .as_deref()
                == Some("Retained account A"),
            "retained profile missing"
        );
        host.shutdown().await;
        push_named_step(
            &mut steps,
            "last_logout_restart_and_import_retains_data",
            started,
        );
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("account lifecycle scenario timed out")??;
    let result = HarnessResult {
        status: HarnessStatus::Pass,
        scenario: scenario.name.clone(),
        steps,
        artifacts: vec![],
        metrics_snapshot: None,
    };
    write_result_artifact(root, artifacts_dir, &result)?;
    Ok(result)
}

struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Drop for RestoreEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}
