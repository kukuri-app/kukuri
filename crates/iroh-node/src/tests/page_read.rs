use anyhow::Result;
use iroh_docs::{Capability, NamespaceSecret};
use kukuri_core::ReplicaId;
use kukuri_store::SqliteStore;
use std::sync::Arc;

use crate::{DocReadQuery, DocReadRecord, DocReadResponse, IrohDocsNode};

#[tokio::test]
async fn private_bucket_page_requires_the_epoch_capability() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let replica = ReplicaId::new("bucket::v1::channel::6368::6570::1");
    let secret = NamespaceSecret::from_bytes(&[9; 32]);
    let doc = provider
        .docs()
        .import_namespace(Capability::Write(secret.clone()))
        .await?;
    doc.set_bytes(
        provider.docs().author_default().await?,
        b"indexes/timeline/0001/private".to_vec(),
        b"private".to_vec(),
    )
    .await?;
    let query = DocReadQuery::Keys {
        prefix: "indexes/timeline/".into(),
        descending: true,
        limit: 1,
        author: None,
    };
    let response = requester
        .query_remote_docs(provider.endpoint().addr(), &replica, &secret, query.clone())
        .await?;
    let DocReadResponse::Keys { entries, .. } = response else {
        anyhow::bail!("expected private page")
    };
    assert_eq!(entries[0].key, "indexes/timeline/0001/private");
    assert!(
        requester
            .query_remote_docs(
                provider.endpoint().addr(),
                &replica,
                &NamespaceSecret::from_bytes(&[8; 32]),
                query,
            )
            .await
            .is_err()
    );
    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn legacy_topic_and_private_epoch_use_the_same_bounded_reader() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let public = ReplicaId::new("topic::kukuri:topic:rust");
    let public_secret = NamespaceSecret::from_bytes(
        blake3::hash(format!("kukuri-docs:{}", public.as_str()).as_bytes()).as_bytes(),
    );
    let private = ReplicaId::new("channel::room::epoch::e1");
    let private_secret = NamespaceSecret::from_bytes(&[42; 32]);
    let mut docs = Vec::new();
    for (secret, body) in [
        (&public_secret, b"public".as_slice()),
        (&private_secret, b"private".as_slice()),
    ] {
        let doc = provider
            .docs()
            .import_namespace(Capability::Write(secret.clone()))
            .await?;
        doc.set_bytes(
            provider.docs().author_default().await?,
            b"objects/post/envelope".to_vec(),
            body.to_vec(),
        )
        .await?;
        docs.push(doc);
    }
    let before = docs[0].status().await?;
    for (replica, secret, body) in [
        (&public, &public_secret, b"public".as_slice()),
        (&private, &private_secret, b"private".as_slice()),
    ] {
        let response = requester
            .query_remote_docs(
                provider.endpoint().addr(),
                replica,
                secret,
                DocReadQuery::Exact {
                    key: "objects/post/envelope".into(),
                    limit: 1,
                    author: None,
                },
            )
            .await?;
        let DocReadResponse::Records(records) = response else {
            anyhow::bail!("expected record")
        };
        assert_eq!(records[0].value, body);
    }
    assert!(
        requester
            .query_remote_docs(
                provider.endpoint().addr(),
                &private,
                &NamespaceSecret::from_bytes(&[41; 32]),
                DocReadQuery::Exact {
                    key: "objects/post/envelope".into(),
                    limit: 1,
                    author: None
                },
            )
            .await
            .is_err()
    );
    let after = docs[0].status().await?;
    assert_eq!(after.handles, before.handles);
    assert_eq!(after.sync, before.sync);
    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn cached_remote_record_is_reprovided_without_local_namespace() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let cache = Arc::new(SqliteStore::connect_memory().await?);
    provider.install_remote_cache(cache.clone())?;
    let replica = ReplicaId::new("bucket::v1::topic::72757374::1");
    let secret = NamespaceSecret::from_bytes(
        blake3::hash(format!("kukuri-docs:{}", replica.as_str()).as_bytes()).as_bytes(),
    );
    let record = DocReadRecord {
        key: "objects/post-1/envelope".into(),
        value: b"signed envelope".to_vec(),
        content_hash: iroh_blobs::Hash::new(b"signed envelope").to_string(),
        content_len: 15,
        docs_author: provider.docs().author_default().await?.to_string(),
    };
    anyhow::ensure!(
        cache
            .put_remote_record(
                replica.as_str(),
                &record.key,
                &record.docs_author,
                &serde_json::to_vec(&record)?,
            )
            .await?,
        "fixture must fit"
    );
    let response = requester
        .query_remote_docs(
            provider.endpoint().addr(),
            &replica,
            &secret,
            DocReadQuery::Exact {
                key: record.key.clone(),
                limit: 1,
                author: Some(record.docs_author.clone()),
            },
        )
        .await?;
    let DocReadResponse::Records(records) = response else {
        anyhow::bail!("expected records")
    };
    assert_eq!(records[0].value, record.value);
    provider.shutdown().await?;
    requester.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn real_peer_returns_only_requested_local_keys_and_records() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let replica = ReplicaId::new("bucket::v1::topic::72757374::1");
    let secret = NamespaceSecret::from_bytes(
        blake3::hash(format!("kukuri-docs:{}", replica.as_str()).as_bytes()).as_bytes(),
    );
    let doc = provider
        .docs()
        .import_namespace(Capability::Write(secret.clone()))
        .await?;
    let author = provider.docs().author_default().await?;
    doc.set_bytes(
        author,
        b"indexes/timeline/0001/a".to_vec(),
        b"first".to_vec(),
    )
    .await?;
    doc.set_bytes(
        author,
        b"indexes/timeline/0002/b".to_vec(),
        b"second".to_vec(),
    )
    .await?;
    doc.set_bytes(author, b"other/key".to_vec(), b"unrelated".to_vec())
        .await?;
    let before = doc.status().await?;

    let peer = provider.endpoint().addr();
    let page = requester
        .query_remote_docs(
            peer.clone(),
            &replica,
            &secret,
            DocReadQuery::Keys {
                prefix: "indexes/timeline/".into(),
                descending: true,
                limit: 1,
                author: None,
            },
        )
        .await?;
    let DocReadResponse::Keys {
        entries,
        reached_limit,
    } = page
    else {
        anyhow::bail!("expected index page")
    };
    assert!(reached_limit);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "indexes/timeline/0002/b");

    let record = requester
        .query_remote_docs(
            peer.clone(),
            &replica,
            &secret,
            DocReadQuery::Exact {
                key: entries[0].key.clone(),
                limit: 8,
                author: None,
            },
        )
        .await?;
    let DocReadResponse::Records(records) = record else {
        anyhow::bail!("expected exact records")
    };
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].value, b"second");
    doc.set_bytes(author, b"indexes/timeline/\xff".to_vec(), b"junk".to_vec())
        .await?;
    let malformed = requester
        .query_remote_docs(
            peer.clone(),
            &replica,
            &secret,
            DocReadQuery::Keys {
                prefix: "indexes/timeline/".into(),
                descending: true,
                limit: 1,
                author: None,
            },
        )
        .await?;
    let DocReadResponse::Keys {
        entries: malformed_entries,
        reached_limit,
    } = malformed
    else {
        anyhow::bail!("expected bounded key page")
    };
    assert!(malformed_entries.is_empty());
    assert!(
        reached_limit,
        "skipped keys still count toward the page boundary"
    );
    for index in 0..8u8 {
        let author = provider.docs().author_create().await?;
        doc.set_bytes(author, b"bulk/key".to_vec(), vec![index; 64 * 1024])
            .await?;
    }
    let bulk = requester
        .query_remote_docs(
            peer.clone(),
            &replica,
            &secret,
            DocReadQuery::Exact {
                key: "bulk/key".into(),
                limit: 8,
                author: None,
            },
        )
        .await?;
    let DocReadResponse::Records(bulk) = bulk else {
        anyhow::bail!("expected bounded records")
    };
    assert_eq!(bulk.len(), 8);
    assert!(bulk.iter().all(|record| record.value.len() == 64 * 1024));
    assert!(
        requester
            .query_remote_docs(
                peer.clone(),
                &ReplicaId::new("channel::private"),
                &NamespaceSecret::from_bytes(&[8; 32]),
                DocReadQuery::Exact {
                    key: "bulk/key".into(),
                    limit: 1,
                    author: None,
                },
            )
            .await
            .is_err(),
        "the page protocol must not serve a private replica without its capability"
    );
    assert!(
        requester
            .query_remote_docs(
                peer,
                &replica,
                &NamespaceSecret::from_bytes(&[72; 32]),
                DocReadQuery::Exact {
                    key: entries[0].key.clone(),
                    limit: 8,
                    author: None,
                },
            )
            .await
            .is_err()
    );
    let after = doc.status().await?;
    assert_eq!(
        after.handles, before.handles,
        "remote reads must not open a handle"
    );
    assert_eq!(
        after.sync, before.sync,
        "remote reads must not start docs sync"
    );

    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}

