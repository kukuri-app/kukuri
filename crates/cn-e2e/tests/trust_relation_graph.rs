//! #616 T6: 信頼・申し立て・関係の E2E。
//!
//! - 危険信号（UserPubkey 対象。本番と同じ永続化関数で発行）が信頼値の絶対・相対成分へ
//!   入り、閲覧者に固定された read（viewer = bearer）で読める
//! - 申し立ての係争中は寄与が据え置かれ、認容で寄与が戻り、訂正信号が読み取り面に反映される
//!   （申し立て受理の HTTP 経路は cn-user-api の contract test が固定済みのため、ここでは
//!   handler と同じ遷移関数で状態を進める）
//! - 関係: 公開トピックの投稿どうしの双方向のアクション → 解析後に近接度と近傍が更新される。プライベート
//!   チャンネルだけの参加はグラフへ入らない。distance opt-out は近距離の関係を維持し、解除可能
//!
//! 関係解析は `cn-cli relation analyze` と同じ経路（`relation_actions` の差分 +
//! `ArcadeDbRelationGraph` + `analyze_relations`）を同一プロセスで実行する。取込みがアクションを保存する経路は
//! cn-indexer の `relation_ingest_contracts` が固定する（#1221 R5-E）。

use std::time::Duration;

use anyhow::Result;
use kukuri_cn_core::{IndexEntryStore, RiskSignalCorrection};
use kukuri_cn_core::{
    IndexScopeKind, NewIndexEntry, RelationAction, RelationActionKind, dispute_risk_signal,
    list_risk_signals_for_target, persist_risk_signal, record_relation_action,
    reissue_corrected_risk_signal, update_risk_signal_appeal_status, upsert_scan_verdict,
};
use kukuri_cn_e2e::E2eStack;
use kukuri_cn_indexer::{ArcadeDbConfig, ArcadeDbRelationGraph, analyze_relations};
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{
    AppealStatus, Basis, ReasonCode, RiskSignalTarget, SafetyAction, SafetyCategory,
    SafetyRiskSignal, SafetyVerdict, Severity, Visibility,
};
use kukuri_cn_safety_runtime::VerdictPersistMeta;
use kukuri_core::generate_keys;
use reqwest::{Client, StatusCode};

const PROJECTION_TIMEOUT: Duration = Duration::from_secs(120);

fn user_risk_signal(
    target_pubkey: &str,
    category: SafetyCategory,
    severity: Severity,
    basis: Basis,
    visibility: Visibility,
) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target: RiskSignalTarget::UserPubkey,
        target_id: target_pubkey.to_string(),
        category,
        severity,
        basis,
        confidence: Some(100),
        visibility,
        expires_at: None,
        appeal_status: None,
    }
}

