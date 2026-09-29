use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    kukuri_cn_user_api::run_from_env().await
}
