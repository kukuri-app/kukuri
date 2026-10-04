//! #1211 AC-6: 自分のフォロー・ブロックの edge と、自分がフォローしている相手から自分への edge を、移行の必須 bundle と
//! 本人の端末間の同期で運ぶ。

use super::fetch::memory_device;
use super::transfer::{bundle, keys_of};
use super::*;
use kukuri_core::{
    BlockEdgeStatus, ChannelAudienceKind, CreatePrivateChannelInput, FollowEdgeStatus, TopicId,
    build_block_edge_envelope, build_follow_edge_envelope,
};

async fn connections(app: &AppService, kind: SocialConnectionKind) -> BTreeSet<String> {
    app.list_social_connections(kind)
        .await
        .expect("connections")
        .into_iter()
        .map(|view| view.author_pubkey)
        .collect()
}

async fn put(app: &AppService, envelope: KukuriEnvelope) {
    app.services
        .store
        .put_envelope(envelope)
        .await
        .expect("edge");
}

/// 時刻を決めた follow の edge の envelope（`build_follow_edge_envelope` と同じ形）。
fn follow_at(
    keys: &KukuriKeys,
    target: &Pubkey,
    status: FollowEdgeStatus,
    created_at: i64,
) -> KukuriEnvelope {
    let subject = keys.public_key();
    let content = kukuri_core::KukuriFollowEdgeEnvelopeContentV1 {
        subject_pubkey: subject.clone(),
        target_pubkey: target.clone(),
        status,
    };
    let tags = vec![
        vec!["subject".into(), subject.as_str().to_string()],
        vec!["target".into(), target.as_str().to_string()],
        vec!["object".into(), "follow-edge".into()],
    ];
    kukuri_core::sign_envelope_json_at(keys, "follow-edge", tags, &content, created_at)
        .expect("edge")
}

/// 移行の bundle は、AC-6 より前からある自分のフォロー（64 件を超える）・ブロックと、相互フォローの相手から自分への
/// edge を、64 件以下の page で channel の item より先に運ぶ。移行先では、一覧と相互フォローが移行元とそろい、相互
/// フォローの相手へ DM を送れる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bundle_carries_own_edges_and_the_follow_backs() {
    let keys = generate_keys();
    let me = keys.public_key();
    let (source, _) = memory_device(&keys, "device-a").await;
    let peers: Vec<KukuriKeys> = (0..70).map(|_| generate_keys()).collect();
    // AC-6 より前の edge（同期の item が無い）と、この端末の操作の edge。
    for peer in &peers[..66] {
        put(
            &source,
            build_follow_edge_envelope(&keys, &peer.public_key(), FollowEdgeStatus::Active)
                .expect("follow"),
        )
        .await;
    }
    for peer in &peers[66..] {
        source
            .follow_author(peer.public_key().as_str())
            .await
            .expect("follow");
    }
    source
        .unfollow_author(peers[69].public_key().as_str())
        .await
        .expect("unfollow");
    // 相互フォローの相手（相手の署名の edge を受けた）。
    for peer in &peers[..3] {
        put(
            &source,
            build_follow_edge_envelope(peer, &me, FollowEdgeStatus::Active).expect("follow back"),
        )
        .await;
    }
    let blocked = [generate_keys().public_key(), generate_keys().public_key()];
    put(
        &source,
        build_block_edge_envelope(&keys, &blocked[0], BlockEdgeStatus::Active).expect("block"),
    )
    .await;
    source
        .block_author(blocked[1].as_str())
        .await
        .expect("block");
    source
        .restore_joined_private_channels()
        .await
        .expect("restore");
    let topic = "kukuri:topic:account-transfer-edges";
    let _ = source.list_timeline(topic, None, 20).await;
    source
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "edges".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create");

    let pages = bundle(&source).await;
    assert!(pages.iter().all(|page| page.len() <= 64));
    let keys_sent = keys_of(&pages);
    let count = |prefix: &str| {
        keys_sent
            .iter()
            .filter(|key| key.starts_with(prefix))
            .count()
    };
    assert_eq!(
        (
            count("graph/follows/"),
            count("graph/followers/"),
            count("graph/blocks/")
        ),
        (70, 3, 2)
    );
    let last_edge = keys_sent
        .iter()
        .rposition(|key| key.starts_with("graph/"))
        .expect("edges");
    let first_channel = keys_sent
        .iter()
        .position(|key| key.starts_with("channel/"))
        .expect("channel");
    assert!(
        last_edge < first_channel,
        "the edges come before the channels"
    );

    let (target, _) = memory_device(&keys, "device-b").await;
    target
        .restore_joined_private_channels()
        .await
        .expect("restore");
    for page in pages {
        target
            .merge_account_transfer_items(page)
            .await
            .expect("merge");
    }
    let following = connections(&source, SocialConnectionKind::Following).await;
    assert_eq!(following.len(), 69);
    assert_eq!(
        connections(&target, SocialConnectionKind::Following).await,
        following
    );
    assert_eq!(
        connections(&target, SocialConnectionKind::Blocking).await,
        blocked
            .iter()
            .map(|pubkey| pubkey.as_str().to_string())
            .collect()
    );
    for peer in &peers[..3] {
        assert!(
            target
                .services
                .projection_store
                .get_author_relationship(me.as_str(), peer.public_key().as_str())
                .await
                .expect("relationship")
                .is_some_and(|relationship| relationship.mutual),
            "the follow back reached the target"
        );
    }
    target
        .send_direct_message(
            peers[0].public_key().as_str(),
            Some("hello from the moved device"),
            None,
            Vec::new(),
        )
        .await
        .expect("a mutual follower can be messaged");
}

