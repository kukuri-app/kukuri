use anyhow::Result;
use kukuri_cn_indexer::ArcadeDbConfig;
use serde_json::Value;

pub async fn command(config: &ArcadeDbConfig, language: &str, sql: &str) -> Result<Value> {
    Ok(reqwest::Client::new()
        .post(format!(
            "{}/api/v1/command/{}",
            config.base_url.trim_end_matches('/'),
            config.database
        ))
        .basic_auth(&config.username, Some(&config.password))
        .json(&serde_json::json!({"language":language,"command":sql}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub async fn read_records(config: &ArcadeDbConfig) -> Result<u64> {
    command(config, "sql", "SELECT readRecord FROM schema:stats").await?["result"][0]["readRecord"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("schema:stats has no readRecord counter"))
}
