//! #1211 AC-1（1b〜1d）: QR・専用リンクの移行の接続と両端末の確認。負例では、どちらの端末にも転送（確認の後の
//! 唯一の続き）が始まらないことを確かめる。AC-2（2a・2c・2f）: 確認の後の必須 bundle の転送と、保存の確定の境界。

use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use kukuri_core::{
    ACCOUNT_TRANSFER_LINK_PREFIX, AccountTransferStatus as Status,
    MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS, SealedAccountSyncItem,
};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};
use tokio::sync::Notify;

use super::*;
use crate::{IrohDocsNode, NodeOptions};

const WAIT: Duration = Duration::from_secs(20);
/// custom path を閉じた後の relay への切替の待ち（iroh の path の idle 期限 15 秒と、その後の転送）。
const FALLBACK_WAIT: Duration = Duration::from_secs(45);
const ACCOUNT: &str = "acct";

/// 中身を確かめない item（封の検証は保存先の責務）。`bytes` は封の大きさ。
fn item(index: usize, bytes: usize) -> AccountTransferItem {
    AccountTransferItem {
        key: format!("trust/always-visible/{index:064x}"),
        sealed: SealedAccountSyncItem {
            v: 1,
            nonce_hex: "00".repeat(24),
            ciphertext_hex: "ab".repeat(bytes / 2),
        },
    }
}

/// 移行元の fake。page を順に返す。`gate` があれば、2 page 目からは 1 page ごとに開くまで返さない。
/// `fail_at` の page は読めない（移行元の保存の失敗）。
#[derive(Default)]
struct FakeSource {
    pages: Vec<Vec<AccountTransferItem>>,
    gate: Option<Arc<Notify>>,
    fail_at: Option<usize>,
    sent: StdMutex<usize>,
}

impl FakeSource {
    fn pages(sizes: &[usize]) -> Self {
        Self {
            pages: sizes
                .iter()
                .map(|size| (0..*size).map(|index| item(index, 64)).collect())
                .collect(),
            ..Self::default()
        }
    }
}

#[async_trait::async_trait]
impl AccountBundleSource for FakeSource {
    fn secret_hex(&self) -> String {
        *self.sent.lock().unwrap() += 1;
        "11".repeat(32)
    }

    async fn page(
        &self,
        cursor: Option<String>,
    ) -> Result<(Vec<AccountTransferItem>, Option<String>)> {
        let index = cursor.map_or(0, |cursor| cursor.parse().unwrap());
        if index > 0
            && let Some(gate) = &self.gate
        {
            gate.notified().await;
        }
        ensure!(self.fail_at != Some(index), "injected source failure");
        let next = (index + 1 < self.pages.len()).then(|| (index + 1).to_string());
        Ok((self.pages.get(index).cloned().unwrap_or_default(), next))
    }
}

/// 移行先の保存で故障を入れる境界。
#[derive(Clone, Copy, PartialEq)]
enum Fault {
    Begin,
    /// n 回目（0 始まり）の chunk の保存。
    Stage(usize, Failure),
    Commit,
}

/// 移行先の fake。保存した chunk の件数・確定・破棄を記録する。`commit_gate`（入った・開く）があれば、確定に入った
/// ことを知らせ、開くまで待つ。
#[derive(Default)]
struct FakeSink {
    fault: Option<Fault>,
    commit_gate: Option<Arc<(Notify, Notify)>>,
    log: Arc<StdMutex<SinkLog>>,
}

#[derive(Default, Debug, PartialEq)]
struct SinkLog {
    begun: usize,
    staged: Vec<usize>,
    committed: usize,
    aborted: usize,
    /// 確定も `abort` もされずに落とされた保存（取消・停止で task ごと止めた）。
    abandoned: usize,
}

struct FakeStaging {
    fault: Option<Fault>,
    commit_gate: Option<Arc<(Notify, Notify)>>,
    log: Arc<StdMutex<SinkLog>>,
    settled: bool,
}

#[async_trait::async_trait]
impl AccountBundleSink for FakeSink {
    async fn begin(&self, secret_hex: &str) -> Result<Box<dyn AccountBundleStaging>, Failure> {
        assert_eq!(secret_hex, "11".repeat(32));
        if self.fault == Some(Fault::Begin) {
            return Err(Failure::Storage);
        }
        self.log.lock().unwrap().begun += 1;
        Ok(Box::new(FakeStaging {
            fault: self.fault,
            commit_gate: self.commit_gate.clone(),
            log: self.log.clone(),
            settled: false,
        }))
    }
}

