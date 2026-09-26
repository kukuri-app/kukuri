//! #616 T6: 復旧の E2E。
//!
//! - 常駐ワーカー再起動後も対象範囲と取り込みが復元される
//! - 投影（ArcadeDB）を空にしても、真実源とレプリカから再構築される
//! - プロバイダ停止で保留になった投稿が、復旧後の再走査で索引に入る
//!
//! （以前の image と環境変数への切り戻しは実機検証 T7 の記録項目。ここでは
//! プロセス内で再現できる復旧経路を固定する）

use std::time::Duration;

use anyhow::Result;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_e2e::E2eStack;
use kukuri_cn_indexer::projection::IndexProjection;

const PROJECTION_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test(flavor = "multi_thread")]
async fn worker_restart_projection_rebuild_and_provider_recovery() -> Result<()> {
    let Some(mut stack) = E2eStack::boot("recover").await? else {
        return Ok(());
    };

    // 前提: 通常の取り込みが 1 件通っている。
    let first = stack.publish_text_post("再起動前の投稿").await?;
    assert!(
        stack
            .wait_for_projection(first.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "first post did not reach the projection"
    );

    // --- 復旧 1: ワーカー再起動後も対象範囲と取り込みが復元される ---
    stack.restart_worker().await?;
    let second = stack.publish_text_post("再起動後の投稿").await?;
    assert!(
        stack
            .wait_for_projection(second.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "post after worker restart did not reach the projection (scope must be restored)"
    );

    // --- 復旧 2: 投影を空にしても真実源とレプリカから再構築される ---
    stack
        .projection
        .remove_scope_page(IndexScopeKind::PublicTopic, stack.topic_id.as_str(), 128)
        .await?;
    assert!(
        stack
            .wait_for_projection(first.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "projection was not rebuilt after being emptied"
    );
    assert!(
        stack
            .wait_for_projection(second.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "projection rebuild must cover all posts"
    );

    // --- 復旧 3: プロバイダ停止で保留になった投稿が、復旧後の再走査で索引に入る ---
    stack.vlm.reset().await;
    let held = stack.publish_text_post("停止中に届いた投稿").await?;
    assert!(
        stack
            .wait_for_state(|state| state.skipped_non_allow >= 1, PROJECTION_TIMEOUT)
            .await?,
        "post during the outage must be held"
    );
    assert!(
        !stack
            .projection
            .contains_object(
                IndexScopeKind::PublicTopic,
                stack.topic_id.as_str(),
                held.as_str()
            )
            .await?,
        "held post must not surface during the outage"
    );

    stack.restore_default_provider_mocks().await;
    assert!(
        stack
            .wait_for_projection(held.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "held post must be indexed after the provider recovers"
    );

    stack.shutdown().await
}