/// 片方の端末のフォロー・解除・ブロックは、もう片方の一覧に届き、画面に相手と自分の profile の読み直しを知らせる。
/// 手元の新しい edge は古い版で戻らず（台帳の行も採らない）、別のアカウントの edge は採らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_edges_reach_the_other_device_and_keep_newer_local_edges() {
    let keys = generate_keys();
    let me = keys.public_key();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, b_docs) = memory_device(&keys, "device-b").await;
    b_docs.reading_from(&a_docs);
    let (followed, blocked) = (generate_keys().public_key(), generate_keys().public_key());
    a.follow_author(followed.as_str()).await.expect("follow");
    a.block_author(blocked.as_str()).await.expect("block");
    let mut changes = b.subscribe_author_relationship_changes();
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert_eq!(
        connections(&b, SocialConnectionKind::Following).await,
        BTreeSet::from([followed.as_str().to_string()])
    );
    assert_eq!(
        connections(&b, SocialConnectionKind::Blocking).await,
        BTreeSet::from([blocked.as_str().to_string()])
    );
    let mut announced = BTreeSet::new();
    while let Ok(pubkey) = changes.try_recv() {
        announced.insert(pubkey);
    }
    assert!(announced.contains(followed.as_str()) && announced.contains(me.as_str()));
    a.unfollow_author(followed.as_str())
        .await
        .expect("unfollow");
    b.fetch_account_sync_from("device-a").await.expect("fetch");
    assert!(
        connections(&b, SocialConnectionKind::Following)
            .await
            .is_empty()
    );

    let other = generate_keys().public_key();
    let older = follow_at(&keys, &other, FollowEdgeStatus::Active, 1_000);
    put(
        &b,
        follow_at(&keys, &other, FollowEdgeStatus::Revoked, 2_000),
    )
    .await;
    assert!(
        !b.merge_account_sync_item(AccountSyncItem::edge(&me, &older).expect("item"))
            .await
            .expect("merge")
    );
    assert_eq!(
        b.services
            .store
            .get_follow_edge(me.as_str(), other.as_str())
            .await
            .expect("edge")
            .map(|edge| edge.status),
        Some(FollowEdgeStatus::Revoked)
    );
    assert!(
        b.services
            .projection_store
            .get_account_sync_row(&format!("graph/follows/{}", other.as_str()))
            .await
            .expect("row")
            .is_none()
    );
    let foreign = build_follow_edge_envelope(&generate_keys(), &other, FollowEdgeStatus::Active)
        .expect("foreign");
    assert!(AccountSyncItem::edge(&me, &foreign).is_err());
    let forged = AccountSyncItem {
        key: AccountSyncItemKey::OwnFollow {
            target: other.clone(),
        },
        op_id: foreign.id.as_str()[..32].to_string(),
        updated_at: foreign.created_at,
        value: Some(serde_json::to_value(&foreign).expect("value")),
    };
    assert!(b.merge_account_sync_item(forged).await.is_err());
}

