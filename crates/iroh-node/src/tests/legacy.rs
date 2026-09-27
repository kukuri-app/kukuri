//! #1221 R5-I: 旧 iroh store を読むだけに開き、本人の entry と pin を新しい store へ有界なページで写す。

use std::collections::BTreeMap;

use iroh_docs::store::Query;
use iroh_docs::{Capability, NamespaceSecret};
use tempfile::tempdir;

use crate::{IrohDocsNode, LegacyStore, remove_dir_step};

async fn value(node: &IrohDocsNode, doc: &iroh_docs::api::Doc, key: &str) -> Option<Vec<u8>> {
    let entry = doc.get_one(Query::key_exact(key)).await.unwrap()?;
    Some(
        node.blobs()
            .blobs()
            .get_bytes(entry.content_hash())
            .await
            .unwrap()
            .to_vec(),
    )
}

/// 本人の entry だけを 1 回 128 件以内で写し、他の docs author の entry と消した key は写さない。新しい store に既にある
/// key(切替の後の書込み)は上書きしない。旧 store を開き直しても、保存した位置から続ける。
#[tokio::test]
async fn own_entries_move_in_bounded_pages_and_resume_from_the_position() {
    let dir = tempdir().unwrap();
    let legacy_root = dir.path().join("kukuri.iroh-data");
    let legacy = IrohDocsNode::persistent(&legacy_root).await.unwrap();
    let own = legacy.docs().author_create().await.unwrap();
    let other = legacy.docs().author_create().await.unwrap();
    let busy = NamespaceSecret::from_bytes(&[1; 32]);
    let small = NamespaceSecret::from_bytes(&[2; 32]);
    let mut expected = BTreeMap::new();
    let doc = legacy
        .docs()
        .import_namespace(Capability::Write(busy.clone()))
        .await
        .unwrap();
    for index in 0..300 {
        let key = format!("k{index:03}");
        doc.set_bytes(own, key.clone(), format!("own {index}"))
            .await
            .unwrap();
        expected.insert((busy.id(), key), format!("own {index}").into_bytes());
    }
    for index in 0..50 {
        doc.set_bytes(other, format!("other/{index}"), "other")
            .await
            .unwrap();
    }
    doc.set_bytes(own, "deleted", "gone").await.unwrap();
    doc.del(own, "deleted").await.unwrap();
    doc.close().await.unwrap();
    let doc = legacy
        .docs()
        .import_namespace(Capability::Write(small.clone()))
        .await
        .unwrap();
    for index in 0..5 {
        let key = format!("s{index}");
        doc.set_bytes(own, key.clone(), "small").await.unwrap();
        expected.insert((small.id(), key), b"small".to_vec());
    }
    doc.close().await.unwrap();
    // 同期で入った他人の entry: 手元に鍵の無い docs author。
    legacy.docs().author_delete(other).await.unwrap();
    let own_secret = legacy.docs().author_export(own).await.unwrap().unwrap();
    legacy.shutdown().await.unwrap();

    let node = IrohDocsNode::persistent(dir.path().join("kukuri.iroh-store"))
        .await
        .unwrap();
    // 切替の後に新しい store へ書いた値は、旧 store の値で戻さない。
    node.docs().author_import(own_secret).await.unwrap();
    let doc = node
        .docs()
        .import_namespace(Capability::Write(busy.clone()))
        .await
        .unwrap();
    doc.set_bytes(own, "k005", "newer").await.unwrap();
    doc.close().await.unwrap();
    expected.insert((busy.id(), "k005".into()), b"newer".to_vec());

    let mut cursor = String::new();
    let mut steps = 0;
    loop {
        // 1 ステップごとに開き直す(再起動しても位置から続く)。
        let store = LegacyStore::open(&legacy_root).await.unwrap().unwrap();
        let (next, done) = store.copy_own_entries(&node, &cursor, 128).await.unwrap();
        store.close().await.unwrap();
        steps += 1;
        cursor = next;
        if done {
            break;
        }
        assert!(steps < 10, "copy does not converge");
    }
    assert!(steps >= 3, "300 own entries need at least three pages");
    for secret in [&busy, &small] {
        let doc = node
            .docs()
            .import_namespace(Capability::Write(secret.clone()))
            .await
            .unwrap();
        let stream = doc.get_many(Query::all().build()).await.unwrap();
        tokio::pin!(stream);
        let mut keys = 0;
        while let Some(entry) = futures_util::StreamExt::next(&mut stream).await {
            let entry = entry.unwrap();
            assert_eq!(entry.author(), own, "only own entries are copied");
            keys += 1;
        }
        for ((namespace, key), bytes) in &expected {
            if *namespace == secret.id() {
                assert_eq!(value(&node, &doc, key).await.as_ref(), Some(bytes), "{key}");
            }
        }
        assert_eq!(
            keys,
            expected
                .keys()
                .filter(|(namespace, _)| *namespace == secret.id())
                .count()
        );
        assert!(value(&node, &doc, "deleted").await.is_none());
        doc.close().await.unwrap();
    }
    node.shutdown().await.unwrap();
}

