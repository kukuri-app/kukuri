//! #1650: 本人の端末どうしの和集合の同期。待ち受けた 2 端末は、押した順によらず 1 本の接続で、鍵を除く必須 bundle と
//! 投稿の記録を送り合って取り込む（T1）。別のアカウントの端末・待ち受けていない端末とはつながらず、何も受けない（T2）。
//! 待ち受けは期限で終わり、送り合う途中の取消は、取り消した端末で中止、相手では中断になる（T4）。

use kukuri_core::KukuriKeys;

use super::*;

/// 同期を待ち受ける端末: node と、送る必須 bundle・投稿の記録と、受けたものの保存。
struct Device {
    node: Arc<IrohDocsNode>,
    source: Arc<HistorySource>,
    sink: Arc<HistorySink>,
}

impl Device {
    async fn new(source: HistorySource) -> Result<Self> {
        Ok(Self {
            node: IrohDocsNode::memory().await?,
            source: Arc::new(source),
            sink: Arc::new(HistorySink::default()),
        })
    }

    fn transfer(&self) -> &AccountTransfer {
        self.node.account_transfer()
    }

    /// `peers` の端末へつなぐ同期を待ち受ける。
    fn sync(&self, keys: &KukuriKeys, peers: &[&Device]) {
        self.transfer().sync(
            keys.derive_account_sync(),
            self.source.clone(),
            self.sink.clone(),
            peers
                .iter()
                .map(|peer| peer.node.endpoint().addr())
                .collect(),
        );
    }

