//! #925: observable I/O and durable results before source resolver extraction.
#[path = "source_resolution_contracts/support.rs"]
mod support;

use anyhow::Result;
use kukuri_cn_core::IndexScopeKind;
use kukuri_cn_indexer::projection::IndexProjection;
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_core::{
    AssetRef, AssetRole, KukuriKeys, KukuriMediaManifestV1, MediaManifestItem, PayloadRef,
    PostWithdrawalReason, TopicId, WithdrawalReasonVisibility, blob_hash,
    build_media_manifest_envelope, build_post_envelope, build_post_withdrawal_envelope,
};
use kukuri_docs_sync::{DocOp, DocsSync};
use support::*;

#[tokio::test]
async fn unverified_blob_metadata_has_zero_fetch_scan_and_index_upserts() -> Result<()> {
    for invalid in [
        "missing",
        "malformed",
        "signature",
        "id",
        "author",
        "payload",
        "oversized",
    ] {
        let f = Fixture::new(IndexScopeKind::PublicTopic).await?;
        let mut post = if invalid == "oversized" {
            f.post_with(
                PayloadRef::BlobText {
                    hash: blob_hash(b"body"),
                    mime: "text/markdown".into(),
                    bytes: 40_001,
                },
                Vec::new(),
                Vec::new(),
            )?
        } else {
            f.post(b"body")?
        };
        match invalid {
            "signature" => post.envelope.content = "tampered".into(),
            "id" => post.envelope = f.post(b"different signed object")?.envelope,
            "author" => post.state["author"] = serde_json::json!("d".repeat(64)),
            "payload" => {
                post.state["payload_ref"] = serde_json::to_value(PayloadRef::BlobText {
                    hash: blob_hash(b"body"),
                    mime: "text/plain".into(),
                    bytes: 4,
                })?
            }
            _ => {}
        }
        f.persist(&post).await?;
        let envelope_key = format!("objects/{}/envelope", post.id);
        if invalid == "missing" {
            f.docs
                .apply_doc_op(
                    &f.replica,
                    DocOp::DeletePrefix {
                        prefix: envelope_key,
                    },
                )
                .await?;
        } else if invalid == "malformed" {
            f.docs
                .apply_doc_op(
                    &f.replica,
                    DocOp::SetBytes {
                        key: envelope_key,
                        value: b"not-json".to_vec(),
                    },
                )
                .await?;
        }
        let summary = f.ingest().await?;
        assert_eq!(
            (summary.scanned, summary.indexed, summary.skipped_non_allow),
            (1, 0, 1),
            "{invalid}"
        );
        let io = f.observed();
        assert_no_durable_blob_io(&io);
        assert!(io.ephemeral.is_empty(), "{invalid}: unverified body fetch");
        assert!(io.scans.is_empty(), "{invalid}: provider request");
        assert!(
            io.entry_upserts.is_empty() && io.projection_upserts.is_empty(),
            "{invalid}: index write"
        );
        assert_eq!(io.entry_removes, vec![post.id.clone()]);
        assert_eq!(io.projection_removes, vec![post.id.clone()]);
        assert!(!f.entries.inner.contains(f.kind, SCOPE, &post.id));
        assert_eq!(f.projection.count_scope(f.kind, SCOPE).await?, 0);
    }
    Ok(())
}

