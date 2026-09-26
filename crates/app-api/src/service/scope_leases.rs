//! 購読する scope の lease(#1221 R2-C、ADR 0055 §1)。
//!
//! 購読の task・gossip hint の購読・docs replica の購読は、lease を持つ key だけに置く。key は account 全体で
//! 同時に [`MAX_ACTIVE_SCOPES`] 件まで。holder(開いている列・参加中の private channel / live / Dome・CLI の
//! desired)が key を取り、最後の holder が外れたら task を止め、hint の購読を抜け、replica を閉じる。
//! 読み書きの操作は lease を取らない。

use super::*;

/// 同時に購読する scope の上限(ADR 0055 §2 の「意味上の同期対象 64」)。
pub const MAX_ACTIVE_SCOPES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ScopeKey {
    Topic(String),
    Channel(String, String),
    Author(String),
}

impl ScopeKey {
    fn topic(&self) -> Option<&str> {
        match self {
            Self::Topic(topic) | Self::Channel(topic, _) => Some(topic),
            Self::Author(_) => None,
        }
    }
}

/// 上限を超える取得の拒否。拒否した要求は、どの key も取っていない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeLimitReached;

impl std::fmt::Display for ScopeLimitReached {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "SCOPE_LIMIT_REACHED: all {MAX_ACTIVE_SCOPES} active scopes are in use"
        )
    }
}

impl std::error::Error for ScopeLimitReached {}

pub(crate) struct ScopeTask {
    pub(crate) handle: AbortOnDropTask,
    pub(crate) replica: ReplicaId,
    /// author の購読は hint を購読しない。
    pub(crate) hint_topic: Option<TopicId>,
    /// private channel の epoch が変わる前の replica(1 つだけ)。参加者はそこに書かれた handoff の grant を
    /// 同期で受け取る。現在と直前の 2 つまでを開く(ADR 0055 §2 の「現在 / 直前の 2 bucket」)。
    pub(crate) previous: Option<ReplicaId>,
}

struct Lease {
    holders: usize,
    /// gossip を止めた topic・参加していない channel・起動に失敗した key は task を持たない。
    task: Option<ScopeTask>,
}

#[derive(Default)]
pub(crate) struct ScopeLeases {
    leases: HashMap<ScopeKey, Lease>,
    holders: HashMap<String, BTreeSet<ScopeKey>>,
}

#[derive(Default)]
pub(crate) struct LeaseChange {
    pub(crate) started: Vec<ScopeKey>,
    pub(crate) stopped: Vec<ScopeTask>,
}

impl ScopeLeases {
    /// holder の key を `keys` に置き換える。上限を超えるなら何も変えずに拒否する。
    /// 他の holder が既に持つ key は枠を消費しない。
    pub(crate) fn set_holder(
        &mut self,
        holder: &str,
        keys: BTreeSet<ScopeKey>,
    ) -> Result<LeaseChange, ScopeLimitReached> {
        let old = self.holders.remove(holder).unwrap_or_default();
        let created = keys
            .iter()
            .filter(|key| !self.leases.contains_key(key))
            .count();
        let freed = old
            .difference(&keys)
            .filter(|key| self.leases.get(key).is_some_and(|lease| lease.holders == 1))
            .count();
        if created > 0 && self.leases.len() + created - freed > MAX_ACTIVE_SCOPES {
            if !old.is_empty() {
                self.holders.insert(holder.to_string(), old);
            }
            return Err(ScopeLimitReached);
        }
        let mut change = LeaseChange::default();
        for key in keys.difference(&old) {
            let lease = self.leases.entry(key.clone()).or_insert_with(|| {
                change.started.push(key.clone());
                Lease {
                    holders: 0,
                    task: None,
                }
            });
            lease.holders += 1;
        }
        for key in old.difference(&keys) {
            if let Some(lease) = self.leases.get_mut(key) {
                lease.holders -= 1;
                if lease.holders == 0
                    && let Some(lease) = self.leases.remove(key)
                {
                    change.stopped.extend(lease.task);
                }
            }
        }
        if !keys.is_empty() {
            self.holders.insert(holder.to_string(), keys);
        }
        Ok(change)
    }

    pub(crate) fn keys(&self) -> Vec<ScopeKey> {
        self.leases.keys().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.leases.len()
    }

    #[cfg(test)]
    pub(crate) fn running_tasks(&self) -> usize {
        self.leases
            .values()
            .filter(|lease| {
                lease
                    .task
                    .as_ref()
                    .is_some_and(|task| !task.handle.is_finished())
            })
            .count()
    }

    pub(crate) fn holds(&self, holder: &str, key: &ScopeKey) -> bool {
        self.holders
            .get(holder)
            .is_some_and(|keys| keys.contains(key))
    }