    fn id(&self) -> String {
        self.node.endpoint().id().to_string()
    }
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// a は必須 bundle 3 件と、投稿 3 件（本文 1 つ）・1 件（移行元に無い本文 1 つ）の 2 page を持つ。
fn source_a() -> HistorySource {
    let text = vec![7u8; 100];
    HistorySource {
        blobs: HashMap::from([(hash(&text), text.clone())]),
        ..HistorySource::new(vec![(3, vec![hash(&text)]), (1, vec!["f".repeat(64)])])
    }
}

/// b は必須 bundle 2 件・1 件の 2 page と、投稿 2 件の 1 page を持つ。
fn source_b() -> HistorySource {
    HistorySource {
        bundle: FakeSource::pages(&[2, 1]),
        ..HistorySource::new(vec![(2, Vec::new())])
    }
}

fn synced(posts: u64, unavailable: u64, stopped: Option<Failure>) -> Status {
    Status::Completed {
        role: Role::Sync,
        account_id: None,
        history: Some(AccountTransferHistoryResult {
            posts,
            unavailable,
            stopped,
        }),
    }
}

/// T1: 2 端末が同期を待ち受けると、押した順（a が先・b が先・同時）によらず 1 本の接続で、互いの必須 bundle（鍵を除く）
/// と投稿の記録すべて（本文の blob つき）を送り合い、受けた向きの数で完了する。投稿の記録は「すべて」の範囲で、送り元の
/// 端末ごとの置き場へ受ける。
#[tokio::test]
async fn own_devices_sync_both_ways_over_one_connection() -> Result<()> {
    let keys = KukuriKeys::generate();
    for order in ["a first", "b first", "together"] {
        let a = Device::new(source_a()).await?;
        let b = Device::new(source_b()).await?;
        match order {
            "a first" => {
                a.sync(&keys, &[&b]);
                n0_future::time::sleep(Duration::from_millis(200)).await;
                b.sync(&keys, &[&a]);
            }
            "b first" => {
                b.sync(&keys, &[&a]);
                n0_future::time::sleep(Duration::from_millis(200)).await;
                a.sync(&keys, &[&b]);
            }
            _ => {
                a.sync(&keys, &[&b]);
                b.sync(&keys, &[&a]);
            }
        }
        let done_a = wait_for(a.transfer(), order, is_completed).await?;
        let done_b = wait_for(b.transfer(), order, is_completed).await?;
        assert_eq!(done_a, synced(2, 0, None), "{order}");
        assert_eq!(done_b, synced(4, 1, None), "{order}");
        // 必須 bundle は 1 回ずつ受けて確定した（2 本目の接続では送り合わない）。
        let bundle = |device: &Device| {
            let log = device.sink.bundle.log.lock().unwrap();
            (log.begun, log.staged.clone(), log.committed)
        };
        assert_eq!(bundle(&a), (1, vec![2, 1], 1), "{order}");
        assert_eq!(bundle(&b), (1, vec![3], 1), "{order}");
        let text = (hash(&[7u8; 100]), 100);
        assert_eq!(
            b.sink.log.lock().unwrap().pages,
            [(3, vec![text]), (1, Vec::new())],
            "{order}"
        );
        assert_eq!(a.sink.log.lock().unwrap().pages, [(2, Vec::new())]);
        assert_eq!(
            a.sink.log.lock().unwrap().opened,
            [(ACCOUNT.to_string(), History::All, b.id())]
        );
        assert_eq!(
            b.sink.log.lock().unwrap().opened,
            [(ACCOUNT.to_string(), History::All, a.id())]
        );
        // 鍵の frame は送らない（各端末が自分の鍵で保存を始める）。
        for device in [&a, &b] {
            assert_eq!(*device.source.bundle.sent.lock().unwrap(), 1, "{order}");
        }
    }
    Ok(())
}

/// T2: 別のアカウントの鍵の端末とはつながらず、待ち受けていない端末からは何も受けない。同じアカウントの端末が待ち受け
/// たら、その端末とだけ送り合う。
#[tokio::test]
async fn only_a_waiting_device_of_the_same_account_is_synced() -> Result<()> {
    let keys = KukuriKeys::generate();
    let a = Device::new(source_a()).await?;
    let stranger = Device::new(source_b()).await?;
    let idle = Device::new(source_b()).await?;
    let b = Device::new(source_b()).await?;
    stranger.sync(&KukuriKeys::generate(), &[&a]);
    a.sync(&keys, &[&stranger, &idle]);
    b.sync(&keys, &[&a]);
    assert_eq!(
        wait_for(a.transfer(), "a", is_completed).await?,
        synced(2, 0, None)
    );
    wait_for(b.transfer(), "b", is_completed).await?;
    assert!(matches!(
        stranger.transfer().status(),
        Status::Waiting { .. }
    ));
    assert_eq!(idle.transfer().status(), Status::Idle);
    for device in [&stranger, &idle] {
        assert_eq!(device.sink.bundle.log.lock().unwrap().begun, 0);
        assert_eq!(*device.sink.log.lock().unwrap(), HistoryLog::default());
    }
    assert_eq!(a.sink.log.lock().unwrap().opened[0].2, b.id());
    Ok(())
}

/// T4: 待ち受けは期限で終わり、その後の接続を受けない。送り合う途中で受ける側が取り消すと、取り消した端末は確定した
/// page までで中止になる。相手は、自分が受け終えていても、送り終える前に止まったので中断を示す（独立監査 B-1）。
/// 待ち受けの取消は状態を消す。
#[tokio::test]
async fn the_wait_expires_and_a_cancel_stops_the_sync() -> Result<()> {
    let keys = KukuriKeys::generate();
    let a = Device::new(source_a()).await?;
    let mut session = Session::new(
        Role::Sync,
        Pairing::Account(keys.derive_account_sync()),
        Status::Waiting {
            expires_at_ms: now_ms() + 300,
        },
        Some(a.source.clone()),
        None,
    );
    Arc::get_mut(&mut session).unwrap().sink = Some(a.sink.clone());
    a.transfer().replace(Some(session), None);
    n0_future::time::sleep(Duration::from_millis(400)).await;
    assert!(failed(Failure::Expired)(&a.transfer().status()));
    assert!(a.transfer().waiting_sync().is_none());
    a.transfer().cancel();
    assert_eq!(a.transfer().status(), Status::Idle);

    // b の 2 page 目の本文を止めておき、b が a の投稿をすべて受け、a が b の 1 page 目を確定した後に、a が取り消す。
    let gated = "e".repeat(64);
    let gate = Arc::new(Notify::new());
    let b = Device::new(HistorySource {
        blob_gate: Some((gated.clone(), gate.clone())),
        ..HistorySource::new(vec![(2, Vec::new()), (1, vec![gated])])
    })
    .await?;
    a.sync(&keys, &[&b]);
    b.sync(&keys, &[&a]);
    timeout(WAIT, async {
        while {
            let log = a.sink.log.lock().unwrap();
            (log.pages.len(), log.pending) != (1, 1) || b.sink.log.lock().unwrap().pages.len() != 2
        } {
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    a.transfer().cancel();
    gate.notify_one();
    assert_eq!(
        wait_for(a.transfer(), "a cancelled", is_completed).await?,
        synced(2, 0, Some(Failure::Cancelled))
    );
    assert_eq!(
        wait_for(b.transfer(), "b interrupted", is_completed).await?,
        synced(4, 1, Some(Failure::Interrupted))
    );
    assert_eq!(a.sink.log.lock().unwrap().pages, [(2, Vec::new())]);
    Ok(())
}