#[tokio::test]
async fn rejected_blob_bytes_use_only_ephemeral_fetch_and_never_reach_a_provider() -> Result<()> {
    for invalid in [
        "missing",
        "unavailable",
        "size",
        "hash",
        "utf8",
        "characters",
    ] {
        let f = Fixture::new(IndexScopeKind::PublicTopic).await?;
        let bytes = match invalid {
            "utf8" => vec![0xff],
            "characters" => vec![b'x'; 10_001],
            _ => b"body".to_vec(),
        };
        let post = f.post(&bytes)?;
        f.persist(&post).await?;
        if !matches!(invalid, "utf8" | "characters") {
            assert_eq!(f.ingest().await?.indexed, 1);
            assert!(f.entries.inner.contains(f.kind, SCOPE, &post.id));
            f.clear_io();
        }
        let hash = blob_hash(&bytes);
        let reply = match invalid {
            "missing" => Reply::Missing,
            "unavailable" => Reply::Failed,
            "size" => Reply::Bytes(b"too long".to_vec()),
            "hash" => Reply::Bytes(b"evil".to_vec()),
            _ => Reply::Bytes(bytes),
        };
        f.blobs.respond(&hash, reply);
        let summary = f.ingest().await?;
        assert_eq!(
            (summary.indexed, summary.skipped_non_allow),
            (0, 1),
            "{invalid}"
        );
        let io = f.observed();
        assert_eq!(io.ephemeral, vec![hash.as_str().to_string()]);
        assert_no_durable_blob_io(&io);
        assert!(
            io.scans.is_empty(),
            "{invalid}: provider must not see rejected bytes"
        );
        assert!(io.entry_upserts.is_empty() && io.projection_upserts.is_empty());
        if matches!(invalid, "missing" | "unavailable") {
            // 取得できないことは一時的な失敗で、索引済み entry を保持する（#1090）。
            assert!(io.entry_removes.is_empty(), "{invalid}: truth removed");
            assert!(
                io.projection_removes.is_empty(),
                "{invalid}: projection removed"
            );
            assert!(f.entries.inner.contains(f.kind, SCOPE, &post.id));
            assert_eq!(f.projection.count_scope(f.kind, SCOPE).await?, 1);
        } else {
            assert_eq!(io.entry_removes, vec![post.id.clone()]);
            assert_eq!(io.projection_removes, vec![post.id.clone()]);
            assert!(!f.entries.inner.contains(f.kind, SCOPE, &post.id));
            assert_eq!(f.projection.count_scope(f.kind, SCOPE).await?, 0);
        }
    }
    Ok(())
}

#[tokio::test]
async fn unicode_body_at_the_character_limit_remains_scoped_and_ephemeral() -> Result<()> {
    for kind in [IndexScopeKind::PublicTopic, IndexScopeKind::PrivateChannel] {
        let f = Fixture::new(kind).await?;
        let body = "界".repeat(10_000);
        let post = f.post(body.as_bytes())?;
        f.persist(&post).await?;
        assert_eq!(f.ingest().await?.indexed, 1);
        let io = f.observed();
        assert_no_durable_blob_io(&io);
        assert_eq!(
            io.ephemeral,
            vec![blob_hash(body.as_bytes()).as_str().to_string()]
        );
        assert_eq!(io.scans.len(), 1);
        assert_eq!(io.scans[0].subject_id.as_deref(), Some(post.id.as_str()));
        assert_eq!(io.scans[0].text.as_deref(), Some(body.as_str()));
        assert_eq!(io.entry_upserts, vec![post.id.clone()]);
        assert_eq!(io.projection_upserts, vec![post.id.clone()]);
        let entries = f.projection.inner.entries_in_scope(kind, SCOPE).await;
        assert_eq!(entries[0].text, body);
        assert_eq!(entries[0].scope_kind, kind);
        let other = if kind == IndexScopeKind::PublicTopic {
            IndexScopeKind::PrivateChannel
        } else {
            IndexScopeKind::PublicTopic
        };
        assert_eq!(f.projection.count_scope(other, SCOPE).await?, 0);
    }
    Ok(())
}

fn media_item(
    hash: kukuri_core::BlobHash,
    mime: &str,
    thumbnail: Option<kukuri_core::BlobHash>,
) -> MediaManifestItem {
    MediaManifestItem {
        blob_hash: hash,
        mime: mime.into(),
        size: 4,
        width: None,
        height: None,
        duration_ms: None,
        codec: None,
        thumbnail_blob_hash: thumbnail,
    }
}

