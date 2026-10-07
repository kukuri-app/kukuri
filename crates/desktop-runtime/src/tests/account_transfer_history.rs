//! #1211 AC-3（3b〜3f）: 移行の任意の投稿の履歴。移行元の page が読む範囲と量、実 runtime の往復での保存・反映・
//! 取得不能・契約の維持、移行先の置き場の再開と回収。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::account_transfer::{eventually, runtime_at, target_host, transfer_with};
use super::*;
use crate::accounts::account_db_path;
use crate::accounts::history::{self, PAGE_QUERIES};
use crate::accounts::transfer::TransferSink;
use crate::requests::{
    PostWithdrawalReasonRequest, WithdrawPostRequest, WithdrawalReasonVisibilityRequest,
};
use kukuri_core::{
    AccountHistoryCursor, AccountHistoryRecord, AccountTransferFailure, AccountTransferHistory,
    AccountTransferHistoryResult, AccountTransferStatus, BlobHash, EnvelopeId, KukuriEnvelope,
    KukuriKeys, ObjectVisibility, PayloadRef, ReplicaId, TopicId, blob_hash,
    build_post_envelope_with_docs_author,
};
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};
use kukuri_iroh_node::{AccountBundleSink, DocReadRecord};
use kukuri_store::{ObjectProjectionStore, SqliteStore};

const SINCE: u64 = 20_000;
const DOCS_AUTHOR: &str = "docs-author";
/// 履歴の送り元の端末（endpoint id）。
const PEER: &str = "source-device";

fn payload(key: &str) -> Vec<u8> {
    let value = key.as_bytes().to_vec();
    serde_json::to_vec(&DocReadRecord {
        key: key.to_string(),
        content_hash: blob_hash(&value).0,
        content_len: value.len() as u64,
        value,
        docs_author: DOCS_AUTHOR.into(),
    })
    .unwrap()
}

fn bucket(scope: BucketScope, day: u64) -> String {
    let bucket = TimeBucket::from_index(day).unwrap();
    let replica = BucketReplica::new(scope, bucket).unwrap().replica_id();
    replica.as_str().to_string()
}

fn topic() -> BucketScope {
    BucketScope::Topic {
        topic_id: "history-topic".into(),
    }
}

fn author(pubkey: &str) -> BucketScope {
    BucketScope::Author {
        author_pubkey: pubkey.into(),
    }
}

async fn own(store: &SqliteStore, replica: &str, key: &str) {
    store
        .put_owned_record(replica, key, DOCS_AUTHOR, &payload(key))
        .await
        .unwrap();
}

/// 移行先の置き場の file（`<db>.account-history*`）。
fn history_files(db: &Path) -> Vec<String> {
    let name = db.file_stem().unwrap().to_string_lossy().to_string();
    let mut files: Vec<_> = fs::read_dir(db.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|file| file.starts_with(&format!("{name}.account-history")))
        .collect();
    files.sort();
    files
}

/// 範囲の page を最後まで読む。page ごとの（照会の数・読んだ行・record の数・bytes）と、読んだ record を返す。
async fn read_pages(
    store: &SqliteStore,
    account: &str,
    since: Option<u64>,
) -> (
    Vec<(usize, usize, usize, usize)>,
    BTreeSet<(String, String)>,
) {
    let (mut pages, mut keys, mut cursor) = (Vec::new(), BTreeSet::new(), None);
    loop {
        let (calls, rows) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let read = |reference, after: AccountHistoryCursor, limit| {
            calls.fetch_add(1, Ordering::SeqCst);
            let rows = &rows;
            async move {
                let found = store
                    .protected_records_after(reference, &after, limit)
                    .await?;
                rows.fetch_add(found.len(), Ordering::SeqCst);
                Ok(found)
            }
        };
        let page = history::page(read, account, since, cursor).await.unwrap();
        let bytes = page.records.iter().map(|record| record.value.len()).sum();
        let count = (calls.into_inner(), rows.into_inner());
        pages.push((count.0, count.1, page.records.len(), bytes));
        keys.extend(
            page.records
                .iter()
                .map(|record| (record.replica.clone(), record.key.clone())),
        );
        match page.next {
            Some(next) => cursor = Some(next),
            None => return (pages, keys),
        }
    }
}

