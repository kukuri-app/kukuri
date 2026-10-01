//! timeline に出た author のうち、手元に profile が無いものを背景で読む(#1221 R6-B)。
//!
//! 読取りは購読しない(R2-C)。1 回の表示で渡すのはページの author だけで、queue と台帳は上限つき。
//! 同じ author は `MISSING_PROFILE_RETRY` の間は読み直さない(profile の無い author を読み続けない)。
//! 投稿が載る公開 topic が分かれば、その topic の参加者(profile を中継する)も読む(#1419)。

use std::collections::{HashMap, VecDeque};

use n0_future::time::{Duration, Instant};

use super::author_state_support::hydrate_author_profile;
use super::subscription_registry::AbortOnDropTask;
use super::*;

pub(crate) const MISSING_PROFILE_QUEUE: usize = 64;
pub(crate) const MISSING_PROFILE_LEDGER: usize = 1024;
const MISSING_PROFILE_RETRY: Duration = Duration::from_secs(10 * 60);
/// 読む author と、その投稿が載る公開 topic。
type ProfileRequest = (String, Option<String>);

#[derive(Default)]
pub(crate) struct MissingProfiles {
    next_read_at: HashMap<String, Instant>,
    order: VecDeque<String>,
    worker: Option<(tokio::sync::mpsc::Sender<ProfileRequest>, AbortOnDropTask)>,
}

impl MissingProfiles {
    /// 読んでよければ台帳に記録して true を返す。台帳は古い順に `MISSING_PROFILE_LEDGER` 件まで。
    fn admit(&mut self, author: &str, now: Instant) -> bool {
        if self.next_read_at.get(author).is_some_and(|at| *at > now) {
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
        true
    }

    pub(crate) fn stop(&mut self) {
        self.worker = None;
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
            let (sender, mut receiver) =
                tokio::sync::mpsc::channel::<ProfileRequest>(MISSING_PROFILE_QUEUE);
            let services = self.services.clone();
            let local = self.current_author_pubkey();
            let task = AbortOnDropTask::new(n0_future::task::spawn(async move {
                while let Some((author, topic)) = receiver.recv().await {
                    if let Err(error) = hydrate_author_profile(
                        &services,
                        local.as_str(),
                        author.as_str(),
                        topic.as_deref(),
                    )
                    .await
                    {
                        tracing::debug!(author_pubkey = %author, %error, "missing profile read deferred");
                    }
                }
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
        assert!(missing.admit("a", now + MISSING_PROFILE_RETRY));
        for index in 0..MISSING_PROFILE_LEDGER * 2 {
            missing.admit(&format!("author-{index}"), now);
        }
        assert_eq!(missing.next_read_at.len(), MISSING_PROFILE_LEDGER);
        assert_eq!(missing.order.len(), MISSING_PROFILE_LEDGER);
        assert!(
            missing.admit("author-0", now),
            "evicted authors can be read again"
        );
    }
}