/// 自分のフォローを 10 倍にしても、新しい 1 件の取得の読取りの回数と bytes は増えない（変更の窓の seq の桁がそろう
/// 規模で比べる。seq は 10 と 91）。周回の 1 照会の読取りは page の上限を超えない（全件を 1 照会で読むと、200 件で
/// 上限を超える）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_follows_do_not_grow_a_cycle_page_or_one_change_fetch() {
    let mut fetches = Vec::new();
    for follows in [9, 90] {
        let keys = generate_keys();
        let (a, a_docs) = memory_device(&keys, "device-a").await;
        let (b, b_docs) = memory_device(&keys, "device-b").await;
        b_docs.reading_from(&a_docs);
        for _ in 0..follows {
            a.follow_author(generate_keys().public_key().as_str())
                .await
                .expect("follow");
        }
        b.fetch_account_sync_from("device-a")
            .await
            .expect("catch up");
        a.follow_author(generate_keys().public_key().as_str())
            .await
            .expect("follow");
        a_docs.work();
        b.fetch_account_sync_from("device-a").await.expect("fetch");
        let (reads, bytes, _) = a_docs.work();
        fetches.push((reads, bytes));
    }
    assert_eq!(fetches[0], fetches[1]);

    let keys = generate_keys();
    let (a, a_docs) = memory_device(&keys, "device-a").await;
    let (b, _) = memory_device(&keys, "device-b").await;
    for _ in 0..200 {
        a.follow_author(generate_keys().public_key().as_str())
            .await
            .expect("follow");
    }
    let (mut prefix, mut page) = (Some("profile".to_string()), 0);
    while let Some(current) = prefix {
        a_docs.work();
        prefix = b
            .account_sync_cycle_step(&a_docs, &current, false)
            .await
            .expect("cycle step");
        page = page.max(a_docs.work().0);
    }
    // 1 照会（64 key）と、key ごとの点読（組の 1 件か旧候補）。
    assert!(page <= 1 + 2 * 64, "{page} reads in a page");
}

/// 自分のフォローを 10 倍にしても、移行の 1 page の item の数は変わらない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bundle_edge_pages_do_not_grow_with_follows() {
    let mut largest = Vec::new();
    for follows in [70, 700] {
        let keys = generate_keys();
        let (source, _) = memory_device(&keys, "device-a").await;
        for _ in 0..follows {
            put(
                &source,
                build_follow_edge_envelope(
                    &keys,
                    &generate_keys().public_key(),
                    FollowEdgeStatus::Active,
                )
                .expect("follow"),
            )
            .await;
        }
        let pages = bundle(&source).await;
        let edges = pages
            .iter()
            .filter(|page| page.iter().any(|item| item.key.starts_with("graph/")))
            .map(Vec::len)
            .collect::<Vec<_>>();
        assert_eq!(edges.iter().sum::<usize>(), follows);
        largest.push(edges.into_iter().max());
    }
    assert_eq!(largest[0], largest[1]);
}
