//! #1211 AC-1（1b〜1d）: QR・専用リンクの移行の接続と両端末の確認。
//! 負例では、どちらの端末にも確認済み（AC-2 の転送の唯一の入口）が作られないことを確かめる。

use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use kukuri_core::{ACCOUNT_TRANSFER_LINK_PREFIX, AccountTransferStatus as Status};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};

use super::*;
use crate::{IrohDocsNode, NodeOptions};

const WAIT: Duration = Duration::from_secs(20);

async fn wait_for(
    transfer: &AccountTransfer,
    what: &str,
    done: impl Fn(&Status) -> bool,
) -> Result<Status> {
    timeout(WAIT, async {
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

fn is_confirmed(status: &Status) -> bool {
    matches!(status, Status::Confirmed { .. })
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

/// 両端末で確認コードの一致を確かめて承認し、両方が確認済みになる。
async fn confirm_both(source: &AccountTransfer, target: &AccountTransfer) -> Result<()> {
    let at_source = wait_for(source, "source shows a code", |s| code_of(s).is_some()).await?;
    let at_target = wait_for(target, "target shows a code", |s| code_of(s).is_some()).await?;
    assert_eq!(code_of(&at_source), code_of(&at_target));
    source.decide(true)?;
    target.decide(true)?;
    wait_for(source, "source confirmed", is_confirmed).await?;
    wait_for(target, "target confirmed", is_confirmed).await?;
    Ok(())
}

/// 1b・1d: native↔native は招待の直接 addr で接続し（relay なし = Direct P2P）、両方の承認で確認済みになる。
#[tokio::test]
async fn native_nodes_confirm_over_the_direct_path() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let target = IrohDocsNode::memory().await?;
    let invite = source.account_transfer().issue()?;
    assert!(invite.relay_url.is_none());
    assert!(!invite.direct_addrs.is_empty());
    assert!(matches!(
        source.account_transfer().status(),
        Status::Waiting { .. }
    ));

    target.account_transfer().open(&invite.to_link())?;
    confirm_both(source.account_transfer(), target.account_transfer()).await?;

    // 1c 再生: 使用済みの招待は、同じ端末からも別の端末からも受けない。
    target.account_transfer().open(&invite.to_link())?;
    wait_for(
        target.account_transfer(),
        "replay",
        failed(Failure::Invalid),
    )
    .await?;
    let third = IrohDocsNode::memory().await?;
    third.account_transfer().open(&invite.to_link())?;
    wait_for(third.account_transfer(), "replay", failed(Failure::Invalid)).await?;
    assert!(is_confirmed(&source.account_transfer().status()));
    Ok(())
}

/// 1c 改竄・未認証: 秘密を変えたリンクと、証明の誤った接続は拒否する。待っている招待は消費されず、正しいリンクで続けられる。
#[tokio::test]
async fn a_wrong_proof_is_rejected_without_consuming_the_invite() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let link = source.account_transfer().issue()?.to_link();
    let forger = IrohDocsNode::memory().await?;
    forger.account_transfer().open(&tamper(&link, |json| {
        json["secret"] = "ab".repeat(32).into()
    }))?;
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
    target.account_transfer().open(&link)?;
    confirm_both(source.account_transfer(), target.account_transfer()).await
}

/// 1c 期限切れ: 期限の正本は移行元。移行先のリンクの期限を延ばしても、移行元の期限を過ぎた招待は受けない。
#[tokio::test]
async fn an_invite_expired_at_the_source_is_rejected() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let link = source.account_transfer().issue()?.to_link();
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
    target.account_transfer().open(&extended)?;
    wait_for(
        target.account_transfer(),
        "expired",
        failed(Failure::Invalid),
    )
    .await?;
    // 移行先の手元でも、期限を過ぎたリンクは開く前に拒否する。
    assert!(target.account_transfer().open(&short).is_err());
    Ok(())
}

/// 1c 接続先違い: 別の端末を指すリンクは、その端末の秘密と合わず拒否される。id と addr が食い違うリンクは接続できない。
#[tokio::test]
async fn a_link_pointing_at_another_device_is_rejected() -> Result<()> {
    let source = IrohDocsNode::memory().await?;
    let other = IrohDocsNode::memory().await?;
    let link = source.account_transfer().issue()?.to_link();
    let other_link = other.account_transfer().issue()?.to_link();
    let other_invite = AccountTransferInvite::parse_link(&other_link, now_ms())?;

    let target = IrohDocsNode::memory().await?;
    let to_other = tamper(&link, |json| {
        json["issuer"] = other_invite.issuer.clone().into();
        json["direct_addrs"] = serde_json::to_value(&other_invite.direct_addrs).unwrap();
    });
    target.account_transfer().open(&to_other)?;
    wait_for(
        target.account_transfer(),
        "other device",
        failed(Failure::Invalid),
    )
    .await?;

    let mismatched = tamper(&link, |json| {
        json["direct_addrs"] = serde_json::to_value(&other_invite.direct_addrs).unwrap();
    });
    target.account_transfer().open(&mismatched)?;
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
        let link = source.account_transfer().issue()?.to_link();
        target.account_transfer().open(&link)?;
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
    target
        .account_transfer()
        .open(&source.account_transfer().issue()?.to_link())?;
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

/// 1d: relay だけで届くブラウザ相当の端（交渉を始める側）は、ICE の完了を待たずに relay で確認済みになる（Relay Fallback）。
#[tokio::test]
async fn a_browser_like_device_confirms_over_the_relay() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let node = async |browser_like| -> Result<Arc<IrohDocsNode>> {
        let node = IrohDocsNode::memory_with(NodeOptions {
            relay_config: TransportRelayConfig {
                iroh_relay_urls: vec![relay.to_string()],
            },
            webrtc: Some(WebRtcTransport::new(WebRtcConfig {
                bind_ip: Ipv4Addr::LOCALHOST.into(),
            })),
            browser_like,
            ..NodeOptions::default()
        })
        .await?;
        node.endpoint().online().await;
        Ok(node)
    };
    let source = node(false).await?;
    let web = node(true).await?;
    let invite = source.account_transfer().issue()?;
    assert_eq!(
        invite.relay_url.as_deref(),
        Some(relay.to_string().as_str())
    );
    web.account_transfer().open(&invite.to_link())?;
    confirm_both(source.account_transfer(), web.account_transfer()).await
}
