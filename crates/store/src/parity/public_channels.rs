//! #1840: 公開 channel の一覧の行。2 つの並び順の cursor 走査、最後の投稿の時刻の更新（古い時刻では戻らない）、
//! 置き直しで最後の投稿の時刻を保つこと、topic で分かれることを、両方の実装で同じにする。

use super::*;

const TOPIC: &str = "kukuri:topic:parity-public-channels";

fn channel(id: &str, topic: &str, created_at: i64, last_post_at: i64) -> PublicChannelRow {
    PublicChannelRow {
        channel_id: id.into(),
        topic_id: topic.into(),
        title: format!("title {id}"),
        creator_pubkey: "a".repeat(64),
        created_at,
        first_post_id: (created_at % 200 == 0).then(|| format!("first-{id}")),
        last_post_at,
    }
}

async fn all_pages<S: Store + ProjectionStore>(
    store: &S,
    topic: &str,
    order: PublicChannelOrder,
) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_PAGES {
        let page = PublicChannelStore::list_public_channels(store, topic, order, cursor, 2)
            .await
            .expect("PublicChannelStore::list_public_channels");
        pages.push(
            page.items
                .iter()
                .map(|row| format!("{}@{}", row.channel_id, row.sort_at(order)))
                .collect(),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    pages
}

#[derive(Debug, PartialEq)]
struct PublicChannelScenarioResult {
    last_post: Vec<Vec<String>>,
    created: Vec<Vec<String>>,
    other_topic: Vec<Vec<String>>,
    one: Option<PublicChannelRow>,
    missing: Option<PublicChannelRow>,
}

async fn public_channel_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> PublicChannelScenarioResult {
    for (row, remote) in [
        (channel("pc-own", TOPIC, 100, 100), false),
        (channel("pc-remote-1", TOPIC, 200, 200), true),
        (channel("pc-remote-2", TOPIC, 300, 150), true),
        (
            channel("pc-other", "kukuri:topic:parity-other", 400, 400),
            true,
        ),
    ] {
        PublicChannelStore::put_public_channel(store, row, remote)
            .await
            .expect("PublicChannelStore::put_public_channel");
    }
    for (channel_id, posted_at) in [("pc-own", 500), ("pc-remote-2", 120), ("pc-missing", 900)] {
        PublicChannelStore::note_public_channel_post(store, channel_id, posted_at)
            .await
            .expect("PublicChannelStore::note_public_channel_post");
    }
    // 置き直しても、最後の投稿の時刻は新しい方が残る。
    PublicChannelStore::put_public_channel(store, channel("pc-remote-1", TOPIC, 200, 50), true)
        .await
        .expect("PublicChannelStore::put_public_channel again");
    PublicChannelScenarioResult {
        last_post: all_pages(store, TOPIC, PublicChannelOrder::LastPost).await,
        created: all_pages(store, TOPIC, PublicChannelOrder::Created).await,
        other_topic: all_pages(
            store,
            "kukuri:topic:parity-other",
            PublicChannelOrder::LastPost,
        )
        .await,
        one: PublicChannelStore::get_public_channel_row(store, "pc-remote-2")
            .await
            .expect("PublicChannelStore::get_public_channel_row"),
        missing: PublicChannelStore::get_public_channel_row(store, "pc-missing")
            .await
            .expect("PublicChannelStore::get_public_channel_row missing"),
    }
}

pub(super) async fn public_channel_lists_match_between_backends<S, F>(make: &F)
where
    S: Store + ProjectionStore,
    F: AsyncFn() -> S,
{
    let backend = public_channel_scenario(&make().await).await;
    let memory = public_channel_scenario(&MemoryStore::default()).await;
    assert_eq!(backend, memory);
    assert_eq!(
        backend.last_post,
        vec![
            vec!["pc-own@500".to_string(), "pc-remote-1@200".to_string()],
            vec!["pc-remote-2@150".to_string()],
        ]
    );
    assert_eq!(
        backend.created,
        vec![
            vec!["pc-remote-2@300".to_string(), "pc-remote-1@200".to_string()],
            vec!["pc-own@100".to_string()],
        ]
    );
    assert_eq!(backend.other_topic, vec![vec!["pc-other@400".to_string()]]);
    assert_eq!(
        backend
            .one
            .as_ref()
            .map(|row| (row.last_post_at, row.first_post_id.clone())),
        Some((150, None))
    );
    assert!(backend.missing.is_none());
}
