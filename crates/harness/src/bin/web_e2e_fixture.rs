//! Web クライアントの実ブラウザ試験の相手（`kukuri_harness::run_web_e2e_fixture`）。

fn main() -> anyhow::Result<()> {
    // runtime の起動の future は大きく、debug build では main thread（Windows は 1 MiB）の stack を使い切るので、
    // stack を広げた worker で動かす。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()?;
    runtime.block_on(async { tokio::spawn(kukuri_harness::run_web_e2e_fixture()).await? })
}
