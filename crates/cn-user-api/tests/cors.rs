//! 許可した origin の Web クライアントにだけ CORS で応答する（#1220 W8 AC-2e）。
//!
//! `with_cors` は起動時に user-api の router 全体を包む。DB の要らない `manifest_routes` を包んで検証する。

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use kukuri_cn_protocol::{
    AUTH_CHALLENGE_PATH, CHANNEL_MEMBERSHIP_SECRET_HEADER, NODE_MANIFEST_PATH,
};
use kukuri_cn_user_api::{manifest_routes, with_cors};
use reqwest::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
    ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN,
};
use reqwest::{Client, Method, Response};

const ALLOWED: &str = "https://web.kukuri.example";

async fn serve(allowed_origins: &[&str]) -> Result<String> {
    let origins: Vec<String> = allowed_origins.iter().map(|o| o.to_string()).collect();
    let app = with_cors(manifest_routes(None, Arc::new(BTreeMap::new())), &origins)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind cors test listener")?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    Ok(format!("http://{addr}"))
}

async fn preflight(base: &str, origin: &str) -> Result<Response> {
    Ok(Client::new()
        .request(Method::OPTIONS, format!("{base}{AUTH_CHALLENGE_PATH}"))
        .header(ORIGIN, origin)
        .header(ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            ACCESS_CONTROL_REQUEST_HEADERS,
            format!("authorization,content-type,{CHANNEL_MEMBERSHIP_SECRET_HEADER}"),
        )
        .send()
        .await?)
}

async fn get(base: &str, origin: &str) -> Result<Response> {
    Ok(Client::new()
        .get(format!("{base}{NODE_MANIFEST_PATH}"))
        .header(ORIGIN, origin)
        .send()
        .await?)
}

fn allow_origin(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn only_allowed_origins_get_cors_headers() -> Result<()> {
    // 末尾の `/` は origin に正規化する。
    let base = serve(&[&format!("{ALLOWED}/")]).await?;

    let response = preflight(&base, ALLOWED).await?;
    assert!(response.status().is_success(), "{}", response.status());
    assert_eq!(allow_origin(&response), Some(ALLOWED));
    let headers = response
        .headers()
        .get(ACCESS_CONTROL_ALLOW_HEADERS)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(headers.contains("authorization"), "{headers}");
    assert!(headers.contains("content-type"), "{headers}");
    assert!(
        headers.contains(CHANNEL_MEMBERSHIP_SECRET_HEADER),
        "{headers}"
    );
    let response = get(&base, ALLOWED).await?;
    assert_eq!(allow_origin(&response), Some(ALLOWED));
    let exposed = response
        .headers()
        .get(ACCESS_CONTROL_EXPOSE_HEADERS)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(exposed.contains("retry-after"), "{exposed}");

    let other = "https://other.example";
    assert_eq!(allow_origin(&preflight(&base, other).await?), None);
    assert_eq!(allow_origin(&get(&base, other).await?), None);
    Ok(())
}

#[tokio::test]
async fn no_cors_headers_without_allowed_origins() -> Result<()> {
    let base = serve(&[]).await?;
    assert_eq!(allow_origin(&preflight(&base, ALLOWED).await?), None);
    assert_eq!(allow_origin(&get(&base, ALLOWED).await?), None);
    Ok(())
}

#[test]
fn rejects_values_that_are_not_http_origins() {
    for value in ["*", "ftp://web.kukuri.example"] {
        let router = manifest_routes(None, Arc::new(BTreeMap::new()));
        assert!(with_cors(router, &[value.to_string()]).is_err(), "{value}");
    }
}