impl Drop for FakeStaging {
    fn drop(&mut self) {
        if !self.settled {
            self.log.lock().unwrap().abandoned += 1;
        }
    }
}

#[async_trait::async_trait]
impl AccountBundleStaging for FakeStaging {
    async fn stage(&mut self, items: Vec<AccountTransferItem>) -> Result<(), Failure> {
        let mut log = self.log.lock().unwrap();
        if let Some(Fault::Stage(at, reason)) = self.fault
            && log.staged.len() == at
        {
            return Err(reason);
        }
        log.staged.push(items.len());
        Ok(())
    }

    async fn commit(&mut self) -> Result<String, Failure> {
        if let Some(gate) = &self.commit_gate {
            gate.0.notify_one();
            gate.1.notified().await;
        }
        if self.fault == Some(Fault::Commit) {
            return Err(Failure::Storage);
        }
        self.settled = true;
        self.log.lock().unwrap().committed += 1;
        Ok(ACCOUNT.to_string())
    }

    async fn abort(&mut self) {
        self.settled = true;
        self.log.lock().unwrap().aborted += 1;
    }
}

fn fake_source() -> Arc<FakeSource> {
    Arc::new(FakeSource::pages(&[3, 2]))
}

fn fake_sink() -> Arc<FakeSink> {
    Arc::new(FakeSink::default())
}

async fn wait_for(
    transfer: &AccountTransfer,
    what: &str,
    done: impl Fn(&Status) -> bool,
) -> Result<Status> {
    wait_within(transfer, WAIT, what, done).await
}

