use super::*;

/// CN の HTTP の期限（応答の本文を含む）。
const COMMUNITY_NODE_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// CN の HTTP client。接続したまま応答を返さない相手が node の session の lock を持ち続けないよう、応答の本文まで
/// 期限で打ち切る。native は client の期限、Web（ブラウザの reqwest は client の期限を持たない）は request ごとの期限。
pub(crate) struct CommunityNodeHttpClient(Client);

impl CommunityNodeHttpClient {
    pub(crate) fn get(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::GET, url)
    }

    pub(crate) fn post(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::POST, url)
    }

    pub(crate) fn delete(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::DELETE, url)
    }

    pub(crate) fn request(
        &self,
        method: reqwest::Method,
        url: impl reqwest::IntoUrl,
    ) -> reqwest::RequestBuilder {
        let request = self.0.request(method, url);
        #[cfg(target_family = "wasm")]
        let request = request.timeout(COMMUNITY_NODE_HTTP_TIMEOUT);
        request
    }
}

pub(crate) fn community_node_http_client() -> Result<CommunityNodeHttpClient> {
    let builder = Client::builder();
    #[cfg(not(target_family = "wasm"))]
    let builder = builder
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(COMMUNITY_NODE_HTTP_TIMEOUT);
    builder
        .build()
        .map(CommunityNodeHttpClient)
        .context("failed to build community-node http client")
}

/// 通報を POST し、応答の status と本文（読めなければ空）を返す(#703)。
///
/// 通報本文(詳細・連絡先)が転送応答で別ホストへ再送されないよう、転送を追跡しない。native は 3xx を返し、呼び出し側が
/// `REPORT_REDIRECT_REJECTED` として扱う。
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn post_report(endpoint: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>)> {
    let response = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build community-node report http client")?
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await?;
    let status = response.status().as_u16();
    let body = response.bytes().await.map(|bytes| bytes.to_vec());
    Ok((status, body.unwrap_or_default()))
}

/// Web はブラウザの fetch を `redirect: "error"` で呼び、転送を失敗にする（ブラウザの reqwest は転送を止められない）。
/// 応答の本文まで期限で打ち切る。JS の object は 1 つの task の中だけで持つ（ADR 0056 §4）。
#[cfg(target_family = "wasm")]
pub async fn post_report(endpoint: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>)> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let js = |error: wasm_bindgen::JsValue| anyhow!("{error:?}");
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let endpoint = endpoint.to_string();
    n0_future::task::spawn(async move {
        let result = async {
            let init = web_sys::RequestInit::new();
            init.set_method("POST");
            init.set_redirect(web_sys::RequestRedirect::Error);
            let headers = web_sys::Headers::new().map_err(js)?;
            headers
                .set("content-type", "application/json")
                .map_err(js)?;
            init.set_headers(&headers);
            init.set_body(&js_sys::Uint8Array::from(body.as_slice()));
            let timeout = COMMUNITY_NODE_HTTP_TIMEOUT.as_millis() as u32;
            init.set_signal(Some(&web_sys::AbortSignal::timeout_with_u32(timeout)));
            let window = web_sys::window().context("no window")?;
            let response: web_sys::Response =
                JsFuture::from(window.fetch_with_str_and_init(&endpoint, &init))
                    .await
                    .map_err(js)?
                    .dyn_into()
                    .map_err(js)?;
            let body = match response.array_buffer() {
                Ok(buffer) => JsFuture::from(buffer)
                    .await
                    .map(|bytes| js_sys::Uint8Array::new(&bytes).to_vec())
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            Ok((response.status(), body))
        }
        .await;
        let _ = sender.send(result);
    });
    receiver.await.context("the report request task stopped")?
}

#[derive(Debug)]
pub(crate) enum CommunityNodeRequestError {
    AuthRequired,
    ConsentRequired,
    Other(anyhow::Error),
}

impl CommunityNodeRequestError {
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::AuthRequired => anyhow!("community node authentication is required"),
            Self::ConsentRequired => anyhow!("community node consent is required"),
            Self::Other(error) => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Establish the real HTTP exchange before pausing the clock. No real timeout
    // wait (or sleep) is needed to prove that both headers and body are bounded.
    async fn stalled_response_expires(send_headers: bool) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let (accepted, ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0; 4096];
            let received = stream.read(&mut request).await.expect("read request");
            assert!(
                received > 0,
                "client sent a request before the response stalled"
            );
            if send_headers {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\npartial")
                    .await
                    .expect("partial response");
            }
            accepted.send(()).expect("signal request");
            std::future::pending::<()>().await;
        });
        let client = community_node_http_client().expect("client");
        let request = async { client.get(url).send().await?.bytes().await };
        tokio::pin!(request);
        tokio::select! {
            result = &mut request => panic!("request unexpectedly completed: {result:?}"),
            _ = ready => {}
        }
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(31)).await;
        let result = request.now_or_never();
        server.abort();
        assert!(
            matches!(result, Some(Err(ref error)) if error.is_timeout()),
            "a stalled CN response must expire, including its body: {result:?}"
        );
    }

    #[tokio::test]
    async fn stalled_cn_headers_have_a_deadline_without_wall_clock_wait() {
        stalled_response_expires(false).await;
    }

    #[tokio::test]
    async fn stalled_cn_body_has_a_deadline_without_wall_clock_wait() {
        stalled_response_expires(true).await;
    }
}