/// 3b: 期間を選んだ page は、範囲の bucket の自分の record だけを返し、範囲の外は seek で飛ばす。選択外の履歴（範囲
/// より前の bucket・旧形式・account・device・他人の author bucket）を 10 倍にしても、page ごとの照会の数・読む行・
/// record の数・bytes は変わらない。
#[tokio::test]
async fn history_pages_read_the_same_whatever_the_unselected_history() {
    let account = KukuriKeys::generate().public_key_hex();
    let channel = BucketScope::PrivateChannel {
        channel_id: "c".into(),
        epoch_id: "e".into(),
    };
    let mut runs = Vec::new();
    for scale in [1, 10] {
        let store = SqliteStore::connect_memory().await.unwrap();
        for day in 0..3 {
            for i in 0..30 {
                let key = format!("indexes/timeline/{i:02}");
                own(&store, &bucket(topic(), SINCE + day), &key).await;
            }
        }
        for i in 0..5 {
            let key = format!("objects/{i}/envelope");
            own(&store, &bucket(author(&account), SINCE + 1), &key).await;
            own(&store, &bucket(channel.clone(), SINCE + 2), &key).await;
        }
        // 選択外は、どれも 1 回の照会の上限（64 件）より多い（読む行は上限で決まる）。
        for i in 0..70 * scale {
            let old = format!("old/{i:04}");
            own(&store, &bucket(topic(), SINCE - 1 - (i % 30)), &old).await;
            own(&store, &bucket(author(&account), SINCE - 1), &old).await;
            own(&store, &bucket(author("other"), SINCE), &old).await;
            own(&store, "topic::legacy", &old).await;
            own(&store, "account::v1::0a", &old).await;
            own(&store, &format!("device::{account}::d"), &old).await;
        }
        let (pages, keys) = read_pages(&store, &account, Some(SINCE)).await;
        assert_eq!(keys.len(), 100);
        assert!(
            pages
                .iter()
                .all(|(calls, _, records, _)| *calls <= PAGE_QUERIES && *records <= 64),
            "{pages:?}"
        );
        runs.push((pages, keys));
    }
    assert_eq!(runs[0], runs[1]);
}

/// 3b: すべてを選んだ page は、時間 bucket に加えて旧形式（topic・channel・自分の author 領域と、旧形式の移行で守った
/// 参照 `own:<id>`）も返す。account・device の replica と他人の author 領域は返さない。期間は時間 bucket だけ。
#[tokio::test]
async fn all_history_includes_the_legacy_records_but_not_other_replicas() {
    let account = KukuriKeys::generate().public_key_hex();
    let store = SqliteStore::connect_memory().await.unwrap();
    let taken = [
        bucket(topic(), SINCE),
        "topic::legacy".to_string(),
        "channel::legacy".to_string(),
        format!("author::{account}"),
    ];
    let skipped = [
        "account::v1::0a".to_string(),
        format!("device::{account}::d"),
        "author::other".to_string(),
        bucket(author("other"), SINCE),
    ];
    for replica in taken.iter().chain(&skipped) {
        own(&store, replica, "objects/a/envelope").await;
    }
    let legacy = ("topic::legacy", "objects/old/envelope", "legacy");
    let bytes = payload(legacy.1);
    let put = store.put_remote_record(legacy.0, legacy.1, legacy.2, &bytes);
    assert!(put.await.unwrap());
    let key = SqliteStore::remote_record_cache_key(legacy.0, legacy.1, legacy.2);
    store
        .add_protected_ref("own:old", "record", &key)
        .await
        .unwrap();

    let (_, all) = read_pages(&store, &account, None).await;
    let mut expected: BTreeSet<_> = taken
        .iter()
        .map(|replica| (replica.clone(), "objects/a/envelope".to_string()))
        .collect();
    expected.insert((legacy.0.into(), legacy.1.into()));
    assert_eq!(all, expected);
    let (_, recent) = read_pages(&store, &account, Some(SINCE)).await;
    assert_eq!(
        recent,
        BTreeSet::from([(taken[0].clone(), "objects/a/envelope".to_string())])
    );
}

