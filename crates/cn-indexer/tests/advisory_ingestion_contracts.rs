//! #1054: nsfw / objectionable の content advisory 付き index の contract test
//! （ADR 0028 §8.1 / §8.3 / §8.8、`ingestion_contracts.rs` から分離）。
//!
//! - `general_nsfw_is_indexed_with_advisory_label`
//! - `labeled_allow_text_does_not_short_circuit_media_scan`

use std::sync::Arc;

use anyhow::Result;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_indexer::projection::MemoryIndexProjection;
use kukuri_cn_indexer::query::{FailClosedIndexQuery, IndexQuery};
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{
    AdvisorySubjectKind, Basis, GeneralAction, MockSafetyProvider, ModerationAction,
    RiskSignalTarget, SafetyCategory, SafetyPolicy, SafetyProvider, SafetyProviderCapability,
    Severity,
};
use kukuri_cn_safety_runtime::MemorySafetyArtifactStore;
use kukuri_core::TopicId;
use kukuri_docs_sync::{MemoryDocsSync, topic_replica_id};

mod ingest_support;
use ingest_support::*;
// --- #1054: nsfw / objectionable の content advisory 付き index（ADR 0028 §8.1 / §8.3） ---

/// ADR 0028 §8.11 contract: `general_nsfw_is_indexed_with_advisory_label`（indexer 面）。
#[tokio::test]
async fn general_nsfw_is_indexed_with_advisory_label() -> Result<()> {
    // media blob が nsfw suspected（95）でも、既定 policy（general_action = label）では post は
    // index され、post verdict 行には blob_cid の advisory が入り、query 境界が content_advisories を
    // 同梱する。派生タグも index される（§8.8）。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_media_post(&docs, &replica, &topic, "media caption", true).await;

    let providers = || {
        vec![
            MockSafetyProvider::known_csam("mock-known-csam"),
            MockSafetyProvider::with_capabilities(
                "mock-vlm",
                vec![SafetyProviderCapability::GeneralMediaModeration],
            )
            .with_score(
                media_blob_hash(),
                SafetyProviderCapability::GeneralMediaModeration,
                SafetyCategory::Nsfw,
                95,
            )
            .with_derived_tags(media_blob_hash(), vec!["beach".to_string()]),
        ]
    };
    let (pipeline, entries, store) =
        pipeline_with(&docs, &projection, service_with_providers(providers()));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(summary.indexed, 1);
    assert_eq!(summary.skipped_non_allow, 0);
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));

    // blob verdict はラベル付き allow。post 行にはその advisory が和集合として入る。
    let blob = store
        .stored_verdict_for(SubjectKind::Blob, &media_blob_hash())
        .expect("blob verdict");
    assert!(blob.verdict.is_labeled_allow());
    assert_eq!(blob.advisories.len(), 1);
    let advisory = &blob.advisories[0];
    assert_eq!(advisory.subject_kind, AdvisorySubjectKind::BlobCid);
    assert_eq!(advisory.subject_id, media_blob_hash());
    assert_eq!(advisory.category, SafetyCategory::Nsfw);
    assert_eq!(advisory.label, "adult");
    assert_eq!(advisory.confidence, Some(95));
    assert_eq!(advisory.basis, Basis::ClassifierScore);
    let post = store
        .stored_verdict_for(SubjectKind::Post, &object_id)
        .expect("post verdict");
    assert!(post.verdict.is_indexable());
    assert!(!post.verdict.is_labeled_allow(), "本文 text 自体は clean");
    assert_eq!(post.advisories, blob.advisories);

    // risk signal は Low / classifier_score で 1 件、advisory はその id を指す。event は RiskLabel。
    let signals = store.signals_with_ids();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].2.severity, Severity::Low);
    assert_eq!(signals[0].2.category, SafetyCategory::Nsfw);
    assert_eq!(signals[0].2.target, RiskSignalTarget::BlobCid);
    assert_eq!(advisory.signal_id, signals[0].0);
    assert_eq!(advisory.issuer_node_id, signals[0].1);
    assert_eq!(store.events().len(), 1);
    assert_eq!(store.events()[0].body.action, ModerationAction::RiskLabel);

    // query 境界: 真実源の最新 verdict から content_advisories を同梱する。投影には書かない。
    let query = FailClosedIndexQuery::new(projection.clone(), entries.clone());
    let hits = query
        .search_scope(IndexScopeKind::PublicTopic, "rust", "beach", 10)
        .await?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].object_id, object_id);
    assert_eq!(hits[0].content_advisories, blob.advisories);
    let recent = query.list_recent(None, 10).await?;
    assert_eq!(recent[0].content_advisories, blob.advisories);
    let stored = projection
        .entries_in_scope(IndexScopeKind::PublicTopic, "rust")
        .await;
    assert!(
        stored[0].content_advisories.is_empty(),
        "投影は advisory を持たない"
    );

    // 2 巡目（内容・構成不変）: provider 呼び出しなし、signal / event 増えず、advisory 不変（TR-10）。
    let second = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(second.indexed, 1);
    assert_eq!(second.scans_fresh, 0);
    assert!(second.scans_reused >= 2, "post text + media blob を再利用");
    assert_eq!(store.signals().len(), 1);
    assert_eq!(store.events().len(), 1);
    let hits = query
        .search_scope(IndexScopeKind::PublicTopic, "rust", "beach", 10)
        .await?;
    assert_eq!(hits[0].content_advisories, blob.advisories);

    // operator が general_action = exclude に厳格化した node では従来どおり非 index。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let object_id = persist_media_post(&docs, &replica, &topic, "media caption", true).await;
    let strict_store = Arc::new(MemorySafetyArtifactStore::new());
    let strict = service_with_store(
        strict_store.clone(),
        SafetyPolicy {
            general_action: GeneralAction::Exclude,
            ..SafetyPolicy::public_node_default()
        },
        providers()
            .into_iter()
            .map(|provider| Arc::new(provider) as Arc<dyn SafetyProvider>)
            .collect(),
    );
    let (pipeline, entries, _) = pipeline_with(&docs, &projection, (strict, strict_store.clone()));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    let blob = strict_store
        .stored_verdict_for(SubjectKind::Blob, &media_blob_hash())
        .expect("blob verdict");
    assert!(!blob.verdict.is_indexable());
    assert!(blob.advisories.is_empty());
    Ok(())
}

