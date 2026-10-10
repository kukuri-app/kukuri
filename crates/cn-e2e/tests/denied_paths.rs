//! #616 T5: 不許可経路。安全側へ倒す不変条件を全構成で固定する。
//!
//! - 合成した不許可文章（視覚言語モデルの模擬が高スコア分類を返す）が索引の全面へ出ない
//! - 既知一致・重大判定（Arachnid 模擬の合成応答）のメディア投稿が索引へ入らない
//! - 投影にだけ残留した項目（真実源に無い）が検索一致として返らない
//! - 判定が許可から不許可へ変わった項目が読み取り面から消える
//!
//! いずれも wiremock の合成応答で表現し、実在の違法メディアは一切使わない。
//! 決定論化のため canary（無害な投稿）を最後に流し、それが索引に入った時点で
//! 「不許可側が入っていない」ことを確定させる。

use std::time::Duration;

use anyhow::Result;
use kukuri_cn_core::{IndexScopeKind, inspect_index_integrity};
use kukuri_cn_e2e::{E2eStack, arachnid_scanned_media_body};
use kukuri_cn_indexer::projection::{IndexProjection, IndexedEntry};
use reqwest::{Client, StatusCode};

const TINY_JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46];
const DENIED_MARKER: &str = "e2e-denied-text-marker";
const PROJECTION_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test(flavor = "multi_thread")]
async fn disallowed_content_never_surfaces_and_verdict_flip_hides_entries() -> Result<()> {
    let Some(mut stack) = E2eStack::boot("denied").await? else {
        return Ok(());
    };

    // 不許可文章: 目印を含む走査要求には高スコア分類を返す（許可へ落ちない合成応答）。
    stack
        .mount_vlm_response_for_marker(
            DENIED_MARKER,
            r#"{"categories":[{"category":"csam","score":0.97}],"tags":[]}"#,
        )
        .await;
    // 既知一致・重大判定: image/jpeg のメディア走査には合成の既知一致を返す。
    stack
        .mount_arachnid_response_for_content_type(
            "image/jpeg",
            arachnid_scanned_media_body("csam", Some("exact")),
        )
        .await;

    let denied_text = stack
        .publish_text_post(&format!("この投稿は索引に入らない {DENIED_MARKER}"))
        .await?;
    let (denied_image, _) = stack
        .publish_image_post("既知一致の合成メディア", TINY_JPEG, "image/jpeg")
        .await?;

    // 投影残留（真実源に無い項目）を実 ArcadeDB へ直接置く。
    let ghost_id = format!("ghost-{}", stack.topic_id);
    stack
        .projection
        .upsert_entry(&IndexedEntry {
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: stack.topic_id.clone(),
            object_id: ghost_id.clone(),
            author_pubkey: "ghost-author".to_string(),
            text: "tokio ghost residue".to_string(),
            created_at: 1_700_000_000,
            source_replica_id: format!("topic::{}", stack.topic_id),
            content_advisories: Vec::new(),
        })
        .await?;

    // canary: 最後に流した無害な投稿が索引に入った時点で上記の非表出が確定する。
    let canary = stack
        .publish_text_post("e2e-canary tokio runtime notes")
        .await?;
    assert!(
        stack
            .wait_for_projection(canary.as_str(), PROJECTION_TIMEOUT)
            .await?,
        "canary post did not reach the projection"
    );

    // 不許可側は投影へ入らない。
    for object_id in [denied_text.as_str(), denied_image.as_str()] {
        assert!(
            !stack
                .projection
                .contains_object(
                    IndexScopeKind::PublicTopic,
                    stack.topic_id.as_str(),
                    object_id
                )
                .await?,
            "denied object {object_id} must not reach the projection"
        );
    }

    // 真実源: 索引されたのは canary のみ。
    let findings = inspect_index_integrity(&stack.pool).await?;
    assert_eq!(findings.index_entries_total, 1, "{findings:?}");

    // 読み取り面: 発見（scope 全列挙）に canary だけが出る（不許可・投影残留は出ない）。
    let client = Client::new();
    let token = stack.authenticate(&client).await?;
    let discovery_path = format!(
        "/v1/index/discovery?scope_kind=public_topic&scope_id={}",
        stack.topic_id
    );
    let response = client
        .get(format!("{}{discovery_path}", stack.api_base_url))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json::<serde_json::Value>().await?;
    assert_eq!(
        E2eStack::entry_ids(&body),
        vec![canary.clone()],
        "only the canary may surface: {body}"
    );

    // 検索の突合: 投影残留（ghost）は text が一致しても返らない。
    let search = client
        .get(format!(
            "{}/v1/index/search?scope_kind=public_topic&scope_id={}&q=tokio",
            stack.api_base_url, stack.topic_id
        ))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(search.status(), StatusCode::OK);
    let body = search.json::<serde_json::Value>().await?;
    let ids = E2eStack::entry_ids(&body);
    assert!(ids.contains(&canary), "canary must be searchable: {body}");
    assert!(
        !ids.contains(&ghost_id),
        "projection-only residue must not surface: {body}"
    );

    // 判定が許可から不許可へ変わったとき非表示になる: 視覚言語モデルの模擬応答を
    // canary の本文に対して高スコア分類へ切り替え、常駐ワーカーの再走査（定期の
    // 全件見直し）が索引解除するまで待つ。
    stack
        .mount_vlm_response_for_marker(
            "e2e-canary",
            r#"{"categories":[{"category":"csam","score":0.97}],"tags":[]}"#,
        )
        .await;
    // #1050: 内容と scan 構成が同じ間は保存済み verdict を再利用し、provider 応答を変えた
    // だけでは再走査されない。運用と同じく scan 構成（閾値）を変えて起動し直し、次の
    // 全件見直しで再走査させる。
    stack
        .restart_worker_with_suspected_threshold(Some(50))
        .await?;
    let deadline = tokio::time::Instant::now() + PROJECTION_TIMEOUT;
    let mut hidden = false;
    while tokio::time::Instant::now() < deadline {
        let response = client
            .get(format!("{}{discovery_path}", stack.api_base_url))
            .bearer_auth(token.as_str())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.json::<serde_json::Value>().await?;
        if E2eStack::entry_ids(&body).is_empty() {
            hidden = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(hidden, "flipped verdict must hide the entry");

    // 読み取り面の非表示は即時だが、真実源の索引行の削除は再走査の巡内でわずかに遅れる
    // （判定更新 → 行削除）ため、削除完了までは有界に待つ。
    let deadline = tokio::time::Instant::now() + PROJECTION_TIMEOUT;
    let mut deindexed = false;
    while tokio::time::Instant::now() < deadline {
        if inspect_index_integrity(&stack.pool)
            .await?
            .index_entries_total
            == 0
        {
            deindexed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(
        deindexed,
        "flipped entry must be removed from the index truth"
    );

    stack.shutdown().await
}
