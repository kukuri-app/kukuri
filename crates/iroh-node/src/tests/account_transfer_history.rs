//! #1211 AC-3（3a・3d）: 必須の移行の後の履歴の stream。履歴を選んだときだけ範囲を送り、page ごとに保存の ACK を
//! 受けて進むこと、中止・失敗でも両端末の必須の移行の完了を保つこと、置き場が返す位置から続けること。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use kukuri_core::AccountTransferHistory as History;

use super::*;

fn history_record(page: usize, index: usize) -> AccountHistoryRecord {
    AccountHistoryRecord {
        replica: format!("bucket::v1::topic::74::{}", 20_000 + page),
        key: format!("objects/{page}-{index}/envelope"),
        docs_author: "a".repeat(64),
        value: vec![b'v'; 32],
    }
}

/// page の番号を key に置いた続きの位置。
fn page_cursor(page: usize) -> AccountHistoryCursor {
    AccountHistoryCursor {
        reference: "own_docs".into(),
        key: page.to_string(),
        ..AccountHistoryCursor::default()
    }
}

/// 履歴を持つ移行元の fake。必須 bundle は 3 件の 1 page。履歴は `history` の page（record の数と blob の hash）を順に
/// 返し、`blobs` に無い hash は移行元に無いとする。`blob_gate` の hash は、最初の一部を返す前に開くまで待つ。
#[derive(Default)]
struct HistorySource {
    bundle: FakeSource,
    history: Vec<(usize, Vec<String>)>,
    blobs: HashMap<String, Vec<u8>>,
    blob_gate: Option<(String, Arc<Notify>)>,
    /// 受けた範囲と位置（呼ばれた順）。
    calls: StdMutex<Vec<(Option<u64>, Option<AccountHistoryCursor>)>>,
    /// 同時に読んでいる page の数と、その最大。
    reading: AtomicUsize,
    most_reading: AtomicUsize,
}

impl HistorySource {
    fn new(history: Vec<(usize, Vec<String>)>) -> Self {
        Self {
            bundle: FakeSource::pages(&[3]),
            history,
            ..Self::default()
        }
    }
}

#[async_trait::async_trait]
impl AccountBundleSource for HistorySource {
    fn secret_hex(&self) -> String {
        self.bundle.secret_hex()
    }

    async fn page(
        &self,
        cursor: Option<String>,
    ) -> Result<(Vec<AccountTransferItem>, Option<String>)> {
        self.bundle.page(cursor).await
    }

    async fn history_page(
        &self,
        since: Option<u64>,
        cursor: Option<AccountHistoryCursor>,
    ) -> Result<AccountHistoryPage> {
        let reading = self.reading.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_reading.fetch_max(reading, Ordering::SeqCst);
        self.calls.lock().unwrap().push((since, cursor.clone()));
        let index = cursor.map_or(0, |cursor| cursor.key.parse().unwrap());
        n0_future::time::sleep(Duration::from_millis(5)).await;
        let (count, blobs) = self.history[index].clone();
        self.reading.fetch_sub(1, Ordering::SeqCst);
        Ok(AccountHistoryPage {
            records: (0..count).map(|i| history_record(index, i)).collect(),
            blobs,
            posts: count as u64,
            unavailable: 0,
            next: (index + 1 < self.history.len()).then(|| page_cursor(index + 1)),
        })
    }

    async fn blob_part(
        &self,
        hash: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<(u64, Vec<u8>)>> {
        if offset == 0
            && let Some((gated, gate)) = &self.blob_gate
            && gated == hash
        {
            gate.notified().await;
        }
        Ok(self.blobs.get(hash).map(|bytes| {
            let start = offset as usize;
            let end = bytes.len().min(start + limit);
            (bytes.len() as u64, bytes[start..end].to_vec())
        }))
    }
}

/// 履歴の置き場を持つ移行先の fake。必須 bundle は `bundle` に保存する。履歴は `resume` の範囲と位置から始め、page
/// ごとの record の数と揃った blob を記録する。`fault` 番の page の確定は保存の失敗になる。
#[derive(Default)]
struct HistorySink {
    bundle: FakeSink,
    resume: Option<(Option<u64>, AccountHistoryCursor)>,
    fault: Option<usize>,
    log: Arc<StdMutex<HistoryLog>>,
}

#[derive(Default, Debug, PartialEq)]
struct HistoryLog {
    opened: Vec<(String, History)>,
    /// 確定した page の record の数と、揃った blob（hash と長さ）。
    pages: Vec<(usize, Vec<(String, usize)>)>,
    /// 確定していない page で受けた record の数。
    pending: usize,
    /// 確定しないまま落とされた page。
    abandoned: usize,
}

#[async_trait::async_trait]
impl AccountBundleSink for HistorySink {
    async fn begin(&self, secret_hex: &str) -> Result<Box<dyn AccountBundleStaging>, Failure> {
        self.bundle.begin(secret_hex).await
    }

