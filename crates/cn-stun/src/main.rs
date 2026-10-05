use std::net::Ipv4Addr;

use anyhow::{Context, Result};
use tokio::net::UdpSocket;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    kukuri_cn_runtime_support::init_tracing("info");
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, kukuri_cn_stun::PORT))
        .await
        .context("failed to bind the stun socket")?;
    tracing::info!(addr = %socket.local_addr()?, "community-node stun listening");
    kukuri_cn_stun::serve(&socket).await;
    Ok(())
}
