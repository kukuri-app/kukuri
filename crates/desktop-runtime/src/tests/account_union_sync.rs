//! #1650: 本人の端末どうしの和集合の同期を実 runtime の 2 端末で通す。同じアカウントを使う 2 台が同期を始めると、
//! Community Node の rendezvous で互いを見つけてつながり、両端末のフォロー・ブロック（相手ごとに新しい操作）・フォロワー・
//! 自分の投稿の記録（添付の blob つき）が両端末の和集合になる。W5 の自動同期が運ばない旧データ（同期の行の無い edge）と
//! フォロワーも届く。

use std::collections::BTreeSet;

use super::account_transfer::{eventually, runtime_at, target_host, transfer};
use super::account_transfer_history::own_records_since;
use super::account_transfer_sync::{Presence, community_node, consent, converge, social};
use super::*;
use crate::accounts::account_db_path;
use kukuri_app_api::SocialConnectionKind;
use kukuri_core::{
    AccountTransferRole, AccountTransferStatus, EnvelopeId, FollowEdgeStatus, KukuriEnvelope,
    KukuriFollowEdgeEnvelopeContentV1, KukuriKeys, Pubkey, build_follow_edge_envelope,
    sign_envelope_json_at,
};
use kukuri_store::ObjectProjectionStore;

/// 時刻を決めた自分の follow の edge の envelope（同期の行を作らない旧データとして store へ置く）。
fn follow_at(
    keys: &KukuriKeys,
    target: &str,
    status: FollowEdgeStatus,
    created_at: i64,
) -> KukuriEnvelope {
    let subject = keys.public_key();
    let content = KukuriFollowEdgeEnvelopeContentV1 {
        subject_pubkey: subject.clone(),
        target_pubkey: Pubkey::from(target),
        status,
    };
    let tags = vec![
        vec!["subject".into(), subject.as_str().to_string()],
        vec!["target".into(), target.to_string()],
        vec!["object".into(), "follow-edge".into()],
    ];
    sign_envelope_json_at(keys, "follow-edge", tags, &content, created_at).unwrap()
}

async fn connections(runtime: &DesktopRuntime, kind: SocialConnectionKind) -> BTreeSet<String> {
    social(runtime, kind).await.into_iter().collect()
}

async fn synced(runtime: &DesktopRuntime) -> bool {
    matches!(
        runtime.account_transfer_status().await.unwrap(),
        AccountTransferStatus::Completed {
            role: AccountTransferRole::Sync,
            history: Some(ref result),
            ..
        } if result.stopped.is_none()
    )
}

/// 自分の record に `post` の投稿の envelope がある。
async fn holds_post(runtime: &DesktopRuntime, post: &str) -> bool {
    let key = format!("objects/{post}/envelope");
    own_records_since(&runtime.sqlite, 0)
        .await
        .iter()
        .any(|record| record.1 == key)
}

/// T3: 各端末だけにあるフォロー・フォロワー・投稿と、片方の新しい解除が、両端末で同じ和集合になる（新しい解除は戻らない）。
#[tokio::test]
async fn own_devices_merge_their_social_graph_and_posts_into_the_union() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let presence = Arc::new(Presence::default());
    let nodes = [
        community_node(presence.clone()).await,
        community_node(presence).await,
    ];
    // a は最初のアカウントを使う端末。b は a のアカウントを移して使う端末。
    let host_a = target_host(&dir.path().join("a")).await;
    let a = host_a.runtime();
    let b_dir = dir.path().join("b");
    let host_b = target_host(&b_dir).await;
    let id = transfer(&a, &host_b).await;
    host_b
        .replace_runtime(Arc::new(runtime_at(account_db_path(&b_dir, &id)).await))
        .await
        .unwrap()
        .shutdown()
        .await;
    let b = host_b.runtime();
    let keys = a.author_keys.clone();
    let me = keys.public_key();

    // 同期の行の無い edge: a は `unfollowed` を古くフォローし、b は新しく解除した。`only_a`・`only_b` は片方だけ。
    let [only_a, only_b, unfollowed] = [(); 3].map(|_| KukuriKeys::generate().public_key_hex());
    let edges = [
        (
            &a,
            follow_at(&keys, &unfollowed, FollowEdgeStatus::Active, 1_000),
        ),
        (
            &a,
            follow_at(&keys, &only_a, FollowEdgeStatus::Active, 1_000),
        ),
        (
            &b,
            follow_at(&keys, &only_b, FollowEdgeStatus::Active, 1_000),
        ),
        (
            &b,
            follow_at(&keys, &unfollowed, FollowEdgeStatus::Revoked, 2_000),
        ),
    ];
    for (runtime, envelope) in edges {
        runtime.store.put_envelope(envelope).await.unwrap();
    }
    // フォロワー（相手の署名の edge）は、それぞれの端末だけが受けた。
    let [follower_a, follower_b] = [(); 2].map(|_| KukuriKeys::generate());
    for (runtime, follower) in [(&a, &follower_a), (&b, &follower_b)] {
        let edge = build_follow_edge_envelope(follower, &me, FollowEdgeStatus::Active).unwrap();
        runtime.store.put_envelope(edge).await.unwrap();
    }
    let topic = "kukuri:topic:account-union-sync";
    let post = |content: &str, attachments| CreatePostRequest {
        topic: topic.into(),
        content: content.into(),
        reply_to: None,
        channel_ref: ChannelRef::Public,
        attachments,
        content_labels: Vec::new(),
    };
    let image = vec![3u8; 64 * 1024];
    let attached = image_attachment_request("a.png", "image/png", &image);
    let post_a = a.create_post(post("from a", vec![attached])).await.unwrap();
    let post_b = b.create_post(post("from b", Vec::new())).await.unwrap();
    let picture = a
        .sqlite
        .get_object_projection(&EnvelopeId::from(post_a.as_str()))
        .await
        .unwrap()
        .unwrap()
        .attachments[0]
        .hash
        .as_str()
        .to_string();

    for (runtime, node) in [(&a, &nodes[0]), (&b, &nodes[1])] {
        consent(runtime, node).await;
    }
    host_a.start_account_union_sync().await.unwrap();
    host_b.start_account_union_sync().await.unwrap();
    // rendezvous の応答で互いを見つけてつながり、両方の向きを受けて完了する。
    converge("both devices complete the sync", [&a, &b], async || {
        synced(&a).await && synced(&b).await
    })
    .await;

    let following = BTreeSet::from([only_a, only_b]);
    let followers = BTreeSet::from([follower_a, follower_b].map(|key| key.public_key_hex()));
    for (runtime, post) in [(&a, &post_b), (&b, &post_a)] {
        eventually("the union reaches the device", async || {
            connections(runtime, SocialConnectionKind::Following).await == following
                && connections(runtime, SocialConnectionKind::Followed).await == followers
                && holds_post(runtime, post).await
        })
        .await;
    }
    eventually("the attachment reaches b", async || {
        b.sqlite
            .get_remote_content("blob", &picture)
            .await
            .unwrap()
            .is_some_and(|bytes| bytes == image)
    })
    .await;

    host_a.shutdown().await;
    host_b.shutdown().await;
}
