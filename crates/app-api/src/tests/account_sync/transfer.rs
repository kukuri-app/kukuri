//! #1211 AC-2（2b・2d）: 移行元の必須 bundle（手元の account の replica を周回と同じ木で辿る page）と、移行先の
//! item 単位の merge。

use super::fetch::{memory_device, trust, trusted};
use super::*;
use kukuri_core::{AccountTransferItem, ChannelAudienceKind, CreatePrivateChannelInput, TopicId};

/// 移行元の bundle の全 page。
async fn bundle(app: &AppService) -> Vec<Vec<AccountTransferItem>> {
    let (mut pages, mut cursor) = (Vec::new(), None);
    loop {
        let (items, next) = app.account_transfer_page(cursor).await.expect("page");
        pages.push(items);
        let Some(next) = next else { return pages };
        cursor = Some(next);
    }
}

fn keys_of(pages: &[Vec<AccountTransferItem>]) -> Vec<String> {
    pages
        .iter()
        .flatten()
        .map(|item| item.key.clone())
        .collect()
}

/// 2b: bundle は allowlist の各種類（tombstone を含む）を持ち、変更の窓を持たない。同じアカウントの端末へ merge すると、
/// 表示例外・profile・参加中の channel と現在の世代がそろう。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bundle_carries_every_transferred_kind_and_merges_on_a_new_device() {
    let keys = generate_keys();
    let (source, _) = memory_device(&keys, "device-a").await;
    let authors = trust(&source, 2).await;
    source
        .set_trust_always_visible(&authors[0], false)
        .await
        .expect("untrust");
    source
        .set_my_profile(ProfileInput {
            name: Some("moved".into()),
            ..Default::default()
        })
        .await
        .expect("profile");
    source
        .restore_joined_private_channels()
        .await
        .expect("restore");
    let topic = "kukuri:topic:account-transfer";
    let _ = source.list_timeline(topic, None, 20).await;
    let channel = source
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "moved".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create")
        .channel_id;

    let pages = bundle(&source).await;
    let keys_sent = keys_of(&pages);
    let channel_hex = hex::encode(channel.as_str());
    for expected in [
        "profile".to_string(),
        format!("trust/always-visible/{}", authors[0]),
        format!("trust/always-visible/{}", authors[1]),
        format!("channel/{channel_hex}/membership"),
    ] {
        assert!(keys_sent.contains(&expected), "{expected} in {keys_sent:?}");
    }
    assert!(
        keys_sent
            .iter()
            .any(|key| key.starts_with(&format!("channel/{channel_hex}/epoch/")))
    );
    assert!(!keys_sent.iter().any(|key| key.starts_with("changes/")));

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
    assert_eq!(trusted(&target).await, vec![authors[1].clone()]);
    assert_eq!(
        target
            .get_my_profile()
            .await
            .expect("profile")
            .name
            .as_deref(),
        Some("moved")
    );
    let epoch = async |app: &AppService| {
        app.joined_private_channel_state(topic, channel.as_str())
            .await
            .expect("joined")
            .current_epoch_id
    };
    assert_eq!(epoch(&target).await, epoch(&source).await);
}

/// 2b: 投稿を 10 倍にしても、bundle の page の数・item・bytes は変わらない（投稿・履歴は bundle に入らない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bundle_does_not_grow_with_posts() {
    let mut sizes = Vec::new();
    for posts in [3, 30] {
        let (source, _) = memory_device(&generate_keys(), "device-a").await;
        trust(&source, 5).await;
        for index in 0..posts {
            source
                .create_post(
                    "kukuri:topic:transfer-posts",
                    &format!("post {index}"),
                    None,
                )
                .await
                .expect("post");
        }
        let pages = bundle(&source).await;
        let bytes: usize = pages
            .iter()
            .flatten()
            .map(|item| serde_json::to_vec(item).expect("item").len())
            .sum();
        sizes.push((pages.len(), keys_of(&pages).len(), bytes));
    }
    assert_eq!(sizes[0], sizes[1]);
}

/// 2d: 登録済みの同じアカウントへの merge は item ごとの版で採り、手元の新しい版を古い bundle で戻さない。別の
/// アカウントの bundle は開けず、何も採らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merging_keeps_newer_local_versions_and_isolates_other_accounts() {
    let keys = generate_keys();
    let (source, _) = memory_device(&keys, "device-a").await;
    let (target, _) = memory_device(&keys, "device-b").await;
    let shared = trust(&source, 1).await;
    // 移行先では、移行元の版より後に解除した。
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    target
        .set_trust_always_visible(&shared[0], false)
        .await
        .expect("newer local tombstone");
    let only_source = trust(&source, 1).await;
    let pages = bundle(&source).await;
    for page in pages.clone() {
        target
            .merge_account_transfer_items(page)
            .await
            .expect("merge");
    }
    assert_eq!(trusted(&target).await, only_source);

    let (other, _) = memory_device(&generate_keys(), "device-o").await;
    let items: Vec<_> = pages.into_iter().flatten().collect();
    assert!(!items.is_empty());
    assert!(other.merge_account_transfer_items(items).await.is_err());
    assert!(trusted(&other).await.is_empty());
}
