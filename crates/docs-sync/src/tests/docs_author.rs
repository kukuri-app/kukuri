//! アカウントの署名鍵から導出した docs author(ADR 0053、Issue #1258)。実 iroh-docs で確かめる。

use kukuri_core::KukuriKeys;
use kukuri_iroh_node::IrohDocsNode;

use crate::{
    DocFetchPolicy, DocKeyOrder, DocKeyQuery, DocOp, DocQuery, DocsSync, IrohDocsSync,
    MemoryDocsSync, topic_replica_id,
};

const ACCOUNT_SECRET: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const OTHER_ACCOUNT_SECRET: &str =
    "101112131415161718191a1b1c1d1e1f000102030405060708090a0b0c0d0e0f";

fn keys(secret: &str) -> KukuriKeys {
    KukuriKeys::parse(secret).expect("test key")
}

async fn docs_for(secret: &str) -> (std::sync::Arc<IrohDocsNode>, IrohDocsSync, String) {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = IrohDocsSync::new(node.clone());
    let id = docs
        .use_account_docs_author(&keys(secret).derive_docs_author_seed())
        .await
        .expect("use the account docs author");
    (node, docs, id)
}

// TR-1 / TR-2 / AC-2: 同じアカウント鍵からは、別の保存場所(別の端末に相当)でも同じ docs author になる。
#[tokio::test]
async fn account_docs_author_is_the_same_on_every_device_of_the_account() {
    let (node_a, docs_a, id_a) = docs_for(ACCOUNT_SECRET).await;
    let (node_b, docs_b, id_b) = docs_for(ACCOUNT_SECRET).await;
    let (node_c, docs_c, id_c) = docs_for(OTHER_ACCOUNT_SECRET).await;

    assert_eq!(id_a, id_b);
    assert_ne!(id_a, id_c);
    // 導出の手順か context が変わると、全アカウントの docs author の id が変わる(ADR 0053 §1)。
    assert_eq!(
        id_a, "8b634e9d4977cbafafd1381b4f97e3f97c12c21937557d3d74c6a5e4952a4348",
        "the docs author id of a known account key changed"
    );
    assert_eq!(
        docs_a.local_docs_author().await.expect("local docs author"),
        Some(id_a.clone())
    );
    assert_eq!(
        node_a
            .docs()
            .author_default()
            .await
            .expect("default author")
            .to_string(),
        id_a,
        "writes must use the derived docs author"
    );
    // 設定し直しても同じ結果になる。
    assert_eq!(
        docs_a
            .use_account_docs_author(&keys(ACCOUNT_SECRET).derive_docs_author_seed())
            .await
            .expect("idempotent"),
        id_a
    );

    for (docs, node) in [(docs_a, node_a), (docs_b, node_b), (docs_c, node_c)] {
        docs.shutdown().await;
        node.shutdown().await.expect("shutdown node");
    }
}

#[tokio::test]
async fn docs_without_an_account_docs_author_report_none() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = IrohDocsSync::new(node.clone());
    assert_eq!(docs.local_docs_author().await.expect("none"), None);
    assert_eq!(
        MemoryDocsSync::default()
            .local_docs_author()
            .await
            .expect("none"),
        None
    );
    docs.shutdown().await;
    node.shutdown().await.expect("shutdown node");
}

// AC-5: docs author と key の組の読み出しは、同じ key の他の名義の entry の数に影響されない。
#[tokio::test]
async fn read_by_docs_author_returns_one_record_among_many_authors() {
    let (node, docs, id) = docs_for(ACCOUNT_SECRET).await;
    let replica = topic_replica_id("kukuri:topic:docs-author-read");
    let key = "withdrawals/shared/state";
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: key.into(),
            value: b"written by the account".to_vec(),
        },
    )
    .await
    .expect("write as the account");
    let doc = docs.ensure_replica(&replica).await.expect("replica");
    let mut first_other = None;
    for index in 0..40u8 {
        let other = node.docs().author_create().await.expect("other author");
        first_other.get_or_insert(other);
        doc.set_bytes(other, key.as_bytes().to_vec(), vec![b'x', index])
            .await
            .expect("write the same key as another docs author");
    }

    let record = docs
        .query_replica_by_author(&replica, id.as_str(), key, DocFetchPolicy::LocalOnly)
        .await
        .expect("read by docs author")
        .expect("the account's record");
    assert_eq!(record.value, b"written by the account");
    assert_eq!(record.docs_author.as_deref(), Some(id.as_str()));
    assert_eq!(record.key, key);

    // 他の名義を指定すれば、その名義の record が返る。手がかりが偽でも、読めるのはその名義の 1 件だけ。
    let other = first_other.expect("other author").to_string();
    let other_record = docs
        .query_replica_by_author(&replica, other.as_str(), key, DocFetchPolicy::LocalOnly)
        .await
        .expect("read by another docs author")
        .expect("the other record");
    assert_eq!(other_record.value, vec![b'x', 0]);
    assert_eq!(other_record.docs_author.as_deref(), Some(other.as_str()));

    // 存在しない組、docs author の id として読めない値、key を延長した別の key は「無い」。
    let unknown = "ab".repeat(32);
    for (author, key) in [
        (unknown.as_str(), key),
        ("not a docs author id", key),
        (id.as_str(), "withdrawals/shared/stat"),
        (id.as_str(), "withdrawals/shared/state-longer"),
    ] {
        assert!(
            docs.query_replica_by_author(&replica, author, key, DocFetchPolicy::LocalOnly)
                .await
                .expect("missing pair")
                .is_none(),
            "{author} / {key}"
        );
    }

    // key だけの読み出しと key の entry は、書いた docs author を運ぶ。
    let all = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact(key.into()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("exact");
    assert_eq!(all.len(), 41);
    assert!(
        all.iter()
            .any(|record| record.docs_author.as_deref() == Some(id.as_str()))
    );
    let entries = docs
        .query_replica_keys(
            &replica,
            DocKeyQuery {
                prefix: "withdrawals/".into(),
                order: DocKeyOrder::Ascending,
                limit: 100,
            },
        )
        .await
        .expect("keys");
    assert!(entries.iter().all(|entry| entry.docs_author.is_some()));

    docs.shutdown().await;
    node.shutdown().await.expect("shutdown node");
}

