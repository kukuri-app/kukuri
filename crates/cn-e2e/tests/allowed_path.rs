//! #616 T4: 全構成 E2E の許可経路。
//!
//! まっさらな migration → 空の投影 → 公開トピック 1 件登録 → 無害な文章・画像の投稿が
//! ノード間同期 → 走査（wiremock の Arachnid / 視覚言語モデルは許可応答）→ 判定の永続化 →
//! Postgres の真実源と ArcadeDB の投影へ入り、検索・発見・おすすめの各 API から取得できる。
//! 取得したメディアが cn-indexer 側の blob 保存領域に残らないことも検証する。

use std::time::Duration;

use anyhow::Result;
use kukuri_cn_core::inspect_index_integrity;
use kukuri_cn_e2e::E2eStack;
use kukuri_cn_indexer::projection::IndexProjection;
use reqwest::{Client, StatusCode};

/// 最小の PNG 風バイト列（実画像である必要はない。実在メディアは使わない）。
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
];

/// 同期 → 走査 → 投影までの許容時間。CI の並行負荷を考慮して長めに取る。
const PROJECTION_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test(flavor = "multi_thread")]
async fn harmless_text_and_image_posts_become_searchable() -> Result<()> {
    let Some(stack) = E2eStack::boot("allowed").await? else {
        return Ok(());
    };

    // 空の状態から始まる（この走行専用トピックの投影は 0 件）。
    assert_eq!(
        stack
            .projection
            .count_scope(
                kukuri_cn_core::IndexScopeKind::PublicTopic,
                stack.topic_id.as_str()
            )
            .await?,
        0,
        "the per-run scope must start empty"
    );

    // 無害な文章投稿と画像添付投稿を投稿者ノードに置く。
    let text_object = stack
        .publish_text_post("非同期処理の設計メモを共有します")
        .await?;
    let (image_object, blob_hash) = stack
        .publish_image_post("作業机の写真です", TINY_PNG, "image/png")
        .await?;

    // ノード間同期 → 常駐ワーカーの走査 → 実 ArcadeDB 投影まで通る。
    assert!(
        stack
            .wait_for_projection(text_object.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "text post did not reach the projection"
    );
    assert!(
        stack
            .wait_for_projection(image_object.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "image post did not reach the projection"
    );

    // 真実源（実 Postgres）: 索引の総数と、索引対象に private channel が無いこと。
    let findings = inspect_index_integrity(&stack.pool).await?;
    assert_eq!(findings.index_entries_total, 2, "{findings:?}");
    assert_eq!(findings.private_scopes_supported, 0, "{findings:?}");

    // 認証 + 同意済みの利用者が検索・発見・おすすめから取得できる。
    let client = Client::new();
    let token = stack.authenticate(&client).await?;
    let search = client
        .get(format!(
            "{}/v1/index/search?scope_kind=public_topic&scope_id={}&q=設計メモ",
            stack.api_base_url, stack.topic_id
        ))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(search.status(), StatusCode::OK);
    let body = search.json::<serde_json::Value>().await?;
    assert!(
        E2eStack::entry_ids(&body).contains(&text_object),
        "search must return the harmless text post: {body}"
    );

    for path in [
        format!(
            "/v1/index/discovery?scope_kind=public_topic&scope_id={}",
            stack.topic_id
        ),
        format!(
            "/v1/index/recommendations?scope_kind=public_topic&scope_id={}",
            stack.topic_id
        ),
    ] {
        let response = client
            .get(format!("{}{path}", stack.api_base_url))
            .bearer_auth(token.as_str())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = response.json::<serde_json::Value>().await?;
        let ids = E2eStack::entry_ids(&body);
        assert!(
            ids.contains(&text_object) && ids.contains(&image_object),
            "{path} must list both harmless posts: {body}"
        );
    }

    // 走査のために一時取得したメディアが indexer 側の保存領域に残らない。
    assert!(
        stack.blob_is_absent_locally(blob_hash.as_str()).await?,
        "fetched media must not persist in the indexer blob store"
    );
    // メディア取得の成功が観測されている（実際にピア経由で取得した証跡）。
    assert!(
        stack.runtime_state.snapshot().media_fetch_success >= 1,
        "media fetch must be observed"
    );

    stack.shutdown().await
}
