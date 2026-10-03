//! #1505: iroh-gossip は message を内容の hash で識別し、同じ内容を 90 秒間 1 回だけ届ける(plumtree の
//! `message_id_retention`)。同じ投稿への続く reaction(別の人・同じ人の付け外し)の hint も、第三の端末へ届く。

use super::*;

/// iroh-gossip と同じく、受け手ごとに同じ内容の hint を 1 回だけ届ける(試験は保持の 90 秒の内側で終わる)。
struct GossipHints(StaticTransport);

#[async_trait]
impl HintTransport for GossipHints {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        let mut seen = BTreeSet::new();
        let stream = self.0.subscribe_hints(topic).await?;
        Ok(Box::pin(stream.filter(move |event| {
            std::future::ready(seen.insert(serde_json::to_vec(&event.hint).expect("hint bytes")))
        })))
    }

    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        self.0.unsubscribe_hints(topic).await
    }

    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        self.0.publish_hint(topic, hint).await
    }
}

/// 投稿が一覧に出て、付いている reaction の絵文字(並べ替えたもの)が `expected` になるまで待つ。
async fn wait_for_reactions(app: &AppService, topic: &str, post: &str, expected: &[&str]) {
    let reactions = || async {
        let timeline = app.list_timeline(topic, None, 20).await.expect("timeline");
        let item = timeline
            .items
            .into_iter()
            .find(|item| item.object_id == post)?;
        let mut emojis = item
            .reaction_summary
            .into_iter()
            .filter_map(|entry| entry.emoji)
            .collect::<Vec<_>>();
        emojis.sort();
        Some(emojis)
    };
    let arrived = timeout(Duration::from_secs(10), async {
        while !reactions().await.is_some_and(|emojis| emojis == expected) {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        arrived.is_ok(),
        "expected {expected:?}, got {:?}",
        reactions().await
    );
}

#[tokio::test]
async fn reactions_to_the_same_post_within_the_gossip_window_reach_a_third_device() {
    let hints = Arc::new(GossipHints(StaticTransport::new(PeerSnapshot::default())));
    // 3 端末とも同じ docs を provider として読む(手元に無い record は provider から読む)。
    let world = CountingDocsSync::default();
    let docs = Arc::new(CountingDocsSync {
        remote: Some(Arc::new(world.clone())),
        ..world
    });
    let blob_service = Arc::new(MemoryBlobService::default());
    let device = || {
        let store = Arc::new(MemoryStore::default());
        let app = app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            hints.clone(),
            docs.clone(),
            blob_service.clone(),
            generate_keys(),
        );
        app.switch_writer(1);
        app
    };
    let (a, b, c) = (device(), device(), device());
    let topic = "kukuri:topic:reaction-hints";
    for app in [&a, &b, &c] {
        display_topic(app, topic).await.expect("display the topic");
    }
    let post = c
        .create_post(topic, "reactions within the gossip window", None)
        .await
        .expect("create post");
    // 反応する 2 端末にも、投稿が hint で届いている。
    wait_for_reactions(&a, topic, &post, &[]).await;
    wait_for_reactions(&b, topic, &post, &[]).await;
    let emoji = |emoji: &str| ReactionKeyV1::Emoji {
        emoji: emoji.into(),
    };

    a.toggle_reaction(topic, &post, emoji("👍"), None)
        .await
        .expect("a reacts");
    wait_for_reactions(&c, topic, &post, &["👍"]).await;
    // 別の人の同じ投稿への reaction。
    b.toggle_reaction(topic, &post, emoji("🙌"), None)
        .await
        .expect("b reacts");
    wait_for_reactions(&c, topic, &post, &["👍", "🙌"]).await;
    // 同じ人の付け外し。
    a.toggle_reaction(topic, &post, emoji("👍"), None)
        .await
        .expect("a removes the reaction");
    wait_for_reactions(&c, topic, &post, &["🙌"]).await;
}