/// ADR 0028 §8.11 contract: `labeled_allow_text_does_not_short_circuit_media_scan`。
#[tokio::test]
async fn labeled_allow_text_does_not_short_circuit_media_scan() -> Result<()> {
    // (a) 本文 text がラベル付き allow でも media scan を実行し、media が known CSAM なら
    //     post は index しない（worst-case 合成）。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let topic = TopicId::new("rust");
    let replica = topic_replica_id("rust");
    let object_id = persist_media_post(&docs, &replica, &topic, "spicy caption", true).await;
    let known =
        MockSafetyProvider::known_csam("mock-known-csam").with_known_hash_match(media_blob_hash());
    let vlm = MockSafetyProvider::with_capabilities(
        "mock-vlm",
        vec![SafetyProviderCapability::GeneralMediaModeration],
    )
    .with_score(
        object_id.clone(),
        SafetyProviderCapability::GeneralMediaModeration,
        SafetyCategory::Nsfw,
        90,
    );
    let (pipeline, entries, store) =
        pipeline_with(&docs, &projection, service_with_providers(vec![known, vlm]));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.skipped_non_allow, 1);
    assert!(!entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    let post = store
        .stored_verdict_for(SubjectKind::Post, &object_id)
        .expect("post verdict");
    assert!(
        post.verdict.is_labeled_allow(),
        "text はラベル付き allow として記録"
    );
    let blob = store
        .stored_verdict_for(SubjectKind::Blob, &media_blob_hash())
        .expect("media scan が実行され verdict が記録される（短絡しない）");
    assert!(blob.verdict.critical);
    assert!(!blob.verdict.is_indexable());

    // (b) media も nsfw / objectionable なら index され、advisory は text + blob の和集合。
    let docs = Arc::new(MemoryDocsSync::default());
    let projection = Arc::new(MemoryIndexProjection::new());
    let object_id = persist_media_post(&docs, &replica, &topic, "spicy caption", true).await;
    let known = MockSafetyProvider::known_csam("mock-known-csam");
    let vlm = MockSafetyProvider::with_capabilities(
        "mock-vlm",
        vec![SafetyProviderCapability::GeneralMediaModeration],
    )
    .with_score(
        object_id.clone(),
        SafetyProviderCapability::GeneralMediaModeration,
        SafetyCategory::Nsfw,
        90,
    )
    .with_score(
        media_blob_hash(),
        SafetyProviderCapability::GeneralMediaModeration,
        SafetyCategory::Objectionable,
        85,
    );
    let (pipeline, entries, store) =
        pipeline_with(&docs, &projection, service_with_providers(vec![known, vlm]));
    let summary = pipeline
        .ingest_recent_scope(IndexScopeKind::PublicTopic, "rust", &replica)
        .await?;
    assert_eq!(summary.indexed, 1);
    assert!(entries.contains(IndexScopeKind::PublicTopic, "rust", &object_id));
    let post = store
        .stored_verdict_for(SubjectKind::Post, &object_id)
        .expect("post verdict");
    assert!(post.verdict.is_labeled_allow());
    let kinds: Vec<(AdvisorySubjectKind, String, SafetyCategory, String)> = post
        .advisories
        .iter()
        .map(|advisory| {
            (
                advisory.subject_kind,
                advisory.subject_id.clone(),
                advisory.category,
                advisory.label.clone(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                AdvisorySubjectKind::PostId,
                object_id.clone(),
                SafetyCategory::Nsfw,
                "adult".to_string()
            ),
            (
                AdvisorySubjectKind::BlobCid,
                media_blob_hash(),
                SafetyCategory::Objectionable,
                "sensitive".to_string()
            ),
        ]
    );
    // signal は text / blob それぞれ 1 件（Low）、advisory は各 signal を指す。
    let signals = store.signals_with_ids();
    assert_eq!(signals.len(), 2);
    assert!(
        signals
            .iter()
            .all(|(_, _, signal)| signal.severity == Severity::Low)
    );
    for advisory in &post.advisories {
        assert!(signals.iter().any(|(id, _, _)| id == &advisory.signal_id));
    }
    let query = FailClosedIndexQuery::new(projection.clone(), entries.clone());
    let hits = query
        .search_scope(IndexScopeKind::PublicTopic, "rust", "spicy", 10)
        .await?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].content_advisories, post.advisories);
    Ok(())
}