    async fn history(
        &self,
        account_id: &str,
        history: History,
    ) -> Result<AccountHistoryResume, Failure> {
        let mut log = self.log.lock().unwrap();
        log.opened.push((account_id.to_string(), history));
        let (since, cursor) = match &self.resume {
            Some((since, cursor)) => (*since, Some(cursor.clone())),
            None => (None, None),
        };
        Ok(AccountHistoryResume {
            since,
            cursor,
            staging: Box::new(HistoryStaging {
                fault: self.fault,
                log: self.log.clone(),
                records: 0,
                blob: None,
                blobs: Vec::new(),
            }),
        })
    }
}

struct HistoryStaging {
    fault: Option<usize>,
    log: Arc<StdMutex<HistoryLog>>,
    records: usize,
    blob: Option<(String, Vec<u8>)>,
    blobs: Vec<(String, usize)>,
}

impl Drop for HistoryStaging {
    fn drop(&mut self) {
        if self.records > 0 || self.blob.is_some() || !self.blobs.is_empty() {
            self.log.lock().unwrap().abandoned += 1;
        }
    }
}

#[async_trait::async_trait]
impl AccountHistoryStaging for HistoryStaging {
    async fn records(&mut self, records: Vec<AccountHistoryRecord>) -> Result<(), Failure> {
        self.records += records.len();
        self.log.lock().unwrap().pending = self.records;
        Ok(())
    }