#[tokio::test]
async fn invalid_media_manifest_never_sends_media_to_the_provider_or_upserts_indexes() -> Result<()>
{
    for invalid in ["missing", "signature", "kind", "author", "malformed"] {
        let f = Fixture::new(IndexScopeKind::PublicTopic).await?;
        let post = f.post_with(
            PayloadRef::InlineText {
                text: "caption".into(),
            },
            Vec::new(),
            vec!["manifest".into()],
        )?;
        f.persist(&post).await?;
        let manifest = KukuriMediaManifestV1 {
            manifest_id: "manifest".into(),
            owner_pubkey: post.keys.public_key(),
            created_at: 1,
            items: vec![media_item(blob_hash(b"media"), "image/png", None)],
        };
        let topic = TopicId::new(SCOPE);
        let mut envelope = if invalid == "author" {
            let other = KukuriKeys::generate();
            build_media_manifest_envelope(
                &other,
                &topic,
                &KukuriMediaManifestV1 {
                    owner_pubkey: other.public_key(),
                    ..manifest
                },
            )?
        } else if invalid == "kind" {
            build_post_envelope(&post.keys, &topic, "wrong kind", None)?
        } else {
            build_media_manifest_envelope(&post.keys, &topic, &manifest)?
        };
        if invalid == "signature" {
            envelope.content = "tampered".into();
        }
        let key = "manifests/media/manifest/envelope";
        if invalid == "malformed" {
            f.docs
                .apply_doc_op(
                    &f.replica,
                    DocOp::SetBytes {
                        key: key.into(),
                        value: b"not-json".to_vec(),
                    },
                )
                .await?;
        } else if invalid != "missing" {
            f.set(key, serde_json::to_value(envelope)?).await?;
        }
        let summary = f.ingest().await?;
        assert_eq!(
            (summary.indexed, summary.skipped_non_allow),
            (0, 1),
            "{invalid}"
        );
        let io = f.observed();
        assert_no_durable_blob_io(&io);
        assert!(io.ephemeral.is_empty());
        assert_eq!(
            io.scans.len(),
            1,
            "only the already-allowed post text is scanned"
        );
        assert_eq!(io.scans[0].subject_kind, Some(SubjectKind::Post));
        assert!(io.entry_upserts.is_empty() && io.projection_upserts.is_empty());
        assert_eq!(io.entry_removes, vec![post.id.clone()]);
        assert_eq!(io.projection_removes, vec![post.id]);
    }
    Ok(())
}

#[tokio::test]
async fn duplicate_media_hashes_preserve_first_mime_including_a_thumbnail_without_mime()
-> Result<()> {
    let f = Fixture::new(IndexScopeKind::PublicTopic).await?;
    let a = blob_hash(b"attachment");
    let b = blob_hash(b"manifest item");
    let c = blob_hash(b"thumbnail");
    let post = f.post_with(
        PayloadRef::InlineText {
            text: "caption".into(),
        },
        vec![AssetRef {
            hash: a.clone(),
            mime: " image/png ".into(),
            bytes: 4,
            role: AssetRole::Attachment,
        }],
        vec!["manifest".into()],
    )?;
    f.persist(&post).await?;
    let manifest = KukuriMediaManifestV1 {
        manifest_id: "manifest".into(),
        owner_pubkey: post.keys.public_key(),
        created_at: 1,
        items: vec![
            media_item(a.clone(), "image/jpeg", None),
            media_item(b.clone(), "image/webp", Some(c.clone())),
            media_item(c.clone(), "image/png", Some(a.clone())),
        ],
    };
    let signed = build_media_manifest_envelope(&post.keys, &TopicId::new(SCOPE), &manifest)?;
    f.set(
        "manifests/media/manifest/envelope",
        serde_json::to_value(signed)?,
    )
    .await?;
    assert_eq!(f.ingest().await?.indexed, 1);
    let io = f.observed();
    assert_no_durable_blob_io(&io);
    let media: Vec<_> = io
        .scans
        .iter()
        .filter(|request| request.subject_kind == Some(SubjectKind::Blob))
        .map(|request| {
            (
                request.subject_id.clone().expect("media scan subject id"),
                request.media_mime.clone(),
            )
        })
        .collect();
    assert_eq!(
        media,
        vec![
            (a.as_str().into(), Some("image/png".into())),
            (b.as_str().into(), Some("image/webp".into())),
            (c.as_str().into(), None)
        ]
    );
    assert_eq!(io.scans.len(), 4);
    assert_eq!(io.entry_upserts, vec![post.id.clone()]);
    assert_eq!(io.projection_upserts, vec![post.id]);
    Ok(())
}