// #1221 R5-C: author replica も同じ有界な reader で読め、docs author を指定した key の一覧は他の名義の entry を返さない。
#[tokio::test]
async fn author_replica_keys_are_read_by_the_requested_docs_author() -> Result<()> {
    let provider = IrohDocsNode::memory().await?;
    let requester = IrohDocsNode::memory().await?;
    let replica = ReplicaId::new(format!("author::{}", "a".repeat(64)));
    let secret = NamespaceSecret::from_bytes(
        blake3::hash(format!("kukuri-docs:{}", replica.as_str()).as_bytes()).as_bytes(),
    );
    let doc = provider
        .docs()
        .import_namespace(Capability::Write(secret.clone()))
        .await?;
    let writer = provider.docs().author_create().await?;
    let other = provider.docs().author_create().await?;
    doc.set_bytes(
        writer,
        b"indexes/profile/0002/mine".to_vec(),
        b"mine".to_vec(),
    )
    .await?;
    doc.set_bytes(
        other,
        b"indexes/profile/0003/other".to_vec(),
        b"other".to_vec(),
    )
    .await?;
    let page = |author: Option<String>| DocReadQuery::Keys {
        prefix: "indexes/profile/".into(),
        descending: true,
        limit: 1,
        author,
    };
    let DocReadResponse::Keys { entries, .. } = requester
        .query_remote_docs(provider.endpoint().addr(), &replica, &secret, page(None))
        .await?
    else {
        anyhow::bail!("expected a key page")
    };
    assert_eq!(entries[0].key, "indexes/profile/0003/other");
    let DocReadResponse::Keys { entries, .. } = requester
        .query_remote_docs(
            provider.endpoint().addr(),
            &replica,
            &secret,
            page(Some(writer.to_string())),
        )
        .await?
    else {
        anyhow::bail!("expected a key page")
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "indexes/profile/0002/mine");
    assert_eq!(entries[0].docs_author, writer.to_string());
    requester.shutdown().await?;
    provider.shutdown().await?;
    Ok(())
}