    pub(crate) fn has_running_task(&self, key: &ScopeKey) -> bool {
        self.leases
            .get(key)
            .and_then(|lease| lease.task.as_ref())
            .is_some_and(|task| !task.handle.is_finished())
    }

    fn slot(&mut self, key: &ScopeKey) -> Option<&mut Option<ScopeTask>> {
        self.leases.get_mut(key).map(|lease| &mut lease.task)
    }

    /// 条件に合う key を 1 つでも持つ holder。
    pub(crate) fn holders_with(
        &self,
        holder_prefixes: &[&str],
        matches: impl Fn(&ScopeKey) -> bool,
    ) -> Vec<String> {
        self.holders
            .iter()
            .filter(|(holder, keys)| {
                holder_prefixes
                    .iter()
                    .any(|prefix| holder.starts_with(prefix))
                    && keys.iter().any(&matches)
            })
            .map(|(holder, _)| holder.clone())
            .collect()
    }

    pub(crate) fn clear(&mut self) -> Vec<ScopeTask> {
        self.holders.clear();
        self.leases
            .drain()
            .filter_map(|(_, lease)| lease.task)
            .collect()
    }
}

pub(crate) fn display_holder(observer: &str) -> String {
    format!("display:{observer}")
}

pub(crate) fn desired_holder(topic: &str, scope: &TimelineScope) -> String {
    match scope {
        TimelineScope::Public => format!("desired:{topic}"),
        TimelineScope::Channel { channel_id } => {
            format!("desired:{topic}::{}", channel_id.as_str())
        }
    }
}

pub(crate) fn private_channel_holder(topic: &str, channel: &str) -> String {
    format!("private:{topic}::{channel}")
}

pub(crate) fn live_holder(task_key: &str) -> String {
    format!("live:{task_key}")
}

pub(crate) fn dome_holder(instance_id: &str) -> String {
    format!("dome:{instance_id}")
}

/// 参加の holder が取る key(topic と、channel なら channel)。
pub(crate) fn participation_keys(topic: &str, channel: Option<&ChannelId>) -> Vec<ScopeKey> {
    let mut keys = vec![ScopeKey::Topic(topic.to_string())];
    if let Some(channel) = channel {
        keys.push(ScopeKey::Channel(
            topic.to_string(),
            channel.as_str().to_string(),
        ));
    }
    keys
}

/// task を止め、hint の購読を抜け、replica を閉じる(旧 sync の要求も残さない)。
pub(crate) async fn stop_scope_task(services: &ServiceHandles, task: ScopeTask) {
    task.handle.abort();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task.handle.wait()).await;
    if let Some(hint_topic) = &task.hint_topic {
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            services.hint_transport.unsubscribe_hints(hint_topic),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(topic = %hint_topic.as_str(), %error, "failed to leave scope hints")
            }
            Err(_) => warn!(topic = %hint_topic.as_str(), "timed out leaving scope hints"),
        }
    }
    for replica in std::iter::once(&task.replica).chain(&task.previous) {
        close_scope_replica(services, replica).await;
    }
}

async fn close_scope_replica(services: &ServiceHandles, replica: &ReplicaId) {
    if let Err(error) = services.docs_sync.close_replica(replica).await {
        warn!(replica = %replica.as_str(), %error, "failed to close scope replica");
    }
}

/// holder の key をすべて外す。task の中からも呼べるように、AppService を取らない。
pub(crate) async fn release_scope_holder(
    services: &ServiceHandles,
    leases: &Mutex<ScopeLeases>,
    holder: &str,
) {
    let mut leases = leases.lock().await;
    let stopped = leases
        .set_holder(holder, BTreeSet::new())
        .map(|change| change.stopped)
        .unwrap_or_default();
    // 同じ key の取り直しが、止める前の hint 購読の上に乗らないよう、lock の中で止める。
    for task in stopped {
        stop_scope_task(services, task).await;
    }
}

impl AppService {
    /// holder の key を置き換え、始まった key の task を起こし、外れた key の task を止める。
    pub(crate) async fn set_scope_holder(
        &self,
        holder: &str,
        keys: impl IntoIterator<Item = ScopeKey>,
    ) -> Result<()> {
        let mut leases = self.subscription_registry.scope_leases.lock().await;
        let change = leases.set_holder(holder, keys.into_iter().collect())?;
        for task in change.stopped {
            stop_scope_task(&self.services, task).await;
        }
        for key in change.started {
            let task = self.start_scope_task(&key).await;
            if let Some(slot) = leases.slot(&key) {
                *slot = task;
            }
        }
        Ok(())
    }