/// pin の tag と blob を名前順に上限つきで写す。
#[tokio::test]
async fn pin_tags_move_with_their_blobs() {
    let dir = tempdir().unwrap();
    let legacy_root = dir.path().join("kukuri.iroh-data");
    let legacy = IrohDocsNode::persistent(&legacy_root).await.unwrap();
    let mut hashes = Vec::new();
    for index in 0..3 {
        let hash = legacy
            .blobs()
            .blobs()
            .add_bytes(format!("pinned {index}").into_bytes())
            .await
            .unwrap()
            .hash;
        legacy
            .blobs()
            .tags()
            .set(format!("kukuri/metaverse/pin/{hash}"), hash)
            .await
            .unwrap();
        hashes.push(hash);
    }
    legacy.shutdown().await.unwrap();
    let node = IrohDocsNode::persistent(dir.path().join("kukuri.iroh-store"))
        .await
        .unwrap();
    let store = LegacyStore::open(&legacy_root).await.unwrap().unwrap();
    let staging = dir.path().join("pin.tmp");
    let (cursor, done) = store
        .copy_tags(&node, "kukuri/metaverse/pin/", "", 2, &staging)
        .await
        .unwrap();
    assert!(!done);
    let (_, done) = store
        .copy_tags(&node, "kukuri/metaverse/pin/", &cursor, 2, &staging)
        .await
        .unwrap();
    assert!(done);
    store.close().await.unwrap();
    for hash in hashes {
        assert!(node.blobs().blobs().has(hash).await.unwrap());
        assert!(
            node.blobs()
                .tags()
                .get(format!("kukuri/metaverse/pin/{hash}"))
                .await
                .unwrap()
                .is_some()
        );
    }
    node.shutdown().await.unwrap();
}

/// 名前を変えた旧 root を、1 回に消す file と directory を上限つきで消していき、最後に root も消す。
#[test]
fn a_retired_directory_is_removed_in_bounded_steps() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("kukuri.iroh-data.retiring");
    for sub in 0..5 {
        let path = root.join(format!("data/{sub}"));
        std::fs::create_dir_all(&path).unwrap();
        for file in 0..60 {
            std::fs::write(path.join(format!("{file}.bin")), b"x").unwrap();
        }
    }
    std::fs::write(root.join("docs.redb"), b"x").unwrap();
    let count = |path: &std::path::Path| {
        fn walk(path: &std::path::Path) -> usize {
            std::fs::read_dir(path)
                .map(|entries| {
                    entries
                        .map(|entry| {
                            let entry = entry.unwrap();
                            1 + if entry.file_type().unwrap().is_dir() {
                                walk(&entry.path())
                            } else {
                                0
                            }
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(path)
    };
    let mut steps = 0;
    loop {
        let before = count(&root);
        let done = remove_dir_step(&root, 128).unwrap();
        steps += 1;
        if done {
            break;
        }
        assert!(
            before - count(&root) <= 128,
            "one step removes at most 128 entries"
        );
    }
    assert!(!root.exists());
    assert!(steps >= 3, "307 entries need at least three steps");
    assert!(remove_dir_step(&root, 128).unwrap());
}
