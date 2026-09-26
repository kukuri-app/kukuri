//! #616 T5: 障害経路。プロバイダ・メディア取得の失敗が「保留」になり、
//! 許可へ落ちない（fail-closed）ことを全構成で固定する。
//!
//! - 視覚言語モデル停止（模擬の応答を全て外す）→ 走査失敗として保留
//! - Arachnid の時間切れ（応答遅延 > 応答待ち上限）→ 保留
//! - メディア保持者への不達（実体の無い blob 参照）→ 保留
//! - 大きさ超過（取得上限より大きい blob）→ 保留
//!
//! いずれも真実源の `provider_failure_allowed = 0`（失敗が許可へ落ちていない）と
//! 「索引 0 件」を突合する。

use std::time::Duration;

use anyhow::Result;
use kukuri_cn_core::{IndexScopeKind, inspect_index_integrity};
use kukuri_cn_e2e::{E2eOptions, E2eStack};
use kukuri_cn_indexer::config::MediaFetchConfig;
use kukuri_cn_indexer::projection::IndexProjection;
use reqwest::{Client, StatusCode};

const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
];
const HOLD_TIMEOUT: Duration = Duration::from_secs(120);

/// 保留（索引 0 件・許可落ち 0 件）を突合する共通の後段検証。
async fn assert_held(stack: &E2eStack, object_id: &str) -> Result<()> {
    assert!(
        !stack
            .projection
            .contains_object(
                IndexScopeKind::PublicTopic,
                stack.topic_id.as_str(),
                object_id
            )
            .await?,
        "held object {object_id} must not reach the projection"
    );
    let findings = inspect_index_integrity(&stack.pool).await?;
    assert_eq!(findings.index_entries_total, 0, "{findings:?}");
    assert_eq!(findings.provider_failure_allowed, 0, "{findings:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn vlm_outage_holds_text_posts() -> Result<()> {
    let Some(stack) = E2eStack::boot("vlmdown").await? else {
        return Ok(());
    };
    // 模擬の応答を全て外す = 視覚言語モデル停止（未定義の要求は 404 になる）。
    stack.vlm.reset().await;

    let object_id = stack.publish_text_post("停止中に流れた無害な文章").await?;
    assert!(
        stack
            .wait_for_state(
                |state| state.scanned >= 1 && state.skipped_non_allow >= 1,
                HOLD_TIMEOUT
            )
            .await?,
        "scan must run and hold while the vlm is down"
    );
    assert_held(&stack, object_id.as_str()).await?;
    stack.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn arachnid_timeout_holds_media_posts() -> Result<()> {
    let Some(stack) = E2eStack::boot("shieldslow").await? else {
        return Ok(());
    };
    // 応答待ち上限（5 秒）を超える遅延応答 = 時間切れ。
    stack.mount_arachnid_delay(Duration::from_secs(8)).await;

    let (object_id, _) = stack
        .publish_image_post("時間切れ対象のメディア投稿", TINY_PNG, "image/png")
        .await?;
    assert!(
        stack
            .wait_for_state(|state| state.skipped_non_allow >= 1, HOLD_TIMEOUT)
            .await?,
        "scan must hold when the arachnid probe times out"
    );
    assert_held(&stack, object_id.as_str()).await?;
    stack.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_media_holder_holds_posts() -> Result<()> {
    let Some(stack) = E2eStack::boot("nomedia").await? else {
        return Ok(());
    };
    // どのノードも実体を持たない blob 参照（保持者への不達）。
    let missing_hash = "a".repeat(64);
    let object_id = stack
        .publish_post_with_missing_media(
            "実体の無い添付を持つ投稿",
            missing_hash.as_str(),
            "image/png",
        )
        .await?;
    assert!(
        stack
            .wait_for_state(
                |state| state.skipped_non_allow >= 1 || state.media_fetch_unavailable >= 1,
                HOLD_TIMEOUT
            )
            .await?,
        "scan must hold when the media holder is unreachable"
    );
    assert_held(&stack, object_id.as_str()).await?;
    stack.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_mime_media_holds_posts_without_retaining_the_blob() -> Result<()> {
    let Some(stack) = E2eStack::boot("unknownmime").await? else {
        return Ok(());
    };
    let (object_id, thumbnail_hash) = stack
        .publish_post_with_unknown_mime_thumbnail("unknown MIME thumbnail")
        .await?;
    assert!(
        stack
            .wait_for_state(
                |state| state.skipped_non_allow >= 1 || state.scan_errors >= 1,
                HOLD_TIMEOUT
            )
            .await?,
        "scan must hold when MIME metadata and recognizable magic bytes are both absent"
    );
    assert_held(&stack, object_id.as_str()).await?;
    assert!(
        stack
            .blob_is_absent_locally(thumbnail_hash.as_str())
            .await?,
        "unknown-MIME media must remain ephemeral on the indexer"
    );
    stack.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn oversize_media_holds_posts() -> Result<()> {
    let Some(stack) = E2eStack::boot_with(
        "oversize",
        E2eOptions {
            media_fetch: Some(MediaFetchConfig {
                max_bytes: 8,
                timeout: Duration::from_secs(30),
            }),
        },
    )
    .await?
    else {
        return Ok(());
    };

    // 取得上限（8 bytes）より大きいメディア（12 bytes）。
    let (object_id, _) = stack
        .publish_image_post("上限超過のメディア投稿", TINY_PNG, "image/png")
        .await?;
    assert!(
        stack
            .wait_for_state(
                |state| state.skipped_non_allow >= 1 || state.media_fetch_oversize >= 1,
                HOLD_TIMEOUT
            )
            .await?,
        "scan must hold when the media exceeds the fetch limit"
    );
    assert_held(&stack, object_id.as_str()).await?;
    stack.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_provider_keeps_new_posts_out_of_surfaces_until_recovery() -> Result<()> {
    let Some(stack) = E2eStack::boot("providerloss").await? else {
        return Ok(());
    };

    // 障害前に許可済みの投稿を1件作り、通常経路が成立していることを先に固定する。
    let baseline = stack.publish_text_post("replica failure baseline").await?;
    assert!(
        stack
            .wait_for_projection(baseline.as_str(), HOLD_TIMEOUT)
            .await?,
        "baseline post did not reach the projection"
    );

    let client = Client::new();
    let token = stack.authenticate(&client).await?;

    // 投稿者の登録を失効させ、indexer が読める提供元を無くす（bucket の読取りは登録済みの提供元だけを読む）。
    stack.set_author_provider_available(false).await?;
    let blocked = stack
        .publish_text_post("replica-failure-marker must stay hidden")
        .await?;
    // 需要の周期（300ms）の巡回が何度も回る間、索引に入らない。
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(
        !stack
            .projection
            .contains_object(
                IndexScopeKind::PublicTopic,
                stack.topic_id.as_str(),
                blocked.as_str(),
            )
            .await?,
        "a post published while no provider is reachable must not reach ArcadeDB"
    );
    let findings = inspect_index_integrity(&stack.pool).await?;
    assert_eq!(
        findings.index_entries_total, 1,
        "an unreachable provider must not add a truth entry: {findings:?}"
    );

    let search_url = format!(
        "{}/v1/index/search?scope_kind=public_topic&scope_id={}&q=replica-failure-marker",
        stack.api_base_url, stack.topic_id,
    );
    let response = client
        .get(&search_url)
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json::<serde_json::Value>().await?;
    assert!(
        !E2eStack::entry_ids(&body).contains(&blocked),
        "an unreachable provider must not expose the new post: {body}"
    );

    // 提供元が戻ると同じ常駐workerの次の巡回が、未処理投稿を安全に取り込む。
    stack.set_author_provider_available(true).await?;
    assert!(
        stack
            .wait_for_projection(blocked.as_str(), HOLD_TIMEOUT)
            .await?,
        "the post did not reach the projection after the provider recovered"
    );
    let response = client
        .get(&search_url)
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json::<serde_json::Value>().await?;
    assert!(
        E2eStack::entry_ids(&body).contains(&blocked),
        "the recovered provider must make the post searchable: {body}"
    );

    stack.shutdown().await
}
