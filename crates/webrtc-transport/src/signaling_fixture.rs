//! 試験だけの接続交渉（feature `test-signaling`）。製品の交渉は W10（#1422）の専用 ALPN で行う。
//!
//! native の相手は `POST /offer` で offer を受けて answer を返し、browser の試験はそこへ offer を送る。
//! 本文は `<送り手の endpoint id>\n<session id の hex>\n<SDP>`、応答は `<相手の endpoint id>\n<SDP>`。
//! 相手の URL は `scripts/ci/browser_peer_test.sh` が `KUKURI_PEER_URL` で試験の build に渡す。

use anyhow::{Context as _, Result};
use iroh::EndpointId;

use crate::SessionId;

#[cfg(not(target_family = "wasm"))]
pub use native::{offer_routes, reachable_ip, serve};

#[cfg(target_family = "wasm")]
pub use browser::{post, post_offer};

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::{
        net::{IpAddr, Ipv4Addr, UdpSocket},
        sync::Arc,
    };

    use axum::{extract::State, http::header, routing::post};

    use super::*;
    use crate::WebRtcTransport;

    /// ブラウザが到達できる、loopback ではない手元の IP（パケットは送らない）。
    pub fn reachable_ip() -> IpAddr {
        UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .and_then(|socket| {
                socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
                socket.local_addr()
            })
            .map(|addr| addr.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    /// `POST /offer` の route。`local` は応答に載せる相手（この端）の endpoint id。
    pub fn offer_routes(transport: Arc<WebRtcTransport>, local: EndpointId) -> axum::Router {
        axum::Router::new()
            .route("/offer", post(offer))
            .with_state((transport, local))
    }

    /// 1 行目に `KUKURI_PEER_URL=<url>` を出して serve する。試験の page から呼ぶので CORS は `*`。
    pub async fn serve(app: axum::Router) -> Result<()> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        println!("KUKURI_PEER_URL=http://{}", listener.local_addr()?);
        let cors = axum::middleware::map_response(|mut response: axum::response::Response| async {
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                header::HeaderValue::from_static("*"),
            );
            response
        });
        axum::serve(listener, app.layer(cors)).await?;
        Ok(())
    }

    async fn offer(
        State((transport, local)): State<(Arc<WebRtcTransport>, EndpointId)>,
        body: String,
    ) -> String {
        match answer(&transport, &body).await {
            Ok(answer) => format!("{local}\n{answer}"),
            Err(error) => format!("error\n{error:#}"),
        }
    }

    async fn answer(transport: &WebRtcTransport, body: &str) -> Result<String> {
        let mut lines = body.splitn(3, '\n');
        let remote: EndpointId = lines.next().context("remote id")?.parse()?;
        let session = lines.next().context("session")?;
        let offer = lines.next().context("offer")?;
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(
                session.get(index * 2..index * 2 + 2).context("session")?,
                16,
            )?;
        }
        transport
            .answer(remote, SessionId::from_bytes(bytes), offer)
            .await
    }
}

#[cfg(target_family = "wasm")]
mod browser {
    use anyhow::{anyhow, ensure};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::*;

    /// 試験の相手へ `POST <url><path>` し、応答の本文を返す。
    pub async fn post(path: &str, body: &str) -> Result<String> {
        let url = option_env!("KUKURI_PEER_URL")
            .context("run the browser tests through scripts/ci/browser_peer_test.sh")?;
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        init.set_body(&JsValue::from_str(body));
        let window = web_sys::window().context("window")?;
        let response =
            JsFuture::from(window.fetch_with_str_and_init(&format!("{url}{path}"), &init))
                .await
                .map_err(|error| anyhow!("post {path}: {error:?}"))?;
        let response: web_sys::Response = response
            .dyn_into()
            .map_err(|error| anyhow!("response: {error:?}"))?;
        ensure!(response.ok(), "the native peer rejected {path}");
        let text = response
            .text()
            .map_err(|error| anyhow!("text: {error:?}"))?;
        JsFuture::from(text)
            .await
            .map_err(|error| anyhow!("read {path}: {error:?}"))?
            .as_string()
            .context("response text")
    }

    /// offer を native の相手へ送り、相手の endpoint id と answer を受け取る。
    pub async fn post_offer(
        local: EndpointId,
        session: SessionId,
        offer: &str,
    ) -> Result<(EndpointId, String)> {
        let session: String = session
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let text = post("/offer", &format!("{local}\n{session}\n{offer}")).await?;
        let (remote, answer) = text.split_once('\n').context("endpoint id and answer")?;
        ensure!(
            remote != "error",
            "the native peer failed to answer: {answer}"
        );
        Ok((remote.parse()?, answer.to_string()))
    }
}