async fn wait_within(
    transfer: &AccountTransfer,
    limit: Duration,
    what: &str,
    done: impl Fn(&Status) -> bool,
) -> Result<Status> {
    timeout(limit, async {
        loop {
            let status = transfer.status();
            if done(&status) {
                return status;
            }
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("{what}: {:?}", transfer.status()))
}

fn code_of(status: &Status) -> Option<&str> {
    match status {
        Status::Confirming { code, .. } => Some(code),
        _ => None,
    }
}

fn failed(reason: Failure) -> impl Fn(&Status) -> bool {
    move |status| matches!(status, Status::Failed { reason: actual, .. } if *actual == reason)
}

/// 確認の後の転送が始まった（または終わった）。
fn is_confirmed(status: &Status) -> bool {
    matches!(
        status,
        Status::Transferring { .. } | Status::Completed { .. }
    )
}

fn is_completed(status: &Status) -> bool {
    matches!(status, Status::Completed { .. })
}

/// リンクの JSON を書き換える。
fn tamper(link: &str, edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let payload = link.strip_prefix(ACCOUNT_TRANSFER_LINK_PREFIX).unwrap();
    let mut json: serde_json::Value =
        serde_json::from_slice(&BASE64_URL.decode(payload).unwrap()).unwrap();
    edit(&mut json);
    format!(
        "{ACCOUNT_TRANSFER_LINK_PREFIX}{}",
        BASE64_URL.encode(serde_json::to_vec(&json).unwrap())
    )
}

/// 両端末で確認コードの一致を確かめて承認する。
async fn decide_both(source: &AccountTransfer, target: &AccountTransfer) -> Result<()> {
    let at_source = wait_for(source, "source shows a code", |s| code_of(s).is_some()).await?;
    let at_target = wait_for(target, "target shows a code", |s| code_of(s).is_some()).await?;
    assert_eq!(code_of(&at_source), code_of(&at_target));
    source.decide(true)?;
    target.decide(true)
}

/// 両端末で承認し、両方が転送へ進む。
async fn confirm_both(source: &AccountTransfer, target: &AccountTransfer) -> Result<()> {
    decide_both(source, target).await?;
    wait_for(source, "source confirmed", is_confirmed).await?;
    wait_for(target, "target confirmed", is_confirmed).await?;
    Ok(())
}

/// 確認して、両端末の転送の完了まで待つ。
async fn transfer_both(source: &AccountTransfer, target: &AccountTransfer) -> Result<()> {
    confirm_both(source, target).await?;
    completed_both(source, target).await
}

/// 移行先は受けたアカウントの ID つきで、移行元は ACK を受けて完了する。
async fn completed_both(source: &AccountTransfer, target: &AccountTransfer) -> Result<()> {
    let done = wait_for(target, "target completed", is_completed).await?;
    assert_eq!(
        done,
        Status::Completed {
            role: Role::Target,
            account_id: Some(ACCOUNT.to_string())
        }
    );
    let done = wait_for(source, "source completed", is_completed).await?;
    assert_eq!(
        done,
        Status::Completed {
            role: Role::Source,
            account_id: None
        }
    );
    Ok(())
}

/// 1b・1d・2c: native↔native は招待の直接 addr で接続し（relay なし = Direct P2P）、両方の承認の後に必須 bundle を
/// 送る。移行先の確定の後に、両端末が完了になる。
#[tokio::test]
async fn native_nodes_confirm_over_the_direct_path() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let target = IrohDocsNode::memory().await?;
    let invite = source.account_transfer().issue(fake_source())?;
    assert!(invite.relay_url.is_none());
    assert!(!invite.direct_addrs.is_empty());
    assert!(matches!(
        source.account_transfer().status(),
        Status::Waiting { .. }
    ));

    let sink = fake_sink();
    target
        .account_transfer()
        .open(&invite.to_link(), sink.clone())?;
    transfer_both(source.account_transfer(), target.account_transfer()).await?;
    assert_eq!(
        *sink.log.lock().unwrap(),
        SinkLog {
            begun: 1,
            staged: vec![3, 2],
            committed: 1,
            aborted: 0,
            abandoned: 0,
        }
    );

    // 1c 再生: 使用済みの招待は、同じ端末からも別の端末からも受けない。
    target
        .account_transfer()
        .open(&invite.to_link(), fake_sink())?;
    wait_for(
        target.account_transfer(),
        "replay",
        failed(Failure::Invalid),
    )
    .await?;
    let third = IrohDocsNode::memory().await?;
    third
        .account_transfer()
        .open(&invite.to_link(), fake_sink())?;
    wait_for(third.account_transfer(), "replay", failed(Failure::Invalid)).await?;
    assert!(is_confirmed(&source.account_transfer().status()));
    Ok(())
}

/// 1c 改竄・未認証: 秘密を変えたリンクと、証明の誤った接続は拒否する。待っている招待は消費されず、正しいリンクで続けられる。
#[tokio::test]
async fn a_wrong_proof_is_rejected_without_consuming_the_invite() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let link = source.account_transfer().issue(fake_source())?.to_link();
    let forger = IrohDocsNode::memory().await?;
    forger.account_transfer().open(
        &tamper(&link, |json| json["secret"] = "ab".repeat(32).into()),
        fake_sink(),
    )?;
    wait_for(
        forger.account_transfer(),
        "forged secret",
        failed(Failure::Invalid),
    )
    .await?;

    // 証明の代わりに任意の bytes を送る接続。
    let raw = IrohDocsNode::memory().await?;
    let connection = raw
        .endpoint()
        .connect(source.endpoint().addr(), ACCOUNT_TRANSFER_ALPN)
        .await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(&[7; 32]).await?;
    let mut verdict = [9];
    recv.read_exact(&mut verdict).await?;
    assert_eq!(verdict, [REJECTED]);
    assert!(matches!(
        source.account_transfer().status(),
        Status::Waiting { .. }
    ));

    let target = IrohDocsNode::memory().await?;
    target.account_transfer().open(&link, fake_sink())?;
    confirm_both(source.account_transfer(), target.account_transfer()).await
}

/// 1c 期限切れ: 期限の正本は移行元。移行先のリンクの期限を延ばしても、移行元の期限を過ぎた招待は受けない。
#[tokio::test]
async fn an_invite_expired_at_the_source_is_rejected() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let link = source.account_transfer().issue(fake_source())?.to_link();
    let short = tamper(&link, |json| {
        json["expires_at_ms"] = (now_ms() + 300).into()
    });
    let invite = AccountTransferInvite::parse_link(&short, now_ms())?;
    let expires_at_ms = invite.expires_at_ms;
    source.account_transfer().replace(
        Some(Session::new(
            Role::Source,
            invite,
            Status::Waiting { expires_at_ms },
            Some(fake_source()),
        )),
        None,
    );
    n0_future::time::sleep(Duration::from_millis(400)).await;
    assert!(failed(Failure::Expired)(
        &source.account_transfer().status()
    ));

    let target = IrohDocsNode::memory().await?;
    let extended = tamper(&short, |json| {
        json["expires_at_ms"] = (now_ms() + 60_000).into()
    });
    target.account_transfer().open(&extended, fake_sink())?;
    wait_for(
        target.account_transfer(),
        "expired",
        failed(Failure::Invalid),
    )
    .await?;
    // 移行先の手元でも、期限を過ぎたリンクは開く前に拒否する。
    assert!(target.account_transfer().open(&short, fake_sink()).is_err());
    Ok(())
}