/// 移行元の範囲（`since` の bucket から）の自分の record（replica・key・docs author・値）。
pub(super) async fn own_records_since(
    store: &SqliteStore,
    since: u64,
) -> Vec<(String, String, String, Vec<u8>)> {
    let mut position = AccountHistoryCursor {
        reference: "own_docs".into(),
        ..AccountHistoryCursor::default()
    };
    let mut records = Vec::new();
    loop {
        let rows = store
            .protected_records_after("own_docs", &position, 64)
            .await
            .unwrap();
        let Some((last, _)) = rows.last() else {
            return records;
        };
        position = last.clone();
        for (at, payload) in rows {
            let in_range = BucketReplica::parse(&ReplicaId::new(at.replica.clone()))
                .is_ok_and(|replica| replica.bucket().index() >= since);
            if in_range {
                records.push((at.replica, at.key, at.author, payload));
            }
        }
    }
}

fn is_post(key: &str) -> bool {
    key.starts_with("objects/") && key.ends_with("/envelope")
}

/// 3a・3c・3e・3f: 実 runtime の往復。履歴を選ぶと、移行先は範囲の自分の record と投稿の本文・添付を置き場へ受け、
/// そのアカウントの runtime の起動で、同じ replica（bucket locator）・key・docs author・値の自分の record と blob と
/// して反映し、置き場と journal を消す。範囲より前の record は移らず、移行元に無い本文は数える。envelope の署名を確かめ
/// られ、移行元が居なくても、移行先の自分のプロフィールで本文・添付が読め、取り下げた投稿は取り下げとして読める。
#[tokio::test]
async fn a_history_transfer_reflects_the_selected_records_and_blobs() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let source = runtime_at(dir.path().join("source.db")).await;
    source.finish_protected_migration().await.unwrap();
    let topic = "kukuri:topic:account-history";
    let post = |content: &str, attachments| CreatePostRequest {
        topic: topic.into(),
        content: content.into(),
        reply_to: None,
        channel_ref: ChannelRef::Public,
        attachments,
        content_labels: Vec::new(),
    };
    let image = vec![9u8; 700 * 1024];
    let attached = image_attachment_request("p.png", "image/png", &image);
    let kept = source.create_post(post("kept", Vec::new())).await.unwrap();
    let pictured = source
        .create_post(post("pictured", vec![attached]))
        .await
        .unwrap();
    let withdrawn = source
        .create_post(post("withdrawn", Vec::new()))
        .await
        .unwrap();
    source
        .withdraw_post(WithdrawPostRequest {
            topic: topic.into(),
            object_id: withdrawn.clone(),
            channel_ref: ChannelRef::Public,
            replacement_object_id: None,
            reason_visibility: WithdrawalReasonVisibilityRequest::Public,
            reason: Some(PostWithdrawalReasonRequest::AuthorRequest),
        })
        .await
        .unwrap();
    // 範囲より前の bucket の record と、移行元に無い本文を指す投稿（取得不能の固定 fixture）。
    let now = Utc::now().timestamp();
    let today = TimeBucket::from_unix_seconds(now).unwrap().index();
    let scope = || BucketScope::Topic {
        topic_id: topic.into(),
    };
    let old = bucket(scope(), today - 100);
    let old_key = "objects/old/envelope";
    let old_payload = payload(old_key);
    let put_old = source
        .sqlite
        .put_owned_record(&old, old_key, DOCS_AUTHOR, &old_payload);
    put_old.await.unwrap();
    let missing = build_post_envelope_with_docs_author(
        &source.author_keys,
        &TopicId::new(topic),
        PayloadRef::BlobText {
            hash: BlobHash::new("f".repeat(64)),
            mime: "text/plain".into(),
            bytes: 4,
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
        None,
    )
    .unwrap();
    let missing_key = format!("objects/{}/envelope", missing.id.as_str());
    let value = serde_json::to_vec(&missing).unwrap();
    let missing_payload = serde_json::to_vec(&DocReadRecord {
        key: missing_key.clone(),
        content_hash: blob_hash(&value).0,
        content_len: value.len() as u64,
        value,
        docs_author: DOCS_AUTHOR.into(),
    })
    .unwrap();
    source
        .sqlite
        .put_owned_record(
            &bucket(scope(), today),
            &missing_key,
            DOCS_AUTHOR,
            &missing_payload,
        )
        .await
        .unwrap();
    let since = TimeBucket::from_unix_seconds(now - 30 * 86_400)
        .unwrap()
        .index();
    let expected = own_records_since(&source.sqlite, since).await;
    let posts = expected.iter().filter(|record| is_post(&record.1)).count() as u64;
    let mut blobs = Vec::new();
    for id in [&kept, &pictured, &withdrawn] {
        let row = source
            .sqlite
            .get_object_projection(&EnvelopeId::from(id.as_str()))
            .await
            .unwrap()
            .unwrap();
        if let PayloadRef::BlobText { hash, .. } = &row.payload_ref {
            blobs.push(hash.as_str().to_string());
        }
        blobs.extend(
            row.attachments
                .iter()
                .map(|asset| asset.hash.as_str().to_string()),
        );
    }

    let target_dir = dir.path().join("target");
    let host = target_host(&target_dir).await;
    let status = transfer_with(&source, &host, Some(AccountTransferHistory::Month)).await;
    let result = AccountTransferHistoryResult {
        posts,
        unavailable: 1,
        stopped: None,
    };
    let AccountTransferStatus::Completed {
        account_id: Some(id),
        history: Some(received),
        ..
    } = status
    else {
        panic!("unexpected {status:?}");
    };
    assert_eq!(received, result);
    assert!(matches!(
        source.account_transfer_status().await.unwrap(),
        AccountTransferStatus::Completed { history: Some(sent), .. } if sent == result
    ));
    let db = account_db_path(&target_dir, &id);
    assert!(!history_files(&db).is_empty());
    source.shutdown().await;

    let moved = runtime_at(&db).await;
    eventually("the history is reflected", async || {
        history_files(&db).is_empty()
    })
    .await;
    for (replica, key, author, payload) in &expected {
        let held = moved
            .sqlite
            .get_remote_records(replica, key, Some(author), 1, true)
            .await
            .unwrap();
        assert_eq!(held, std::slice::from_ref(payload), "{replica} {key}");
        if is_post(key) && *key != missing_key {
            let record: DocReadRecord = serde_json::from_slice(payload).unwrap();
            let envelope: KukuriEnvelope = serde_json::from_slice(&record.value).unwrap();
            envelope.verify().unwrap();
        }
    }
    let old_held = moved
        .sqlite
        .get_remote_records(&old, old_key, None, 8, false);
    assert!(old_held.await.unwrap().is_empty());
    for hash in &blobs {
        let content = moved.sqlite.get_remote_content("blob", hash).await.unwrap();
        assert!(content.is_some(), "{hash}");
    }
    let absent = moved
        .sqlite
        .get_remote_content("blob", &"f".repeat(64))
        .await;
    assert!(absent.unwrap().is_none());

    // 移行先の自分のプロフィールで読める（移行元が居なくても、自分の record から。取り下げは背景の確認で反映する）。
    let pubkey = moved.local_author_pubkey();
    eventually(
        "the posts are read from the transferred records",
        async || {
            let request = ListProfileTimelineRequest {
                pubkey: pubkey.clone(),
                cursor: None,
                limit: Some(20),
            };
            let Ok(timeline) = moved.list_profile_timeline(request).await else {
                return false;
            };
            let find = |id: &str| timeline.items.iter().find(|item| item.object_id == id);
            find(&kept).is_some_and(|item| item.content == "kept")
                && find(&pictured).is_some_and(|item| item.attachments.len() == 1)
                && find(&withdrawn).is_some_and(|item| item.withdrawal.is_some())
        },
    )
    .await;
    moved.shutdown().await;
    host.shutdown().await;
}