    pub(crate) async fn release_scope_holder(&self, holder: &str) {
        release_scope_holder(
            &self.services,
            &self.subscription_registry.scope_leases,
            holder,
        )
        .await;
    }

    /// timeline の scope が取る key。channel の列は公開 timeline も読むので、topic も取る。
    pub(crate) async fn timeline_scope_keys(
        &self,
        topic: &str,
        scope: &TimelineScope,
    ) -> Result<Vec<ScopeKey>> {
        let channel = match scope {
            TimelineScope::Public => None,
            TimelineScope::Channel { channel_id } => {
                self.ensure_private_channel_access(topic, channel_id)
                    .await?;
                Some(channel_id)
            }
        };
        Ok(participation_keys(topic, channel))
    }

    async fn start_scope_task(&self, key: &ScopeKey) -> Option<ScopeTask> {
        let started = match key {
            ScopeKey::Topic(topic) => {
                if self.is_topic_gossip_disabled(topic).await {
                    return None;
                }
                self.spawn_subscription_task(
                    topic,
                    None,
                    topic_replica_id(topic),
                    TopicId::new(topic),
                )
                .await
            }
            ScopeKey::Channel(topic, channel) => {
                if self.is_channel_gossip_disabled(topic, channel).await {
                    return None;
                }
                let state = self.joined_private_channel_state(topic, channel).await?;
                self.spawn_subscription_task(
                    topic,
                    Some(state.channel_id.clone()),
                    current_private_channel_replica_id(&state),
                    private_channel_hint_topic(channel),
                )
                .await
            }
            ScopeKey::Author(author) => self.spawn_author_subscription(author).await,
        };
        started
            .inspect_err(|error| warn!(?key, %error, "failed to start a scope subscription"))
            .ok()
    }

    /// lease のある key の task を作り直す。lease が無ければ何もしない。
    async fn restart_leased_task(&self, leases: &mut ScopeLeases, key: &ScopeKey) {
        let Some(slot) = leases.slot(key) else {
            return;
        };
        let old = slot.take();
        let mut new = self.start_scope_task(key).await;
        if let Some(old) = old {
            match &mut new {
                // gossip topic は抜けない。抜けてすぐ入り直すと、相手が古い Disconnect を新しい Join より後に
                // 処理した場合に片側だけが neighbor を失う。
                Some(new) if new.replica == old.replica => {
                    old.handle.abort();
                    new.previous = old.previous;
                }
                // epoch が変わった。直前の replica を覚え、それより前(2 世代前)を閉じる。
                Some(new) => {
                    old.handle.abort();
                    new.previous = Some(old.replica);
                    if let Some(replica) = &old.previous {
                        close_scope_replica(&self.services, replica).await;
                    }
                }
                None => stop_scope_task(&self.services, old).await,
            }
        }
        if let Some(slot) = leases.slot(key) {
            *slot = new;
        }
    }

    pub(crate) async fn restart_scope_subscriptions(&self, matches: impl Fn(&ScopeKey) -> bool) {
        let mut leases = self.subscription_registry.scope_leases.lock().await;
        for key in leases.keys().into_iter().filter(|key| matches(key)) {
            self.restart_leased_task(&mut leases, &key).await;
        }
    }

    pub(crate) async fn restart_scope_subscription(&self, key: &ScopeKey) {
        self.restart_scope_subscriptions(|leased| leased == key)
            .await;
    }

    pub(crate) async fn has_topic_subscription(&self, topic_id: &str) -> bool {
        self.subscription_registry
            .scope_leases
            .lock()
            .await
            .has_running_task(&ScopeKey::Topic(topic_id.to_string()))
    }

    pub(crate) async fn leased_topics(&self) -> Vec<String> {
        self.subscription_registry
            .scope_leases
            .lock()
            .await
            .keys()
            .into_iter()
            .filter_map(|key| match key {
                ScopeKey::Topic(topic) => Some(topic),
                _ => None,
            })
            .collect()
    }

    /// endpoint の作り直しの後に 1 回呼ぶ。private channel の秘密を新しい docs へ登録し直し、
    /// lease のある key の task だけを作り直す(最大 [`MAX_ACTIVE_SCOPES`] 件)。
    pub async fn rebuild_scope_subscriptions(&self) -> Result<()> {
        let joined = self
            .joined_private_channels
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for state in joined {
            register_private_channel_replica_secrets(self.docs_sync(), &state).await?;
        }
        self.restart_scope_subscriptions(|_| true).await;
        Ok(())
    }

    /// 購読の対象を topic ごとに変える(gossip の停止・再開)。lease は残す。
    pub(crate) async fn restart_topic_scope_subscriptions(&self, topic_id: &str) {
        self.restart_scope_subscriptions(|key| key.topic() == Some(topic_id))
            .await;
    }
}
