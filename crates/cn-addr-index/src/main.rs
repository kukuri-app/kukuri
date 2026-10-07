//! 公開 blob の発見の補助 index の server（#1632 D6、ADR 0063 §7）。
//!
//! 告知する端末の Mainline の送信元 address → 署名つき endpoint ID の record を期限つきで保持し、照会に答える。自分の
//! Mainline DHT の socket を index と共有する。blob・投稿は持たない。kukuri が運用する server（署名つきの一覧の最大
//! 2 台）として、cn-user-api・cn-iroh-relay とは別の process で動かす。

use std::str::FromStr;

use anyhow::{Context, Result};
use n0_mainline::Dht;
use udp_addr_index::{Limits, Server};

/// DHT と index で共有する UDP の port の既定値。
const DEFAULT_PORT: u16 = 60125;

#[tokio::main]
async fn main() -> Result<()> {
    kukuri_cn_runtime_support::init_tracing("info");
    let defaults = Limits::default();
    let limits = Limits {
        max_entries: env("CN_ADDR_INDEX_MAX_ENTRIES", defaults.max_entries)?,
        max_entries_per_ip: env(
            "CN_ADDR_INDEX_MAX_ENTRIES_PER_IP",
            defaults.max_entries_per_ip,
        )?,
        requests_per_ip_per_sec: env(
            "CN_ADDR_INDEX_REQUESTS_PER_IP_PER_SEC",
            defaults.requests_per_ip_per_sec,
        )?,
        ..defaults
    };
    let dht = Dht::builder()
        .server_mode()
        .port(env("CN_ADDR_INDEX_PORT", DEFAULT_PORT)?)
        .build()
        .context("failed to bind the address index socket")?;
    // kukuri の端末と node は署名つきの一覧で見つける（D6）ので、上流の共通の rendezvous には告知しない。
    let mut udp = Server::new(limits)
        .attach_with_rendezvous(dht, None)
        .await?;
    tracing::info!(addr = %udp.local_addr(), "community-node address index listening");
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("failed to wait for ctrl-c")?,
        result = udp.terminated() => result?,
    }
    Ok(())
}

/// 環境変数 `name` を読む。未設定・空なら `default`。
fn env<T: FromStr>(name: &str, default: T) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => value
            .trim()
            .parse()
            .with_context(|| format!("invalid {name}")),
        None => Ok(default),
    }
}
