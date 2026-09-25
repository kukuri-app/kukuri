use super::*;

/// 購読タスクの台帳(WP-H5 PR5)。
///
/// かつて AppService に生のまま並んでいた購読管理 8 フィールド(task map ×5 +
/// 世代番号 + 復旧 deadline ×2)の置き場所。守るべき不変条件はここに集約する:
/// - task を張り替えるときは必ず旧 task を abort してから登録する(取り残し防止)
/// - 世代番号(generations)は購読の張り替え毎に進み、旧世代の task が古い状態を
///   書き戻さないためのフェンスになる
/// - 復旧 deadline は「同じ対象への再起動が短時間に連打されない」ためのクールダウン
///
/// フィールドは従来と同じ粒度の Mutex map のまま(スケジューリングの挙動は不変)。
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
    #[cfg(test)]
    pub(crate) dm_outbox_retry_starts: Arc<std::sync::atomic::AtomicUsize>,
    /// 公開 topic の購読 task(key = topic_id)。
    pub(crate) subscriptions: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// private channel の購読 task(key = channel 購読キー)。
    pub(crate) private_channel_subscriptions: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// 作者(プロフィール)購読 task(key = author pubkey)。
    pub(crate) author_subscriptions: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// live presence の期限管理 task(key = live_presence_task_key)。
    pub(crate) live_presence_tasks: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// 購読の世代番号(key = topic_id)。
    pub(crate) subscription_generations: Arc<Mutex<HashMap<String, u64>>>,
    /// replica sync の再起動クールダウン(key = replica id、値 = 次回可能時刻)。
    pub(crate) replica_sync_restart_deadlines: Arc<Mutex<HashMap<String, i64>>>,
}

pub(crate) struct AbortOnDropTask(Option<JoinHandle<()>>);

pub(crate) struct PublicNotificationOfferQueue {
    pub(crate) sender:
        tokio::sync::mpsc::Sender<(Vec<u8>, BTreeSet<String>, kukuri_core::ReceiveOfferScopeV1)>,
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
