use super::*;

pub(crate) fn community_node_http_client() -> Result<Client> {
    let builder = Client::builder();
    // Includes response bodies. A connected socket that never completes its
    // response must not retain the node's session lock indefinitely.
    // ブラウザの reqwest の client には期限が無い。Web の期限は runtime を組み立てる W1 AC-5 で入れる。
    #[cfg(not(target_family = "wasm"))]
    let builder = builder
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10));
    builder
        .build()
        .context("failed to build community-node http client")
}

/// 通報送信専用の HTTP クライアント(#703)。
///
/// 通報本文(詳細・連絡先)が転送応答で別ホストへ再送されないよう、転送を追跡しない。
/// 3xx は呼び出し側で `REPORT_REDIRECT_REJECTED` として扱う。ブラウザの fetch は転送を止められないので native だけ。
#[cfg(not(target_family = "wasm"))]
pub(crate) fn community_node_report_http_client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build community-node report http client")
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