/// 3c・3d: 移行先の置き場。範囲の外の record・hash の合わない blob・揃わない blob の page は保存せず、落とされた page の
/// 部分の file は消える。同じ送り元の同じ範囲は確定した続きの位置から、別の送り元（#1650）・別の範囲は最初から始める。
/// 反映は確定した page を自分の record・blob として保存し、範囲の終わりまで反映したら置き場と journal を消す。
#[tokio::test]
async fn history_staging_resumes_and_reclaims_its_files() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let host = target_host(dir.path()).await;
    let sink = TransferSink {
        host: Arc::downgrade(&host),
    };
    let keys = KukuriKeys::generate();
    // 必須 bundle（item なし）を確定して、アカウントを登録する。
    let mut staging = sink.begin(&keys.export_secret_hex()).await.unwrap();
    let id = staging.commit().await.unwrap();
    drop(staging);
    let db = account_db_path(dir.path(), &id);
    let today = TimeBucket::from_unix_seconds(Utc::now().timestamp())
        .unwrap()
        .index();
    let record = |day: u64, key: &str| AccountHistoryRecord {
        replica: bucket(topic(), day),
        key: key.into(),
        docs_author: DOCS_AUTHOR.into(),
        value: key.as_bytes().to_vec(),
    };
    let blob = vec![5u8; 600 * 1024];
    let (hash, len, half) = (
        blake3::hash(&blob).to_hex().to_string(),
        blob.len() as u64,
        300 * 1024,
    );
    let month = AccountTransferHistory::Month;
    let invalid = Err(AccountTransferFailure::Invalid);

    let mut resume = sink.history(&id, month, PEER).await.unwrap();
    assert!(resume.cursor.is_none() && resume.since.is_some());
    let old = resume
        .staging
        .records(vec![record(today - 100, "objects/old/envelope")]);
    assert_eq!(old.await, invalid);
    let zero = "0".repeat(64);
    let forged = resume.staging.blob(&zero, 4, 0, b"abcd".to_vec());
    assert_eq!(forged.await, invalid);
    let part = blob[..half].to_vec();
    resume.staging.blob(&hash, len, 0, part).await.unwrap();
    assert_eq!(resume.staging.commit(None).await, invalid);
    drop(resume);
    eventually("the stopped page is removed", async || {
        history_files(&db) == ["kukuri.account-history.json"]
    })
    .await;

    // 1 page を確定して止める。同じ範囲は続きから、別の範囲は最初から。
    let next = AccountHistoryCursor {
        reference: "own_docs".into(),
        replica: bucket(topic(), today),
        key: "objects/a/envelope".into(),
        author: DOCS_AUTHOR.into(),
    };
    let mut resume = sink.history(&id, month, PEER).await.unwrap();
    let staging = &mut resume.staging;
    staging
        .records(vec![record(today, "objects/a/envelope")])
        .await
        .unwrap();
    let first = blob[..half].to_vec();
    staging.blob(&hash, len, 0, first).await.unwrap();
    let rest = blob[half..].to_vec();
    staging.blob(&hash, len, half as u64, rest).await.unwrap();
    staging.commit(Some(next.clone())).await.unwrap();
    drop(resume);
    let again = sink.history(&id, month, PEER).await.unwrap();
    assert_eq!(again.cursor, Some(next));
    drop(again);
    let other = sink.history(&id, month, "other-device").await.unwrap();
    assert!(other.cursor.is_none());
    drop(other);
    let mut resume = sink
        .history(&id, AccountTransferHistory::Year, PEER)
        .await
        .unwrap();
    assert!(resume.cursor.is_none());
    resume
        .staging
        .records(vec![record(today, "objects/b/envelope")])
        .await
        .unwrap();
    resume.staging.commit(None).await.unwrap();
    drop(resume);

    let moved = runtime_at(&db).await;
    eventually("the staged history is reflected", async || {
        history_files(&db).is_empty()
    })
    .await;
    for key in ["objects/a/envelope", "objects/b/envelope"] {
        let held = moved
            .sqlite
            .get_remote_records(&bucket(topic(), today), key, Some(DOCS_AUTHOR), 1, true)
            .await
            .unwrap();
        assert_eq!(held.len(), 1, "{key}");
    }
    let content = moved
        .sqlite
        .get_remote_content("blob", &hash)
        .await
        .unwrap();
    assert_eq!(content, Some(blob.clone()));
    moved.shutdown().await;

    // 反映の task は止めて作り直すので、blob を保存した後の部分の削除の途中で止まることがある。残りの部分も消す。
    let mut resume = sink.history(&id, month, PEER).await.unwrap();
    let first = blob[..half].to_vec();
    resume.staging.blob(&hash, len, 0, first).await.unwrap();
    let rest = blob[half..].to_vec();
    resume
        .staging
        .blob(&hash, len, half as u64, rest)
        .await
        .unwrap();
    resume.staging.commit(None).await.unwrap();
    drop(resume);
    fs::remove_file(db.with_extension("account-history-0-0.bin")).unwrap();
    let moved = runtime_at(&db).await;
    eventually("the rest of the parts are removed", async || {
        history_files(&db).is_empty()
    })
    .await;
    moved.shutdown().await;
    host.shutdown().await;
}
