//! Web クライアントの実ブラウザ試験の相手（`kukuri_harness::run_web_e2e_fixture`）。

fn main() -> anyhow::Result<()> {
    // DIAG(一時): native の warn と app-api の debug を stderr へ出す。
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "warn,kukuri_app_api=debug",
        ))
        .with_writer(std::io::stderr)
        .try_init();
    // runtime の起動の future は大きく、debug build では main thread（Windows は 1 MiB）の stack を使い切るので、
    // stack を広げた worker で動かす。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()?;
    runtime.block_on(async { tokio::spawn(kukuri_harness::run_web_e2e_fixture()).await? })
}