    async fn blob(
        &mut self,
        hash: &str,
        len: u64,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<(), Failure> {
        let (_, received) = self
            .blob
            .get_or_insert_with(|| (hash.to_string(), Vec::new()));
        if received.len() as u64 != offset {
            return Err(Failure::Invalid);
        }
        received.extend(bytes);
        if received.len() as u64 == len {
            let (hash, received) = self.blob.take().unwrap();
            if blake3::hash(&received).to_hex().as_str() != hash {
                return Err(Failure::Invalid);
            }
            self.blobs.push((hash, received.len()));
        }
        Ok(())
    }

    async fn commit(&mut self, _next: Option<AccountHistoryCursor>) -> Result<(), Failure> {
        let mut log = self.log.lock().unwrap();
        if self.blob.is_some() || self.fault == Some(log.pages.len()) {
            return Err(Failure::Storage);
        }
        let page = (
            std::mem::take(&mut self.records),
            std::mem::take(&mut self.blobs),
        );
        log.pages.push(page);
        log.pending = 0;
        Ok(())
    }
}

/// 移行元の招待を移行先が `history` の範囲で開き、両端末で承認する。
async fn start(
    source: Arc<HistorySource>,
    sink: Arc<HistorySink>,
    history: Option<History>,
) -> Result<(Arc<IrohDocsNode>, Arc<IrohDocsNode>)> {
    let source_node = IrohDocsNode::memory().await?;
    let target_node = IrohDocsNode::memory().await?;
    let link = source_node.account_transfer().issue(source)?.to_link();
    target_node.account_transfer().open(&link, sink, history)?;
    confirm_both(
        source_node.account_transfer(),
        target_node.account_transfer(),
    )
    .await?;
    Ok((source_node, target_node))
}

/// 履歴の結果で完了したか。
fn history_ended(stopped: Option<Failure>, posts: u64) -> impl Fn(&Status) -> bool + Copy {
    move |status| {
        matches!(
            status,
            Status::Completed { history: Some(result), .. }
                if result.stopped == stopped && result.posts == posts
        )
    }
}

/// 3a: 履歴を選ばなければ、移行先は必須の ACK の後に接続を閉じ、移行元へ履歴を求めない。選べば、同じ接続の新しい
/// stream で範囲を送り、移行元は page ごとに record・blob（512 KiB ずつ）・page の終わりを順に送り、保存の ACK を
/// 受けてから次の page を読む（page を同時に読まない）。移行元に無い blob は数え、両端末とも必須と履歴の結果を持って
/// 完了する。
#[tokio::test]
async fn the_history_follows_the_bundle_only_when_chosen() -> Result<()> {
    let small = vec![1u8; 10];
    let large: Vec<u8> = (0..MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES * 2 + 7)
        .map(|i| i as u8)
        .collect();
    let hash = |bytes: &[u8]| blake3::hash(bytes).to_hex().to_string();
    for history in [None, Some(History::All)] {
        let source = Arc::new(HistorySource {
            blobs: HashMap::from([(hash(&small), small.clone()), (hash(&large), large.clone())]),
            ..HistorySource::new(vec![
                (3, vec![hash(&small), hash(&large)]),
                (2, vec!["f".repeat(64)]),
            ])
        });
        let sink = Arc::new(HistorySink::default());
        let (source_node, target_node) = start(source.clone(), sink.clone(), history).await?;
        let target = wait_for(target_node.account_transfer(), "target", is_completed).await?;
        let done = wait_for(source_node.account_transfer(), "source", is_completed).await?;
        let result = history.map(|_| AccountTransferHistoryResult {
            posts: 5,
            unavailable: 1,
            stopped: None,
        });
        assert_eq!(
            target,
            Status::Completed {
                role: Role::Target,
                account_id: Some(ACCOUNT.to_string()),
                history: result.clone(),
            }
        );
        assert_eq!(
            done,
            Status::Completed {
                role: Role::Source,
                account_id: None,
                history: result,
            }
        );
        assert_eq!(sink.bundle.log.lock().unwrap().committed, 1);
        let calls = source.calls.lock().unwrap().clone();
        let log = sink.log.lock().unwrap();
        if history.is_none() {
            assert!(calls.is_empty());
            assert_eq!(*log, HistoryLog::default());
            continue;
        }
        assert_eq!(calls, [(None, None), (None, Some(page_cursor(1)))]);
        assert_eq!(source.most_reading.load(Ordering::SeqCst), 1);
        assert_eq!(log.opened, [(ACCOUNT.to_string(), History::All)]);
        let blobs = vec![(hash(&small), small.len()), (hash(&large), large.len())];
        assert_eq!(log.pages, [(3, blobs), (2, Vec::new())]);
        assert_eq!((log.pending, log.abandoned), (0, 0));
    }
    Ok(())
}

/// 3d: 履歴の途中でどちらかが取り消すと、取り消した端末は中止、相手は切断として、両端末とも必須の移行は完了のまま
/// 履歴を止める（確定した page はそのまま）。移行先の確定していない page は落とされる。止めた後の取消は状態を消す。
#[tokio::test]
async fn stopping_the_history_keeps_the_bundle_completed() -> Result<()> {
    for cancel_source in [true, false] {
        let gated = "e".repeat(64);
        let gate = Arc::new(Notify::new());
        let source = Arc::new(HistorySource {
            blob_gate: Some((gated.clone(), gate.clone())),
            ..HistorySource::new(vec![(3, Vec::new()), (2, vec![gated]), (1, Vec::new())])
        });
        let sink = Arc::new(HistorySink::default());
        let (source_node, target_node) = start(source, sink.clone(), Some(History::Month)).await?;
        // 1 page 目を確定し、2 page 目の record を受けた後、blob を待っている間に取り消す。
        timeout(WAIT, async {
            while sink.log.lock().unwrap().pending < 2 {
                n0_future::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let (cancelling, other) = if cancel_source {
            (
                source_node.account_transfer(),
                target_node.account_transfer(),
            )
        } else {
            (
                target_node.account_transfer(),
                source_node.account_transfer(),
            )
        };
        cancelling.cancel();
        gate.notify_one();
        wait_for(
            cancelling,
            "cancelled",
            history_ended(Some(Failure::Cancelled), 3),
        )
        .await?;
        wait_for(
            other,
            "interrupted",
            history_ended(Some(Failure::Interrupted), 3),
        )
        .await?;
        assert!(matches!(
            target_node.account_transfer().status(),
            Status::Completed {
                account_id: Some(_),
                ..
            }
        ));
        timeout(WAIT, async {
            while sink.log.lock().unwrap().abandoned == 0 {
                n0_future::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        assert_eq!(sink.log.lock().unwrap().pages, [(3, Vec::new())]);
        assert_eq!(sink.bundle.log.lock().unwrap().committed, 1);
        cancelling.cancel();
        assert_eq!(cancelling.status(), Status::Idle);
    }
    Ok(())
}

/// 3d: 移行先が履歴の page を保存できなければ、両端末とも必須の移行は完了のまま、履歴は保存の失敗で止まる。
#[tokio::test]
async fn a_history_storage_failure_keeps_the_bundle_completed() -> Result<()> {
    let source = Arc::new(HistorySource::new(vec![(3, Vec::new()), (2, Vec::new())]));
    let sink = Arc::new(HistorySink {
        fault: Some(1),
        ..HistorySink::default()
    });
    let (source_node, target_node) = start(source, sink.clone(), Some(History::Year)).await?;
    let storage = history_ended(Some(Failure::Storage), 3);
    wait_for(target_node.account_transfer(), "target storage", storage).await?;
    wait_for(source_node.account_transfer(), "source storage", storage).await?;
    assert_eq!(sink.bundle.log.lock().unwrap().committed, 1);
    assert_eq!(sink.log.lock().unwrap().pages, [(3, Vec::new())]);
    Ok(())
}

/// 3d: 移行先の置き場が返す範囲と続きの位置から、移行元は続きの page だけを送る。
#[tokio::test]
async fn the_history_starts_at_the_position_of_the_staging() -> Result<()> {
    let source = Arc::new(HistorySource::new(vec![(3, Vec::new()), (2, Vec::new())]));
    let sink = Arc::new(HistorySink {
        resume: Some((Some(20_000), page_cursor(1))),
        ..HistorySink::default()
    });
    let (source_node, target_node) =
        start(source.clone(), sink.clone(), Some(History::Month)).await?;
    let done = history_ended(None, 2);
    wait_for(target_node.account_transfer(), "target done", done).await?;
    wait_for(source_node.account_transfer(), "source done", done).await?;
    assert_eq!(
        *source.calls.lock().unwrap(),
        [(Some(20_000), Some(page_cursor(1)))]
    );
    assert_eq!(sink.log.lock().unwrap().pages, [(2, Vec::new())]);
    Ok(())
}