// ADR 0053 §1: 切り替え前の名義の entry は、同じ key を書き直すときと prefix を消すときに消す。
#[tokio::test]
async fn entries_of_the_previous_docs_author_are_superseded() {
    let node = IrohDocsNode::memory().await.expect("docs node");
    let docs = IrohDocsSync::new(node.clone());
    let replica = topic_replica_id("kukuri:topic:docs-author-legacy");
    for key in ["profile/state", "sessions/live/a/state", "untouched/state"] {
        docs.apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: key.into(),
                value: b"old".to_vec(),
            },
        )
        .await
        .expect("write as the device docs author");
    }
    let legacy = node
        .docs()
        .author_default()
        .await
        .expect("legacy author")
        .to_string();
    let id = docs
        .use_account_docs_author(&keys(ACCOUNT_SECRET).derive_docs_author_seed())
        .await
        .expect("switch");
    assert_ne!(legacy, id);

    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: "profile/state".into(),
            value: b"new".to_vec(),
        },
    )
    .await
    .expect("rewrite");
    let rewritten = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("profile/state".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("exact");
    assert_eq!(
        rewritten
            .iter()
            .map(|record| (record.value.clone(), record.docs_author.clone()))
            .collect::<Vec<_>>(),
        vec![(b"new".to_vec(), Some(id.clone()))],
        "a reader that takes the first record of the key must not see the old value"
    );

    docs.apply_doc_op(
        &replica,
        DocOp::DeletePrefix {
            prefix: "sessions/".into(),
        },
    )
    .await
    .expect("delete");
    assert!(
        docs.query_replica_with_policy(
            &replica,
            DocQuery::Prefix("sessions/".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("prefix")
        .is_empty(),
        "entries written by the previous docs author must be deletable"
    );

    // 書き直していない key の旧 record は、そのまま読める。
    let untouched = docs
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("untouched/state".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("exact");
    assert_eq!(untouched.len(), 1);
    assert_eq!(untouched[0].docs_author.as_deref(), Some(legacy.as_str()));

    docs.shutdown().await;
    node.shutdown().await.expect("shutdown node");
}

#[tokio::test]
async fn memory_docs_with_a_docs_author_supports_reads_by_docs_author() {
    let docs = MemoryDocsSync::with_docs_author("aa".repeat(32));
    let replica = topic_replica_id("kukuri:topic:docs-author-memory");
    docs.apply_doc_op(
        &replica,
        DocOp::SetBytes {
            key: "objects/x/envelope".into(),
            value: b"value".to_vec(),
        },
    )
    .await
    .expect("write");
    let author = "aa".repeat(32);
    assert_eq!(
        docs.local_docs_author().await.expect("author"),
        Some(author.clone())
    );
    let record = docs
        .query_replica_by_author(
            &replica,
            author.as_str(),
            "objects/x/envelope",
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read")
        .expect("record");
    assert_eq!(record.docs_author.as_deref(), Some(author.as_str()));
    assert!(
        docs.query_replica_by_author(
            &replica,
            "bb".repeat(32).as_str(),
            "objects/x/envelope",
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("read")
        .is_none()
    );
    // docs author を持たない docs は、この読み出しを「無い」と答える(エラーにしない)。
    assert!(
        MemoryDocsSync::default()
            .query_replica_by_author(
                &replica,
                author.as_str(),
                "objects/x/envelope",
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("read")
            .is_none()
    );
}
