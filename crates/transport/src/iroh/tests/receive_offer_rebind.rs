//! 送り手が同じ EndpointId で通信の stack を作り直した後の受信の offer（#1549）。
use super::*;

use kukuri_core::{
    BlobHash, KukuriKeys, ReceiveOfferReferenceV1, ReceiveOfferScopeV1, seal_receive_offer,
};

/// 受け手の account route へ 1 件送り、`limit` の間に届いたかを返す。
async fn offer_arrives(
    left: &IrohGossipTransport,
    right: &IrohGossipTransport,
    recipient: &KukuriKeys,
    incoming: &mut ReceiveOfferStream,
    limit: Duration,
) -> std::result::Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();
    let offer = seal_receive_offer(
        &KukuriKeys::generate(),
        &recipient.public_key(),
        ReceiveOfferReferenceV1 {
            provider_endpoint_id: left.endpoint.id().to_string(),
            payload_hash: BlobHash(blake3::hash(b"payload").to_hex().to_string()),
            payload_bytes: 7,
            scope: ReceiveOfferScopeV1::PublicSource,
        },
        now,
        now + 60_000,
    )
    .unwrap();
    match timeout(
        limit,
        left.publish_receive_offer(&recipient.public_key(), right.endpoint.addr(), offer),
    )
    .await
    {
        Err(_) => return Err(format!("publish timed out after {limit:?}")),
        Ok(Err(error)) => return Err(format!("publish failed: {error}")),
        Ok(Ok(())) => {}
    }
    match timeout(limit, incoming.next()).await {
        Ok(Some(envelope)) => {
            assert_eq!(envelope.source_peer, left.endpoint.id().to_string());
            Ok(())
        }
        Ok(None) => Err("offer stream ended".to_string()),
        Err(_) => Err(format!("published but not received within {limit:?}")),
    }
}

/// 送り手（Web）が Community Node の設定の保存で stack を作り直すと、同じ EndpointId の新しい endpoint が相手へつなぐ。
/// 旧 endpoint の終了は、閉じた WebRTC の経路に送られて相手に届かないことがある（#1549 の CI の観測）ので、旧 endpoint を
/// 閉じずに残す。相手の gossip では、旧接続の間に送った Neighbor の返事待ちが残ったまま接続だけが新しい endpoint に
/// 置き換わる。その状態で新しい endpoint の Join に Neighbor が返らないと、受け手の route への join が成立せず、offer の
/// 送信（app-api の待ちは 2 秒）は時間切れを繰り返す。修正前は 5 回とも時間切れだった。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_receive_offer_reaches_a_peer_after_the_sender_rebinds_with_the_same_endpoint_id() {
    let mut right = IrohGossipTransport::bind_local().await.unwrap();
    let secret = SecretKey::generate();
    let left = IrohGossipTransport::bind_local_with_secret(secret.clone())
        .await
        .unwrap();
    left.discovery.add_endpoint_info(right.endpoint.addr());
    right.discovery.add_endpoint_info(left.endpoint.addr());
    let recipient = KukuriKeys::generate();
    let (lease, mut incoming, _) = right
        .subscribe_receive_offers(&recipient.public_key())
        .await
        .unwrap();
    // 2 件目は、送り手がまだ受け手の route の neighbor のまま Join し直す（30 秒の hold の間の送信）。受け手が送る
    // Neighbor の更新を送り手は返事と受け取り、受け手の側に返事待ちが残る。
    for round in 0..2 {
        offer_arrives(
            &left,
            &right,
            &recipient,
            &mut incoming,
            Duration::from_secs(10),
        )
        .await
        .unwrap_or_else(|error| panic!("before the rebuild, round {round}: {error}"));
    }

    // 旧 endpoint の終了が相手に届かないまま、同じ EndpointId で bind し直す。
    std::mem::forget(left);
    let mut left = IrohGossipTransport::bind_local_with_secret(secret)
        .await
        .unwrap();
    left.discovery.add_endpoint_info(right.endpoint.addr());
    right.discovery.add_endpoint_info(left.endpoint.addr());
    for round in 0..3 {
        offer_arrives(
            &left,
            &right,
            &recipient,
            &mut incoming,
            Duration::from_secs(2),
        )
        .await
        .unwrap_or_else(|error| panic!("after the rebuild, round {round}: {error}"));
    }

    right
        .unsubscribe_receive_offers(&recipient.public_key(), lease)
        .await
        .unwrap();
    left.shutdown().await;
    left._router.take().unwrap().shutdown().await.unwrap();
    right.shutdown().await;
    right._router.take().unwrap().shutdown().await.unwrap();
}