/// 1c 接続先違い: 別の端末を指すリンクは、その端末の秘密と合わず拒否される。id と addr が食い違うリンクは接続できない。
#[tokio::test]
async fn a_link_pointing_at_another_device_is_rejected() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let other = IrohDocsNode::memory().await?;
    let link = source.account_transfer().issue(fake_source())?.to_link();
    let other_link = other.account_transfer().issue(fake_source())?.to_link();
    let other_invite = AccountTransferInvite::parse_link(&other_link, now_ms())?;

    let target = IrohDocsNode::memory().await?;
    let to_other = tamper(&link, |json| {
        json["issuer"] = other_invite.issuer.clone().into();
        json["direct_addrs"] = serde_json::to_value(&other_invite.direct_addrs).unwrap();
    });
    target.account_transfer().open(&to_other, fake_sink())?;
    wait_for(
        target.account_transfer(),
        "other device",
        failed(Failure::Invalid),
    )
    .await?;

    let mismatched = tamper(&link, |json| {
        json["direct_addrs"] = serde_json::to_value(&other_invite.direct_addrs).unwrap();
    });
    target.account_transfer().open(&mismatched, fake_sink())?;
    wait_for(
        target.account_transfer(),
        "id mismatch",
        failed(Failure::Unreachable),
    )
    .await?;

    for transfer in [source.account_transfer(), other.account_transfer()] {
        assert!(matches!(transfer.status(), Status::Waiting { .. }));
    }
    Ok(())
}

/// 1c 未認証: どちらかが「一致しない」を選ぶか、片方しか承認しない間は確認済みにならない。
#[tokio::test]
async fn confirmation_needs_both_devices() -> Result<()> {
    for rejecting_source in [true, false] {
        let source = IrohDocsNode::memory().await?;
        let target = IrohDocsNode::memory().await?;
        let link = source.account_transfer().issue(fake_source())?.to_link();
        target.account_transfer().open(&link, fake_sink())?;
        let (accepting, rejecting) = if rejecting_source {
            (target.account_transfer(), source.account_transfer())
        } else {
            (source.account_transfer(), target.account_transfer())
        };
        wait_for(rejecting, "code", |s| code_of(s).is_some()).await?;
        wait_for(accepting, "code", |s| code_of(s).is_some()).await?;
        accepting.decide(true)?;
        wait_for(accepting, "local accepted", |s| {
            matches!(
                s,
                Status::Confirming {
                    local_accepted: true,
                    ..
                }
            )
        })
        .await?;
        n0_future::time::sleep(Duration::from_millis(300)).await;
        assert!(
            matches!(accepting.status(), Status::Confirming { .. }),
            "one side only"
        );

        rejecting.decide(false)?;
        wait_for(rejecting, "rejected", failed(Failure::Rejected)).await?;
        wait_for(accepting, "rejected by peer", failed(Failure::Rejected)).await?;
        assert!(accepting.decide(true).is_err());
    }

    // 移行先が取り消すと、移行元も確認済みにならない。
    let source = IrohDocsNode::memory().await?;
    let target = IrohDocsNode::memory().await?;
    target.account_transfer().open(
        &source.account_transfer().issue(fake_source())?.to_link(),
        fake_sink(),
    )?;
    wait_for(source.account_transfer(), "code", |s| code_of(s).is_some()).await?;
    source.account_transfer().decide(true)?;
    target.account_transfer().cancel();
    assert_eq!(target.account_transfer().status(), Status::Idle);
    wait_for(source.account_transfer(), "peer cancelled", |s| {
        matches!(s, Status::Failed { .. })
    })
    .await?;
    Ok(())
}

/// 1c: 招待を出していない端末は、接続を受けても何も応じない。
#[tokio::test]
async fn a_device_without_an_invite_does_not_answer() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let raw = IrohDocsNode::memory().await?;
    let connection = raw
        .endpoint()
        .connect(source.endpoint().addr(), ACCOUNT_TRANSFER_ALPN)
        .await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(&[7; 32]).await?;
    assert!(recv.read_to_end(1).await.is_err());
    assert_eq!(source.account_transfer().status(), Status::Idle);
    Ok(())
}