fn allow_verdict() -> SafetyVerdict {
    SafetyVerdict {
        action: SafetyAction::Allow,
        labels: Vec::new(),
        advisory_labels: Vec::new(),
        critical: false,
        reason_code: ReasonCode::NoKnownMatch,
        confidence: None,
        provider: Some("e2e-seed".to_string()),
        provider_capability: None,
        policy_version: "policy-e2e".to_string(),
        scanned_at: "2026-08-06T00:00:00Z".to_string(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn trust_appeal_and_relation_graph_work_end_to_end() -> Result<()> {
    let Some(stack) = E2eStack::boot("graph").await? else {
        return Ok(());
    };
    let client = Client::new();

    // --- 関係: 公開トピックの双方向のアクション → 解析 → 近接度・近傍 ---
    let author_a = stack.author.keys.clone();
    let author_b = generate_keys();
    let post_a = stack.publish_text_post("関係の投稿（著者 A）").await?;
    let post_b = stack
        .publish_text_post_as(&author_b, "関係の投稿（著者 B）")
        .await?;
    for object_id in [post_a.as_str(), post_b.as_str()] {
        assert!(
            stack
                .wait_for_projection(object_id, PROJECTION_TIMEOUT)
                .await?,
            "relation post {object_id} did not reach the projection"
        );
    }

    // A と B が互いの投稿へ反応した（取込みが保存するのと同じ行。起点は各自の投稿）。
    for (source, actor, target) in [
        (
            post_a.as_str(),
            author_a.public_key_hex(),
            author_b.public_key_hex(),
        ),
        (
            post_b.as_str(),
            author_b.public_key_hex(),
            author_a.public_key_hex(),
        ),
    ] {
        record_relation_action(
            &stack.pool,
            &RelationAction {
                kind: RelationActionKind::Reply,
                source_id: source.to_string(),
                actor_pubkey: actor,
                target_pubkey: target,
                scope_id: Some(stack.topic_id.clone()),
                anchor_object_id: Some(source.to_string()),
                created_at: chrono::Utc::now().timestamp(),
            },
        )
        .await?;
    }

    // `cn-cli relation analyze` と同じ経路で解析する（定期解析の 1 回分）。
    let graph = ArcadeDbRelationGraph::new(ArcadeDbConfig::from_env())?;
    graph.ensure_schema().await?;
    let report = analyze_relations(&stack.pool, &graph, 100).await?;
    assert!(
        report.edges_upserted >= 1,
        "mutual actions must produce at least one edge: {report:?}"
    );

    let token_a = stack.authenticate_as(&client, &author_a).await?;
    let pubkey_b = author_b.public_key_hex();
    let relation_url = format!("{}/v1/relation/users/{pubkey_b}", stack.api_base_url);
    let relation = client
        .get(relation_url.as_str())
        .bearer_auth(token_a.as_str())
        .send()
        .await?;
    assert_eq!(relation.status(), StatusCode::OK);
    let relation_body = relation.json::<serde_json::Value>().await?;
    assert_eq!(
        relation_body["target_pubkey"].as_str(),
        Some(pubkey_b.as_str())
    );
    let neighbors = client
        .get(format!("{}/v1/relation/neighbors", stack.api_base_url))
        .bearer_auth(token_a.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let neighbor_list: Vec<&str> = neighbors["neighbors"]
        .as_array()
        .map(|list| list.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        neighbor_list.contains(&pubkey_b.as_str()),
        "author B must appear in author A's neighbors: {neighbors}"
    );

    // --- 関係: プライベートチャンネルだけの参加はグラフへ入らない ---
    let author_c = generate_keys();
    let pubkey_c = author_c.public_key_hex();
    for (author, object_id) in [
        (author_a.public_key_hex(), "private-post-a"),
        (pubkey_c.clone(), "private-post-c"),
    ] {
        let verdict = upsert_scan_verdict(
            &stack.pool,
            SubjectKind::Post,
            &format!("{}-{object_id}", stack.topic_id),
            &allow_verdict(),
            &VerdictPersistMeta::default(),
        )
        .await?;
        stack
            .entries
            .upsert_entry(&NewIndexEntry {
                scope_kind: IndexScopeKind::PrivateChannel,
                scope_id: format!("chan-{}", stack.topic_id),
                object_id: format!("{}-{object_id}", stack.topic_id),
                author_pubkey: author,
                created_at: 1_700_000_000,
                source_replica_id: format!("channel::chan-{}", stack.topic_id),
                verdict_id: verdict.id,
                verdict_action: "allow".to_string(),
                critical: false,
            })
            .await?;
    }
    analyze_relations(&stack.pool, &graph, 100).await?;
    let private_pair = client
        .get(format!(
            "{}/v1/relation/users/{pubkey_c}",
            stack.api_base_url
        ))
        .bearer_auth(token_a.as_str())
        .send()
        .await?;
    assert_eq!(
        private_pair.status(),
        StatusCode::NOT_FOUND,
        "private-channel participation must not enter the public relation graph"
    );

    // --- 関係: distance opt-out は近距離の関係を維持し、解除可能 ---
    let token_b = stack.authenticate_as(&client, &author_b).await?;
    client
        .put(format!("{}/v1/relation/optout", stack.api_base_url))
        .bearer_auth(token_b.as_str())
        .send()
        .await?
        .error_for_status()?;
    let visible_while_close = client
        .get(relation_url.as_str())
        .bearer_auth(token_a.as_str())
        .send()
        .await?;
    assert_eq!(
        visible_while_close.status(),
        StatusCode::OK,
        "distance opt-out must keep a sufficiently close relation visible"
    );
    client
        .delete(format!("{}/v1/relation/optout", stack.api_base_url))
        .bearer_auth(token_b.as_str())
        .send()
        .await?
        .error_for_status()?;
    let visible_again = client
        .get(relation_url.as_str())
        .bearer_auth(token_a.as_str())
        .send()
        .await?;
    assert_eq!(
        visible_again.status(),
        StatusCode::OK,
        "clearing distance opt-out must keep the relation visible"
    );

    // --- 信頼: 危険信号が絶対・相対成分へ入り、根拠つきで読める ---
    const ATTRIBUTED_MARKER: &str = "e2e-attributed-csam-marker";
    stack
        .mount_vlm_response_for_marker(
            ATTRIBUTED_MARKER,
            r#"{"categories":[{"category":"csam","score":0.97}],"tags":[]}"#,
        )
        .await;
    let attributed_post = stack
        .publish_text_post_as(
            &author_b,
            &format!("synthetic moderation contract marker: {ATTRIBUTED_MARKER}"),
        )
        .await?;
    let deadline = tokio::time::Instant::now() + PROJECTION_TIMEOUT;
    let attributed_signals = loop {
        let signals = list_risk_signals_for_target(
            &stack.pool,
            RiskSignalTarget::PostId,
            attributed_post.as_str(),
        )
        .await?;
        if !signals.is_empty() {
            break signals;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "scan-derived risk signal was not persisted"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    // 相対成分の代表は spam（ADR 0026 §7 以降、nsfw / objectionable は advisory-only で寄与 0）。
    let relative_signal = persist_risk_signal(
        &stack.pool,
        "e2e-issuer-node",
        &user_risk_signal(
            pubkey_b.as_str(),
            SafetyCategory::Spam,
            Severity::High,
            Basis::ClassifierScore,
            Visibility::Local,
        ),
    )
    .await?;
    let trust_url = format!("{}/v1/trust/users/{pubkey_b}", stack.api_base_url);
    let read_trust = || async {
        anyhow::Ok(
            client
                .get(trust_url.as_str())
                .bearer_auth(token_a.as_str())
                .send()
                .await?
                .error_for_status()?
                .json::<serde_json::Value>()
                .await?,
        )
    };
    let initial = read_trust().await?;
    let initial_absolute = initial["absolute"].as_f64().unwrap();
    let initial_relative = initial["relative"].as_f64().unwrap();
    assert!(
        initial_absolute < 0.0,
        "scan-derived CSAM signal must reduce the absolute component: {initial}"
    );
    assert!(
        initial_relative < 0.0,
        "classifier signal must weigh on the relative component: {initial}"
    );

    // --- 申し立て: 係争中は寄与据え置き → 認容で寄与が戻る → 訂正信号が反映される ---
    // （HTTP の申し立て受理経路は cn-user-api の contract test が固定済み。ここでは
    //   handler と同じ遷移関数で係争 → 認容を進める）
    // Stop producing duplicate scan artifacts, then prove that appeal state on
    // the original content signal controls the author's absolute trust input.
    stack.vlm.reset().await;
    stack.restore_default_provider_mocks().await;
    let mut attributed_signals = attributed_signals;
    tokio::time::sleep(Duration::from_millis(500)).await;
    attributed_signals.extend(
        list_risk_signals_for_target(
            &stack.pool,
            RiskSignalTarget::PostId,
            attributed_post.as_str(),
        )
        .await?,
    );
    attributed_signals.sort_by(|a, b| a.id.cmp(&b.id));
    attributed_signals.dedup_by(|a, b| a.id == b.id);
    for signal in &attributed_signals {
        dispute_risk_signal(&stack.pool, signal.id.as_str()).await?;
    }
    let disputed_content = read_trust().await?;
    assert_eq!(
        disputed_content["absolute"].as_f64().unwrap(),
        initial_absolute,
        "a disputed content signal must remain effective: {disputed_content}"
    );
    for signal in &attributed_signals {
        update_risk_signal_appeal_status(&stack.pool, signal.id.as_str(), AppealStatus::Cleared)
            .await?;
    }
    let cleared_content = read_trust().await?;
    assert!(
        cleared_content["absolute"].as_f64().unwrap() > initial_absolute,
        "clearing the content signal must restore the author's absolute trust: {cleared_content}"
    );

    dispute_risk_signal(&stack.pool, relative_signal.id.as_str()).await?;
    let disputed = read_trust().await?;
    assert_eq!(
        disputed["relative"].as_f64().unwrap(),
        initial_relative,
        "disputed contribution must stay in place until resolution: {disputed}"
    );

    update_risk_signal_appeal_status(
        &stack.pool,
        relative_signal.id.as_str(),
        AppealStatus::Cleared,
    )
    .await?;
    let cleared = read_trust().await?;
    assert!(
        cleared["relative"].as_f64().unwrap() > initial_relative,
        "accepted appeal must restore the relative component: {cleared}"
    );

    // 訂正信号の再発行: 旧信号を失効させ、訂正内容の新信号が根拠に現れる。
    let corrected = reissue_corrected_risk_signal(
        &stack.pool,
        relative_signal.id.as_str(),
        &RiskSignalCorrection {
            severity: Some(Severity::Low),
            confidence: Some(10),
            ..RiskSignalCorrection::default()
        },
        chrono::Utc::now().to_rfc3339().as_str(),
        true,
    )
    .await?;
    let after_correction = read_trust().await?;
    let basis_ids: Vec<&str> = after_correction["basis"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry["signal_id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        basis_ids.contains(&corrected.id.as_str()),
        "corrected signal must appear in the trust evidence: {after_correction}"
    );
    assert!(
        !basis_ids.contains(&relative_signal.id.as_str()),
        "expired original signal must no longer appear: {after_correction}"
    );

    stack.shutdown().await
}
