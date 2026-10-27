//! #1950: 公開スレッドの添付を再表示する短命の要求。投稿・reaction・blobは書き換えない。
use crate::service::*;
use std::sync::{Mutex as StdMutex, OnceLock};
use tokio::sync::{Notify, Semaphore, broadcast};

pub const LIVE_OVERLAY_PUSH_TTL_MS: i64 = 10_000;
const SEEN_LIMIT: usize = 4096;
const QUEUE_LIMIT: usize = 256;
pub(crate) const PUSH_KIND: &str = "live-overlay-push-v1";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PushLiveOverlayRequest {
    pub topic: String,
    pub channel_id: String,
    pub object_id: String,
    pub attachment_index: u32,
    pub attachment_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ResolveLiveOverlayPushRequest {
    pub event_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LiveOverlayPushView {
    pub event_id: String,
    pub sender_pubkey: String,
    pub target: PushLiveOverlayRequest,
    pub post_created_at: i64,
    pub sent_at: i64,
    pub received_at: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PushPayload {
    version: u8,
    nonce: String,
    target: PushLiveOverlayRequest,
    post_created_at: i64,
    sent_at: i64,
}

#[derive(Default)]
struct Seen {
    events: BTreeMap<String, LiveOverlayPushView>,
    expiries: BTreeSet<(i64, String)>,
}

impl Seen {
    fn prune(&mut self, now: i64) {
        while self.expiries.first().is_some_and(|(at, _)| *at <= now) {
            let (_, id) = self.expiries.pop_first().expect("checked expiry");
            self.events.remove(&id);
        }
    }
}

pub(crate) struct LiveOverlayPushes {
    seen: StdMutex<Seen>,
    pub(crate) sending: Semaphore,
    resolving: Semaphore,
    changes: broadcast::Sender<Option<LiveOverlayPushView>>,
    wake: Arc<Notify>,
    reaper: OnceLock<AbortOnDropTask>,
}

impl Default for LiveOverlayPushes {
    fn default() -> Self {
        Self {
            seen: StdMutex::new(Seen::default()),
            sending: Semaphore::new(QUEUE_LIMIT),
            resolving: Semaphore::new(QUEUE_LIMIT),
            changes: broadcast::channel(QUEUE_LIMIT).0,
            wake: Arc::new(Notify::new()),
            reaper: OnceLock::new(),
        }
    }
}

impl LiveOverlayPushes {
    pub(crate) fn subscribe(self: &Arc<Self>) -> broadcast::Receiver<Option<LiveOverlayPushView>> {
        self.reaper.get_or_init(|| {
            let weak = Arc::downgrade(self);
            AbortOnDropTask::new(n0_future::task::spawn(async move {
                loop {
                    let Some(bus) = weak.upgrade() else { break };
                    let now = Utc::now().timestamp_millis();
                    let delay = {
                        let mut seen = bus.seen.lock().expect("overlay ledger");
                        seen.prune(now);
                        seen.expiries.first().map_or(LIVE_OVERLAY_PUSH_TTL_MS, |(at, _)| at.saturating_sub(now).max(1))
                    };
                    let wake = bus.wake.clone();
                    drop(bus);
                    tokio::select! {
                        _ = n0_future::time::sleep(std::time::Duration::from_millis(delay as u64)) => {},
                        _ = wake.notified() => {},
                    }
                }
            }))
        });
        self.changes.subscribe()
    }

    pub(crate) fn receive(
        &self,
        topic: &str,
        channel: &str,
        envelope: &KukuriEnvelope,
        now: i64,
    ) -> Result<()> {
        if let Some(view) = self.record(topic, channel, envelope, now)? {
            let _ = self.changes.send(Some(view));
        }
        Ok(())
    }

    fn record(
        &self,
        topic: &str,
        channel: &str,
        envelope: &KukuriEnvelope,
        now: i64,
    ) -> Result<Option<LiveOverlayPushView>> {
        anyhow::ensure!(
            envelope.kind == PUSH_KIND && envelope.content.len() <= 4096,
            "invalid overlay event"
        );
        envelope.verify()?;
        let payload: PushPayload = serde_json::from_str(&envelope.content)?;
        anyhow::ensure!(
            payload.version == 1
                && payload.nonce.len() == 32
                && payload.nonce.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid overlay version/nonce"
        );
        anyhow::ensure!(
            payload.target.topic == topic
                && payload.target.channel_id == channel
                && ChannelId::new(channel).is_public_channel(),
            "overlay scope mismatch"
        );
        anyhow::ensure!(
            payload.sent_at <= now.saturating_add(2_000)
                && payload.sent_at.saturating_add(LIVE_OVERLAY_PUSH_TTL_MS) > now,
            "expired overlay event"
        );
        let view = LiveOverlayPushView {
            event_id: envelope.id.0.clone(),
            sender_pubkey: envelope.pubkey.as_str().into(),
            target: payload.target,
            post_created_at: payload.post_created_at,
            sent_at: payload.sent_at,
            received_at: now,
        };
        let mut seen = self.seen.lock().expect("overlay ledger");
        seen.prune(now);
        if seen.events.contains_key(&view.event_id) {
            return Ok(None);
        }
        if seen.events.len() == SEEN_LIMIT {
            let _ = self.changes.send(None);
            anyhow::bail!("overlay queue full");
        }
        seen.expiries.insert((
            view.sent_at.saturating_add(LIVE_OVERLAY_PUSH_TTL_MS),
            view.event_id.clone(),
        ));
        seen.events.insert(view.event_id.clone(), view.clone());
        drop(seen);
        self.wake.notify_one();
        Ok(Some(view))
    }

    fn get(&self, id: &str) -> Option<LiveOverlayPushView> {
        let mut seen = self.seen.lock().expect("overlay ledger");
        seen.prune(Utc::now().timestamp_millis());
        seen.events.get(id).cloned()
    }

    pub(crate) fn overflow(&self) {
        let _ = self.changes.send(None);
    }
}

impl AppService {
    pub fn subscribe_live_overlay_pushes(
        &self,
    ) -> broadcast::Receiver<Option<LiveOverlayPushView>> {
        self.services.live_overlay_pushes.subscribe()
    }

    pub(crate) async fn overlay_author_hidden(&self, author: &str) -> Result<bool> {
        Ok(self
            .services
            .projection_store
            .get_muted_author(author)
            .await?
            .is_some()
            || self
                .authors_blocked_either_direction(self.current_author_pubkey().as_str(), author)
                .await?)
    }

    async fn overlay_post(
        &self,
        request: &PushLiveOverlayRequest,
        near: Option<i64>,
        policy: DocFetchPolicy,
    ) -> Result<ObjectProjectionRow> {
        anyhow::ensure!(
            ChannelId::new(&request.channel_id).is_public_channel(),
            "overlay requires a public thread"
        );
        let scope = TimelineScope::PublicChannel {
            channel_id: ChannelId::new(&request.channel_id),
        };
        let id = EnvelopeId::from(request.object_id.as_str());
        self.ensure_object_projection_near(&request.topic, &scope, &id, policy, near)
            .await?;
        let row = self
            .services
            .projection_store
            .get_object_projection(&id)
            .await?
            .context("overlay post unavailable")?;
        anyhow::ensure!(
            row.topic_id == request.topic
                && row.channel_id == request.channel_id
                && matches!(row.object_kind.as_str(), "post" | "comment"),
            "overlay post scope mismatch"
        );
        let asset = row
            .attachments
            .get(request.attachment_index as usize)
            .context("overlay attachment missing")?;
        anyhow::ensure!(
            asset.hash.as_str() == request.attachment_hash
                && asset.mime.starts_with("image/")
                && asset.role != kukuri_core::AssetRole::VideoPoster,
            "overlay attachment mismatch"
        );
        anyhow::ensure!(
            !self.overlay_author_hidden(&row.author_pubkey).await?
                && !self
                    .post_media_withheld(&asset.hash, &request.object_id)
                    .await?,
            "overlay post hidden"
        );
        Ok(row)
    }

    pub async fn push_live_overlay(&self, request: PushLiveOverlayRequest) -> Result<()> {
        let _permit = self
            .services
            .live_overlay_pushes
            .sending
            .try_acquire()
            .context("overlay send queue full")?;
        let row = self
            .overlay_post(&request, None, DocFetchPolicy::LocalOnly)
            .await?;
        let lives = self
            .services
            .projection_store
            .list_channel_live_sessions(&request.topic, &request.channel_id, LIVE_GAME_LIST_LIMIT)
            .await?;
        let mut active = false;
        for live in lives {
            if live.status == LiveSessionStatus::Live
                && !self.overlay_author_hidden(&live.host_pubkey).await?
            {
                active = true;
                break;
            }
        }
        anyhow::ensure!(active, "no active live in this thread");
        let now = Utc::now().timestamp_millis();
        let payload = PushPayload {
            version: 1,
            nonce: uuid::Uuid::new_v4().simple().to_string(),
            target: request.clone(),
            post_created_at: row.created_at,
            sent_at: now,
        };
        let event = kukuri_core::sign_envelope_json(self.keys(), PUSH_KIND, vec![], &payload)?;
        let accepted = self.services.live_overlay_pushes.record(
            &request.topic,
            &request.channel_id,
            &event,
            now,
        )?;
        // 既存の署名付き短命eventのwire carrierを共用。旧版は未知のenvelope kindを副作用なく無視する。
        self.hint_transport()
            .publish_hint(
                &channel_hint_topic_for(&request.topic, Some(&ChannelId::new(&request.channel_id))),
                GossipHint::MetaverseRoomEvent {
                    topic_id: TopicId::new(&request.topic),
                    room_id: request.channel_id.clone(),
                    event: Box::new(event.clone()),
                },
            )
            .await?;
        if let Some(view) = accepted {
            let _ = self.services.live_overlay_pushes.changes.send(Some(view));
        }
        Ok(())
    }

    /// 表示中の要求だけが呼ぶ。元投稿と添付をキー指定で確かめ、投稿の正本から表示データを作る。
    pub async fn resolve_live_overlay_push(&self, event_id: &str) -> Result<Option<PostView>> {
        let _permit = self
            .services
            .live_overlay_pushes
            .resolving
            .try_acquire()
            .context("overlay receive queue full")?;
        let Some(view) = self.services.live_overlay_pushes.get(event_id) else {
            return Ok(None);
        };
        if self.overlay_author_hidden(&view.sender_pubkey).await? {
            return Ok(None);
        }
        let remaining = view
            .sent_at
            .saturating_add(LIVE_OVERLAY_PUSH_TTL_MS)
            .saturating_sub(Utc::now().timestamp_millis())
            .max(0);
        let row = match n0_future::time::timeout(
            std::time::Duration::from_millis(remaining as u64),
            self.overlay_post(
                &view.target,
                Some(view.post_created_at),
                DocFetchPolicy::LocalThenRemote,
            ),
        )
        .await
        {
            Ok(Ok(row)) => row,
            _ => return Ok(None),
        };
        let author = [row.author_pubkey.clone()];
        let profiles = self.services.store.get_profiles(&author).await?;
        let relationships = self
            .services
            .projection_store
            .list_author_relationships(self.current_author_pubkey().as_str(), &author)
            .await?;
        // Pushは添付を描くだけ。反応者の全件取得を新しい表示経路へ持ち込まない。
        let post = self
            .row_to_view_with_cache(
                row,
                &profiles,
                &relationships,
                &HashMap::new(),
                DocFetchPolicy::LocalOnly,
            )
            .await?;
        if self.services.live_overlay_pushes.get(event_id).is_none() || post.withdrawal.is_some() {
            return Ok(None);
        }
        Ok(Some(post))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const THREAD: &str = "pc-1700000000000-aaaaaaaaaaaaaaaa";

    fn signed(keys: &KukuriKeys, nonce: usize, at: i64) -> KukuriEnvelope {
        kukuri_core::sign_envelope_json(
            keys,
            PUSH_KIND,
            vec![],
            &PushPayload {
                version: 1,
                nonce: format!("{nonce:032x}"),
                sent_at: at,
                post_created_at: 1,
                target: PushLiveOverlayRequest {
                    topic: "topic".into(),
                    channel_id: THREAD.into(),
                    object_id: "post".into(),
                    attachment_index: 0,
                    attachment_hash: "image".into(),
                },
            },
        )
        .unwrap()
    }

    #[test]
    fn independent_presses_count_but_duplicate_invalid_scope_signature_and_expired_events_do_not() {
        let bus = LiveOverlayPushes::default();
        let keys = generate_keys();
        let at = 1_000_000;
        let first = signed(&keys, 1, at);
        bus.receive("topic", THREAD, &first, at).unwrap();
        bus.receive("topic", THREAD, &first, at).unwrap();
        bus.receive("topic", THREAD, &signed(&keys, 2, at), at)
            .unwrap();
        bus.receive("topic", THREAD, &signed(&generate_keys(), 3, at), at)
            .unwrap();
        assert_eq!(bus.seen.lock().unwrap().events.len(), 3);
        assert!(
            bus.receive("elsewhere", THREAD, &signed(&keys, 4, at), at)
                .is_err()
        );
        assert!(
            bus.receive("topic", "pc-1700000000000-bbbbbbbbbbbbbbbb", &first, at)
                .is_err()
        );
        assert!(
            bus.receive("topic", THREAD, &first, at + LIVE_OVERLAY_PUSH_TTL_MS)
                .is_err()
        );
        let mut forged = signed(&keys, 5, at);
        forged.content.push(' ');
        assert!(bus.receive("topic", THREAD, &forged, at).is_err());
        assert!(
            bus.receive("topic", THREAD, &signed(&keys, 6, at + 2_001), at)
                .is_err()
        );
        assert_eq!(bus.seen.lock().unwrap().events.len(), 3);
        let hint = GossipHint::MetaverseRoomEvent {
            topic_id: TopicId::new("topic"),
            room_id: THREAD.into(),
            event: Box::new(first.clone()),
        };
        let bytes = serde_json::to_vec(&hint).unwrap();
        assert!(serde_json::from_slice::<GossipHint>(&bytes).is_ok());
        assert!(
            parse_metaverse_room_event_envelope(first, at, "peer".into())
                .unwrap()
                .is_none(),
            "legacy handler ignores the new kind"
        );
    }

    #[test]
    fn tolerated_future_time_does_not_allow_replaying_a_pruned_id() {
        let bus = LiveOverlayPushes::default();
        let at = 1_000_000;
        let event = signed(&generate_keys(), 1, at + 2_000);
        assert!(bus.record("topic", THREAD, &event, at).unwrap().is_some());
        assert!(
            bus.record("topic", THREAD, &event, at + 10_500)
                .unwrap()
                .is_none()
        );
        assert!(bus.record("topic", THREAD, &event, at + 12_000).is_err());
    }

    #[test]
    fn retention_is_bounded_without_evicting_live_ids_and_recovers_after_expiry() {
        let bus = LiveOverlayPushes::default();
        let keys = generate_keys();
        let at = 1_000_000;
        let first = signed(&keys, 0, at);
        for id in 0..SEEN_LIMIT {
            let envelope = if id == 0 {
                first.clone()
            } else {
                signed(&keys, id, at)
            };
            bus.receive("topic", THREAD, &envelope, at).unwrap();
        }
        assert!(
            bus.receive("topic", THREAD, &signed(&keys, SEEN_LIMIT, at), at)
                .is_err()
        );
        bus.receive("topic", THREAD, &first, at).unwrap();
        assert_eq!(bus.seen.lock().unwrap().events.len(), SEEN_LIMIT);
        assert_eq!(bus.seen.lock().unwrap().expiries.len(), SEEN_LIMIT);
        bus.receive(
            "topic",
            THREAD,
            &signed(&keys, SEEN_LIMIT, at + LIVE_OVERLAY_PUSH_TTL_MS),
            at + LIVE_OVERLAY_PUSH_TTL_MS,
        )
        .unwrap();
        assert_eq!(bus.seen.lock().unwrap().events.len(), 1);
    }

    #[tokio::test]
    async fn idle_expiry_reclaims_the_ledger_without_a_new_press() {
        let bus = Arc::new(LiveOverlayPushes::default());
        let mut changes = bus.subscribe();
        let now = Utc::now().timestamp_millis();
        bus.receive(
            "topic",
            THREAD,
            &signed(&generate_keys(), 1, now - LIVE_OVERLAY_PUSH_TTL_MS + 30),
            now,
        )
        .unwrap();
        assert!(changes.recv().await.unwrap().is_some());
        n0_future::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(bus.seen.lock().unwrap().events.is_empty());
    }

    #[test]
    fn twenty_four_senders_at_ten_presses_per_second_for_sixty_seconds_keep_only_the_live_window() {
        let bus = LiveOverlayPushes::default();
        let mut received = bus.changes.subscribe();
        let keys = (0..24).map(|_| generate_keys()).collect::<Vec<_>>();
        let at = 1_000_000;
        let mut peak = 0;
        let mut midway = 0;
        let mut delivered = 0;
        // 100ms刻みの仮想時計。全14,400件を署名・検証し、7種類の添付参照を循環させる。
        for tick in 0..600 {
            let now = at + tick * 100;
            for (sender, key) in keys.iter().enumerate() {
                let nonce = tick as usize * 24 + sender;
                let payload = PushPayload {
                    version: 1,
                    nonce: format!("{nonce:032x}"),
                    sent_at: now,
                    post_created_at: 1,
                    target: PushLiveOverlayRequest {
                        topic: "topic".into(),
                        channel_id: THREAD.into(),
                        object_id: format!("post-{}", nonce % 7),
                        attachment_index: 0,
                        attachment_hash: format!("image-{}", nonce % 7),
                    },
                };
                let event =
                    kukuri_core::sign_envelope_json(key, PUSH_KIND, vec![], &payload).unwrap();
                bus.receive("topic", THREAD, &event, now).unwrap();
                assert!(received.try_recv().unwrap().is_some());
                delivered += 1;
            }
            let retained = bus.seen.lock().unwrap().events.len();
            peak = peak.max(retained);
            if tick == 299 {
                midway = retained;
            }
        }
        assert_eq!(delivered, 14_400);
        assert_eq!(peak, 2400);
        assert_eq!(midway, bus.seen.lock().unwrap().events.len());
        assert!(received.try_recv().is_err());
        let mut seen = bus.seen.lock().unwrap();
        seen.prune(at + 60_000 + LIVE_OVERLAY_PUSH_TTL_MS);
        assert!(seen.events.is_empty() && seen.expiries.is_empty());
    }
}
