//! #1888 AC-1: 公開スレッドの作成者がタイトルを変え、スレッドを削除する。topic の lease を持つ端末は知らせで、知らせを
//! 受けない端末は ID から読んだ後の背景の読み直しで、行より新しい版だけを反映する。削除したスレッドは一覧に出ず、ID から
//! は削除済みとして返る。作成者以外は変えられず、削除の後は変えられない。
use super::public_channels::{TOPIC, device, device_with_store};
use super::*;
use crate::PublicChannelView;
use kukuri_docs_sync::MemoryDocsSync;
use kukuri_store::{PublicChannelOrder, PublicChannelStore};

/// topic のタイムラインの列を開く(topic の lease を取り、topic の hint を受ける)。
async fn open_topic(app: &AppService) {
    app.set_scope_display(crate::ScopeDisplayRequest {
        observer: "test-topic".into(),
        target: crate::ScopeDisplayTarget::Timeline {
            topic: TOPIC.into(),
            scope: TimelineScope::Public,
        },
        visible: true,
    })
    .await
    .expect("open the topic column");
}

/// `done` が満たされるまで(期限 10 秒)ID から取得し直す。
async fn got_until(
    app: &AppService,
    channel_id: &str,
    done: impl Fn(&PublicChannelView) -> bool,
) -> PublicChannelView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let channel = app
            .get_public_channel(TOPIC, channel_id)
            .await
            .expect("get the public channel")
            .expect("the public channel");
        if done(&channel) || tokio::time::Instant::now() >= deadline {
            return channel;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn listed(app: &AppService) -> Vec<String> {
    app.list_public_channels(TOPIC, PublicChannelOrder::Created, None, 20)
        .await
        .expect("list the public channels")
        .items
        .into_iter()
        .map(|channel| channel.title)
        .collect()
}

#[tokio::test]
async fn a_device_with_the_topic_lease_follows_the_creators_rename_and_deletion() {
    let a_docs = MemoryDocsSync::with_docs_author("11".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("12".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let (b, b_store) = device_with_store("b", &network, b_docs, a_docs, blobs);
    open_topic(&b).await;
    let channel = a
        .create_public_channel(TOPIC, "雑談", "最初の投稿", Vec::new(), Vec::new())
        .await
        .expect("create");
    let row = async |done: &dyn Fn(&kukuri_store::PublicChannelRow) -> bool| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let row = b_store
                .get_public_channel_row(&channel.channel_id)
                .await
                .expect("row");
            if row.as_ref().is_some_and(done) || tokio::time::Instant::now() >= deadline {
                return row.expect("the row on B");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    assert_eq!(row(&|_| true).await.title, "雑談");

    // 作成者以外は変えられない。
    assert!(
        b.update_public_channel_title(TOPIC, &channel.channel_id, "乗っ取り")
            .await
            .is_err()
    );
    let renamed = a
        .update_public_channel_title(TOPIC, &channel.channel_id, " 新しい雑談 ")
        .await
        .expect("rename");
    assert_eq!(renamed.title, "新しい雑談");
    assert_eq!(renamed.first_post_id, channel.first_post_id);
    assert_eq!(row(&|row| row.revision == 1).await.title, "新しい雑談");
    assert_eq!(listed(&b).await, vec!["新しい雑談".to_string()]);

    let deleted = a
        .delete_public_channel(TOPIC, &channel.channel_id)
        .await
        .expect("delete");
    assert!(deleted.deleted && deleted.title.is_empty());
    assert!(row(&|row| row.deleted).await.title.is_empty());
    assert!(listed(&b).await.is_empty());
    assert!(listed(&a).await.is_empty());
    let on_b = b
        .get_public_channel(TOPIC, &channel.channel_id)
        .await
        .expect("get")
        .expect("the deleted channel");
    assert!(on_b.deleted && on_b.title.is_empty());

    // 削除は取り消せず、削除の後はタイトルも変えられない。
    assert!(
        a.update_public_channel_title(TOPIC, &channel.channel_id, "復活")
            .await
            .is_err()
    );
    assert!(
        a.delete_public_channel(TOPIC, &channel.channel_id)
            .await
            .is_err()
    );
}

// 知らせを受けない端末(topic の lease が無い)も、ID から取得した後の背景の読み直しで変更と削除に追いつく。
#[tokio::test]
async fn a_device_without_the_hint_catches_up_after_reading_the_thread_by_id() {
    let a_docs = MemoryDocsSync::with_docs_author("13".repeat(32));
    let b_docs = MemoryDocsSync::with_docs_author("14".repeat(32));
    let blobs = Arc::new(MemoryBlobService::default());
    let network = FakeNetwork::default();
    let a = device("a", &network, a_docs.clone(), b_docs.clone(), blobs.clone());
    let b = device("b", &network, b_docs, a_docs, blobs);
    let channel = a
        .create_public_channel(TOPIC, "雑談", "", Vec::new(), Vec::new())
        .await
        .expect("create");
    assert_eq!(
        got_until(&b, &channel.channel_id, |_| true).await.title,
        "雑談"
    );

    a.update_public_channel_title(TOPIC, &channel.channel_id, "新しい雑談")
        .await
        .expect("rename");
    assert_eq!(
        got_until(&b, &channel.channel_id, |channel| channel.title != "雑談")
            .await
            .title,
        "新しい雑談"
    );
    assert_eq!(listed(&b).await, vec!["新しい雑談".to_string()]);
}