#[tokio::test]
async fn mixed_scope_reingest_never_fetches_withdrawn_prevented_deleted_or_invalid_members()
-> Result<()> {
    let f = Fixture::new(IndexScopeKind::PublicTopic).await?;
    let valid = f.post(b"valid body")?;
    let withdrawn = f.post(b"withdrawn body")?;
    let mut deleted = f.post(b"deleted body")?;
    let mut tombstone = f.post(b"tombstone body")?;
    let prevented = f.post(b"prevented body")?;
    let mut invalid = f.post(b"invalid body")?;
    for post in [
        &valid, &withdrawn, &deleted, &tombstone, &prevented, &invalid,
    ] {
        f.persist(post).await?;
    }
    assert_eq!(f.ingest().await?.indexed, 6);
    let withdrawal = build_post_withdrawal_envelope(
        &withdrawn.keys,
        &withdrawn.envelope,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )?;
    f.set(
        &format!("withdrawals/{}/state", withdrawn.id),
        serde_json::to_value(withdrawal)?,
    )
    .await?;
    deleted.state["status"] = serde_json::json!("deleted");
    f.persist(&deleted).await?;
    tombstone.state["status"] = serde_json::json!("tombstoned");
    f.persist(&tombstone).await?;
    f.entries.inner.prevent_subject(prevented.id.clone());
    invalid.envelope.content = "tampered".into();
    f.persist(&invalid).await?;
    let mut removed = vec![
        withdrawn.id,
        deleted.id,
        tombstone.id,
        prevented.id,
        invalid.id,
    ];
    removed.sort();
    for _ in 0..2 {
        f.clear_io();
        let summary = f.ingest().await?;
        assert_eq!(
            (
                summary.scanned,
                summary.indexed,
                summary.skipped_non_allow,
                summary.deindexed
            ),
            (6, 1, 1, 4)
        );
        let mut io = f.observed();
        assert_no_durable_blob_io(&io);
        assert_eq!(
            io.ephemeral,
            vec![blob_hash(b"valid body").as_str().to_string()]
        );
        // #1050: 内容と scan 構成が不変の valid post は保存済み verdict を再利用し、provider には
        // 届かない（初回 ingest で 1 回 scan 済み）。本文の一時取得は再利用判定より前に行う。
        assert!(
            io.scans.is_empty(),
            "unchanged valid post must not reach the provider on re-ingest"
        );
        assert_eq!(summary.scans_reused, 1);
        assert_eq!(summary.scans_fresh, 0);
        assert_eq!(io.entry_upserts, vec![valid.id.clone()]);
        assert_eq!(io.projection_upserts, vec![valid.id.clone()]);
        io.entry_removes.sort();
        io.projection_removes.sort();
        assert_eq!(io.entry_removes, removed);
        assert_eq!(io.projection_removes, removed);
        let entries = f.projection.inner.entries_in_scope(f.kind, SCOPE).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].object_id, valid.id);
        for id in &removed {
            assert!(!f.entries.inner.contains(f.kind, SCOPE, id));
        }
    }
    Ok(())
}
