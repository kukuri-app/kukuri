//! timeline に出た author のうち、手元に profile が無いものを背景で読む(#1221 R6-B)。
//!
//! 読取りは購読しない(R2-C)。1 回の表示で渡すのはページの author だけで、queue と台帳は上限つき。
//! 同じ author は待機・取得中に重複させず、完了から `MISSING_PROFILE_RETRY` の間は読み直さない。
//! 投稿が載る公開 topic が分かれば、その topic の参加者(profile を中継する)も読む(#1419)。

use std::collections::{HashMap, HashSet, VecDeque};

use n0_future::time::{Duration, Instant};
use tokio_stream::wrappers::ReceiverStream;

use super::author_state_support::hydrate_author_profile;
use super::remote_read_support::REMOTE_READ_DEADLINE;
use super::subscription_registry::AbortOnDropTask;
use super::*;

pub(crate) const MISSING_PROFILE_QUEUE: usize = 64;
pub(crate) const MISSING_PROFILE_LEDGER: usize = 1024;
const MISSING_PROFILE_CONCURRENT: usize = 4;
const MISSING_PROFILE_RETRY: Duration = Duration::from_secs(5);
/// 読む author と、その投稿が載る公開 topic。
type ProfileRequest = (String, Option<String>);

#[derive(Default)]
pub(crate) struct MissingProfiles {
    next_read_at: HashMap<String, Instant>,
    order: VecDeque<String>,
    pending: HashSet<String>,
    worker: Option<(tokio::sync::mpsc::Sender<ProfileRequest>, AbortOnDropTask)>,
}

impl MissingProfiles {
    /// 読んでよければ台帳に記録して true を返す。台帳は古い順に `MISSING_PROFILE_LEDGER` 件まで。
    fn admit(&mut self, author: &str, now: Instant) -> bool {
        if self.pending.contains(author)
            || self.next_read_at.get(author).is_some_and(|at| *at > now)
        {
            return false;
        }
        if self
            .next_read_at
            .insert(author.to_string(), now + MISSING_PROFILE_RETRY)
            .is_none()
        {
            self.order.push_back(author.to_string());
            if self.order.len() > MISSING_PROFILE_LEDGER
                && let Some(oldest) = self.order.pop_front()
            {
                self.next_read_at.remove(&oldest);
            }
        }
        self.pending.insert(author.to_string());
        true
    }

    fn complete(&mut self, author: &str, now: Instant) {
        self.pending.remove(author);
        if let Some(at) = self.next_read_at.get_mut(author) {
            *at = now + MISSING_PROFILE_RETRY;
        }
    }

    pub(crate) fn stop(&mut self) {
        self.worker = None;
        self.pending.clear();
    }
}

impl AppService {
    /// 手元に profile の無い author を、背景の読取りへ渡す。queue が満ちていれば残りは渡さない(次の表示で渡る)。
    pub(crate) async fn request_missing_profiles(
        &self,
        authors: impl IntoIterator<Item = ProfileRequest>,
    ) {
        let mut missing = self.subscription_registry.missing_profiles.lock().await;
        if missing
            .worker
            .as_ref()
            .is_none_or(|(_, task)| task.is_finished())
        {
            let (sender, receiver) =
                tokio::sync::mpsc::channel::<ProfileRequest>(MISSING_PROFILE_QUEUE);
            let services = self.services.clone();
            let local = self.current_author_pubkey();
            let state = Arc::downgrade(&self.subscription_registry.missing_profiles);
            let task = AbortOnDropTask::new(n0_future::task::spawn(async move {
                ReceiverStream::new(receiver)
                    .for_each_concurrent(MISSING_PROFILE_CONCURRENT, |(author, topic)| {
                        let (services, local, state) = (services.clone(), local.clone(), state.clone());
                        async move {
                            let result = n0_future::time::timeout(
                                REMOTE_READ_DEADLINE,
                                hydrate_author_profile(&services, &local, &author, topic.as_deref()),
                            ).await;
                            if !matches!(result, Ok(Ok(_))) {
                                tracing::debug!(author_pubkey = %author, ?result, "missing profile read deferred");
                            }
                            if let Some(state) = state.upgrade() {
                                state.lock().await.complete(&author, Instant::now());
                            }
                        }
                    }).await;
            }));
            missing.worker = Some((sender, task));
        }
        let now = Instant::now();
        for (author, topic) in authors {
            let Some(sender) = missing.worker.as_ref().map(|(sender, _)| sender.clone()) else {
                return;
            };
            if sender.capacity() == 0 {
                return;
            }
            if missing.admit(&author, now) {
                let _ = sender.try_send((author, topic));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ledger_skips_recent_authors_and_keeps_its_bound() {
        let mut missing = MissingProfiles::default();
        let now = Instant::now();
        assert!(missing.admit("a", now));
        assert!(!missing.admit("a", now));
        assert!(!missing.admit("a", now + MISSING_PROFILE_RETRY));
        missing.complete("a", now);
        assert!(missing.admit("a", now + MISSING_PROFILE_RETRY));
        missing.complete("a", now + MISSING_PROFILE_RETRY);
        for index in 0..MISSING_PROFILE_LEDGER * 2 {
            let author = format!("author-{index}");
            missing.admit(&author, now);
            missing.complete(&author, now);
        }
        assert_eq!(missing.next_read_at.len(), MISSING_PROFILE_LEDGER);
        assert_eq!(missing.order.len(), MISSING_PROFILE_LEDGER);
        assert!(
            missing.admit("author-0", now),
            "evicted authors can be read again"
        );
    }
}