/// 手元の relay と WebRTC の transport を持つ node（ブラウザ相当の端は交渉を始める）。
async fn relay_node(
    relay: &RelayUrl,
    browser_like: bool,
) -> Result<(Arc<IrohDocsNode>, Arc<WebRtcTransport>)> {
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: Ipv4Addr::LOCALHOST.into(),
    });
    let node = IrohDocsNode::memory_with(NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        webrtc: Some(transport.clone()),
        browser_like,
        ..NodeOptions::default()
    })
    .await?;
    node.endpoint().online().await;
    Ok((node, transport))
}

/// 1d・2f: relay だけで届くブラウザ相当の端（交渉を始める側）は、ICE の完了を待たずに relay で確認と転送を終える
/// （Relay Fallback）。
#[tokio::test]
async fn a_browser_like_device_confirms_over_the_relay() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (source, _) = relay_node(&relay, false).await?;
    let (web, _) = relay_node(&relay, true).await?;
    let invite = source.account_transfer().issue(fake_source())?;
    assert_eq!(
        invite.relay_url.as_deref(),
        Some(relay.to_string().as_str())
    );
    web.account_transfer()
        .open(&invite.to_link(), fake_sink())?;
    transfer_both(source.account_transfer(), web.account_transfer()).await
}

/// 2f（W10 の T3 と同じ fixture）: relay で始めた転送の接続へ交渉で custom path が足され、chunk が custom を通る。
/// 転送の途中で custom path だけを閉じても、同じ接続のまま relay で完走し、確定・ACK は 1 回。
#[tokio::test]
async fn a_transfer_survives_losing_the_custom_path() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (native, _) = relay_node(&relay, false).await?;
    let (web, web_transport) = relay_node(&relay, true).await?;
    let gate = Arc::new(Notify::new());
    // 2 page 目と 3 page 目は 32 KiB の item 8 件ずつ（256 KiB 前後）。
    let large = || (0..8).map(|index| item(index, 32 * 1024)).collect();
    let source = Arc::new(FakeSource {
        pages: vec![vec![item(0, 64)], large(), large()],
        gate: Some(gate.clone()),
        ..FakeSource::default()
    });
    let sink = fake_sink();
    let staged = |sink: &FakeSink| sink.log.lock().unwrap().staged.iter().sum::<usize>();
    let invite = native.account_transfer().issue(source.clone())?;
    web.account_transfer()
        .open(&invite.to_link(), sink.clone())?;
    confirm_both(native.account_transfer(), web.account_transfer()).await?;
    timeout(WAIT, async {
        while !web
            .endpoint()
            .remote_info(native.endpoint().id())
            .await
            .is_some_and(|info| {
                info.addrs().any(|addr| {
                    addr.addr().is_custom()
                        && matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active)
                })
            })
        {
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("the custom path is active")?;

    // 2 page 目は custom を通る。
    let before = web_transport.stats().received_bytes;
    gate.notify_one();
    timeout(WAIT, async {
        while staged(&sink) < 9 {
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("the second page is staged")?;
    assert!(web_transport.stats().received_bytes >= before + 128 * 1024);

    // custom だけを閉じても、3 page 目は同じ接続のまま relay で届く。閉じた custom path は iroh の path の idle 期限
    // （15 秒）まで選ばれたまま残るので、relay への切替をその分だけ待つ（W10 の T3 と同じ）。
    web.webrtc_signaling().context("webrtc")?.reset();
    let at_reset = web_transport.stats().received_bytes;
    gate.notify_one();
    wait_within(
        web.account_transfer(),
        FALLBACK_WAIT,
        "relay fallback",
        is_completed,
    )
    .await?;
    completed_both(native.account_transfer(), web.account_transfer()).await?;
    assert_eq!(web_transport.stats().received_bytes, at_reset);
    assert_eq!(*source.sent.lock().unwrap(), 1, "the bundle was sent once");
    let log = sink.log.lock().unwrap();
    assert_eq!((log.begun, log.committed, log.aborted), (1, 1, 0));
    assert_eq!(log.staged.iter().sum::<usize>(), 17);
    Ok(())
}

/// 2c: 保存の確定より前の故障（保存の開始・chunk の保存・検証・確定・移行元の読み出し）では、両端末とも完了に
/// ならず、移行先は確定しない。保存を始めていれば破棄する。移行先の保存の失敗は、移行元にも保存の失敗として示す。
#[tokio::test]
async fn faults_before_the_commit_complete_neither_device() -> Result<()> {
    let log = |begun, staged: &[usize]| SinkLog {
        begun,
        staged: staged.to_vec(),
        committed: 0,
        aborted: begun,
        abandoned: 0,
    };
    // 移行元の読み出しの失敗は接続を閉じるので、移行先が受けた分は時機による（`None`）。
    let cases = [
        (
            Some(Fault::Begin),
            Failure::Storage,
            Failure::Storage,
            Some(log(0, &[])),
        ),
        (
            Some(Fault::Stage(1, Failure::Storage)),
            Failure::Storage,
            Failure::Storage,
            Some(log(1, &[3])),
        ),
        (
            Some(Fault::Stage(0, Failure::Invalid)),
            Failure::Invalid,
            Failure::Interrupted,
            Some(log(1, &[])),
        ),
        (
            Some(Fault::Commit),
            Failure::Storage,
            Failure::Storage,
            Some(log(1, &[3, 2])),
        ),
        (None, Failure::Interrupted, Failure::Storage, None),
    ];
    for (fault, at_target, at_source, expected) in cases {
        let source_node = IrohDocsNode::memory().await?;
        let target_node = IrohDocsNode::memory().await?;
        let source = FakeSource {
            fail_at: fault.is_none().then_some(1),
            ..FakeSource::pages(&[3, 2])
        };
        let sink = Arc::new(FakeSink {
            fault,
            ..FakeSink::default()
        });
        let link = source_node
            .account_transfer()
            .issue(Arc::new(source))?
            .to_link();
        target_node.account_transfer().open(&link, sink.clone())?;
        decide_both(
            source_node.account_transfer(),
            target_node.account_transfer(),
        )
        .await?;
        wait_for(
            target_node.account_transfer(),
            "target failed",
            failed(at_target),
        )
        .await?;
        wait_for(
            source_node.account_transfer(),
            "source failed",
            failed(at_source),
        )
        .await?;
        let actual = sink.log.lock().unwrap();
        match expected {
            Some(expected) => assert_eq!(*actual, expected),
            None => assert_eq!((actual.committed, actual.aborted), (0, actual.begun)),
        }
    }
    Ok(())
}

/// 2c: 転送の途中の取消。移行元が取り消すと、移行先は切断で保存を破棄する。移行先が取り消すと、移行元は切断に
/// なり、移行先の保存は確定も `abort` もされずに落とされる（移行の task ごと止める。保存先はそのときも消す）。
#[tokio::test]
async fn cancelling_during_the_transfer_completes_neither_device() -> Result<()> {
    for cancel_source in [true, false] {
        let source_node = IrohDocsNode::memory().await?;
        let target_node = IrohDocsNode::memory().await?;
        let gate = Arc::new(Notify::new());
        let source = Arc::new(FakeSource {
            gate: Some(gate.clone()),
            ..FakeSource::pages(&[3, 2])
        });
        let sink = fake_sink();
        let link = source_node.account_transfer().issue(source)?.to_link();
        target_node.account_transfer().open(&link, sink.clone())?;
        confirm_both(
            source_node.account_transfer(),
            target_node.account_transfer(),
        )
        .await?;
        timeout(WAIT, async {
            while sink.log.lock().unwrap().staged.is_empty() {
                n0_future::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        if cancel_source {
            source_node.account_transfer().cancel();
            wait_for(
                target_node.account_transfer(),
                "target interrupted",
                failed(Failure::Interrupted),
            )
            .await?;
            assert_eq!(source_node.account_transfer().status(), Status::Idle);
        } else {
            target_node.account_transfer().cancel();
            gate.notify_one();
            wait_for(
                source_node.account_transfer(),
                "source interrupted",
                failed(Failure::Interrupted),
            )
            .await?;
            assert_eq!(target_node.account_transfer().status(), Status::Idle);
        }
        let ended = || {
            let log = sink.log.lock().unwrap();
            log.abandoned + log.aborted
        };
        timeout(WAIT, async {
            while ended() == 0 {
                n0_future::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let log = sink.log.lock().unwrap();
        assert_eq!(log.committed, 0);
        assert_eq!(
            (log.aborted, log.abandoned),
            (usize::from(cancel_source), usize::from(!cancel_source))
        );
    }
    Ok(())
}

/// 2c: 確定の後に ACK が届かないと、移行先は完了（保存済み）になり、移行元は完了にならない。
#[tokio::test]
async fn a_lost_ack_completes_only_the_target() -> Result<()> {
    let source_node = IrohDocsNode::memory().await?;
    let target_node = IrohDocsNode::memory().await?;
    let commit_gate = Arc::new((Notify::new(), Notify::new()));
    let sink = Arc::new(FakeSink {
        commit_gate: Some(commit_gate.clone()),
        ..FakeSink::default()
    });
    let link = source_node
        .account_transfer()
        .issue(fake_source())?
        .to_link();
    target_node.account_transfer().open(&link, sink.clone())?;
    confirm_both(
        source_node.account_transfer(),
        target_node.account_transfer(),
    )
    .await?;
    timeout(WAIT, commit_gate.0.notified()).await?;
    source_node.account_transfer().cancel();
    commit_gate.1.notify_one();
    let done = wait_for(
        target_node.account_transfer(),
        "target completed",
        is_completed,
    )
    .await?;
    assert!(matches!(
        done,
        Status::Completed {
            account_id: Some(_),
            ..
        }
    ));
    assert_eq!(sink.log.lock().unwrap().committed, 1);
    assert_eq!(source_node.account_transfer().status(), Status::Idle);
    Ok(())
}

/// 形の崩れた frame を送る移行元（証明を確かめずに承認し、確認の後に `frames` をそのまま書く）。
#[derive(Clone, Debug)]
struct RawSource(Arc<Vec<u8>>);

impl ProtocolHandler for RawSource {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let _ = async {
            let (mut send, mut recv) = connection.accept_bi().await?;
            let mut proof = [0; 32];
            recv.read_exact(&mut proof).await?;
            send.write_all(&[ACCEPTED, ACCEPTED]).await?;
            send.finish()?;
            recv.read_to_end(1).await?;
            let (mut send, _recv) = connection.open_bi().await?;
            send.write_all(&self.0).await?;
            send.finish()?;
            connection.closed().await;
            anyhow::Ok(())
        }
        .await;
        Ok(())
    }
}

fn framed(frame: &serde_json::Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(frame).unwrap();
    [(bytes.len() as u32).to_be_bytes().to_vec(), bytes].concat()
}

/// 2a: 移行先は frame の大きさ・件数・順序・総数を確かめ、外れたら保存を確定せずに失敗にする。
#[tokio::test]
async fn malformed_bundles_are_never_committed() -> Result<()> {
    let key = framed(&serde_json::json!({ "frame": "key", "secret": "11".repeat(32) }));
    let items = |n: usize| {
        framed(&serde_json::json!({
            "frame": "items",
            "items": vec![serde_json::to_value(item(0, 64)).unwrap(); n],
        }))
    };
    let end = |count: u64| framed(&serde_json::json!({ "frame": "end", "count": count }));
    let cases = [
        [items(1), key.clone(), end(1)].concat(),
        [key.clone(), items(2), end(3)].concat(),
        [key.clone(), key.clone()].concat(),
        [
            key.clone(),
            items(MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS + 1),
            end(65),
        ]
        .concat(),
        [
            key.clone(),
            ((MAX_ACCOUNT_TRANSFER_FRAME_BYTES + 1) as u32)
                .to_be_bytes()
                .to_vec(),
        ]
        .concat(),
    ];
    for bytes in cases {
        let raw = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>()?)?
            .bind()
            .await?;
        let _router = iroh::protocol::Router::builder(raw.clone())
            .accept(ACCOUNT_TRANSFER_ALPN, RawSource(Arc::new(bytes)))
            .spawn();
        let invite = AccountTransferInvite::issue(
            raw.id().to_string(),
            None,
            raw.addr().ip_addrs().copied(),
            now_ms(),
        )?;
        let target = IrohDocsNode::memory().await?;
        let sink = fake_sink();
        target
            .account_transfer()
            .open(&invite.to_link(), sink.clone())?;
        wait_for(target.account_transfer(), "code", |s| code_of(s).is_some()).await?;
        target.account_transfer().decide(true)?;
        wait_for(
            target.account_transfer(),
            "invalid bundle",
            failed(Failure::Invalid),
        )
        .await?;
        assert_eq!(sink.log.lock().unwrap().committed, 0);
    }
    Ok(())
}
