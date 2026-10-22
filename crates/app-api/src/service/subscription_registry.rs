use super::*;

/// account の購読taskと世代fence。張り替える前に旧taskをabortする。
/// 世代はaccountのhandle間で共有し、過去のtopic名を保持しない(#1778)。
#[derive(Clone, Default)]
pub(crate) struct SubscriptionRegistry {
    /// Accountごとに一つの暗号化offer受信task。dropでも実行中の取得を中止する。
    pub(crate) account_receive_offer_task: Arc<Mutex<Option<AbortOnDropTask>>>,
    pub(crate) account_receive_offer_lease: Arc<std::sync::Mutex<Option<ReceiveOfferLease>>>,
    pub(crate) account_receive_offer_closed: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) account_receive_offer_shutdown: Arc<tokio::sync::Notify>,
    pub(crate) public_notification_offer_queue: Arc<Mutex<Option<PublicNotificationOfferQueue>>>,
    /// One account-wide owner for due protected DM outbox work.
    pub(crate) dm_outbox_retry_task: Arc<Mutex<Option<AbortOnDropTask>>>,
    pub(crate) dm_outbox_retry_closed: Arc<std::sync::atomic::AtomicBool>,
    /// 復帰の契機（ADR 0059 §5）で、次の間隔を待たずに 1 回分を行わせる。
    pub(crate) dm_outbox_retry_wake: Arc<tokio::sync::Notify>,
    /// 保持する公開 topic の取り込みの task（account ごとに 1 つ。#1909）。
    pub(crate) topic_retention_task: Arc<Mutex<Option<AbortOnDropTask>>>,
    pub(crate) topic_retention_closed: Arc<std::sync::atomic::AtomicBool>,
    /// 保持の設定で、次の間隔を待たずに 1 回分を行わせる。
    pub(crate) topic_retention_wake: Arc<tokio::sync::Notify>,
    #[cfg(test)]
    pub(crate) dm_outbox_retry_starts: Arc<std::sync::atomic::AtomicUsize>,
    /// 購読する scope の lease と、その task(上限 64。#1221 R2-C)。
    pub(crate) scope_leases: Arc<Mutex<ScopeLeases>>,
    /// 手元に profile の無い timeline の author を背景で読む queue と台帳(#1221 R6-B)。
    pub(crate) missing_profiles: Arc<Mutex<super::missing_profiles::MissingProfiles>>,
    /// 自分の端末で hosting する Dome の heartbeat task(key = instance id)。
    pub(crate) dome_heartbeats: Arc<Mutex<HashMap<String, AbortOnDropTask>>>,
    /// live presence の期限管理 task(key = live_presence_task_key)。
    pub(crate) live_presence_tasks: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// public購読の世代を採番する単一counter。
    pub(crate) generation: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) struct AbortOnDropTask(Option<JoinHandle<()>>);

/// 通知の offer の manifest、受け手、scope、送れなかった受け手へ送り直すまでの残りの間隔(#1521)。
pub(crate) type PublicNotificationOffer = (
    Vec<u8>,
    BTreeSet<String>,
    kukuri_core::ReceiveOfferScopeV1,
    &'static [std::time::Duration],
);

pub(crate) struct PublicNotificationOfferQueue {
    pub(crate) sender: tokio::sync::mpsc::Sender<PublicNotificationOffer>,
    pub(crate) task: AbortOnDropTask,
}

impl AbortOnDropTask {
    pub(crate) fn new(handle: JoinHandle<()>) -> Self {
        Self(Some(handle))
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.0.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub(crate) fn abort(&self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }

    pub(crate) async fn wait(mut self) {
        if let Some(handle) = self.0.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for AbortOnDropTask {
    fn drop(&mut self) {
        self.abort();
    }
}
