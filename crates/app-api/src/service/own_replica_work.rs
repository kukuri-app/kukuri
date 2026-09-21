//! 自分の author replica の背景の仕事(#1239): 自分の follow・block の読み出しと、プロフィールの索引の補完。
//!
//! どちらも読み終えた位置を残して小分けに進む。自分の replica の event を取りこぼしたら(`Lagged`)、やり直しを依頼として
//! 覚え、走っている仕事が終わってから 1 回にまとめて最初からやり直す(走っている仕事を止めて位置を戻すと、止めた仕事の位置の
//! 書き込みが後から着地しうる。また、取りこぼしが続くあいだ毎回やり直すと、進みを捨て続ける)。
//!
//! 同期で届いた自分の投稿は、key の event の時点では本体がまだ手元に無いことが多い。索引を足せなかった key は上限つきで覚え、
//! 本体の到着・同期の区切り・envelope の event のときに試し直す。上限を超えたら、補完のやり直しを依頼する。
//! 覚えた key と、やり直しの依頼は store(`sync_checkpoints`)に置き、購読の張り直しや再起動をまたいで引き継ぐ。
//! 自分の docs author が書いた entry(今の版の投稿。索引も同時に書く)は覚えない。

use super::*;
use crate::service::author_state_support::{CHECKPOINT_DONE, CHECKPOINT_RESTART};
use std::collections::VecDeque;

/// 本体が届くのを待つ、索引を足せなかった自分の投稿の key の数の上限。
const PENDING_PROFILE_KEYS: usize = 512;

/// drop されたときに task を止める。親の task が abort されると、その future と一緒に drop される。
pub(crate) struct AbortOnDrop(pub(crate) tokio::task::JoinHandle<()>);

impl AbortOnDrop {
    fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct OwnReplicaWork {
    author_pubkey: String,
    edge_sweep: Option<AbortOnDrop>,
    profile_backfill: Option<AbortOnDrop>,
    restart_requested: bool,
    pending_profile_keys: VecDeque<String>,
    pending_loaded: bool,
}

impl OwnReplicaWork {
    /// 自分の replica の購読の開始時に、両方の仕事を起動する(読み終えていれば、位置を確かめるだけで終わる)。
    pub(crate) fn start(services: &ServiceHandles, author_pubkey: &str) -> Self {
        Self {
            author_pubkey: author_pubkey.to_string(),
            edge_sweep: Some(spawn_own_edge_sweep(services, author_pubkey)),
            profile_backfill: Some(spawn_profile_index_backfill(services, author_pubkey)),
            restart_requested: false,
            pending_profile_keys: VecDeque::new(),
            pending_loaded: false,
        }
    }

    fn pending_checkpoint_key(&self) -> String {
        format!("own-profile-pending/{}", self.author_pubkey)
    }

    /// 前の購読で覚えた key を読み込む(最初の 1 回だけ)。
    async fn load_pending(&mut self, services: &ServiceHandles) {
        if self.pending_loaded {
            return;
        }
        self.pending_loaded = true;
        match services
            .projection_store
            .get_sync_checkpoint(&self.restart_checkpoint_key())
            .await
        {
            Ok(Some(value)) if value == CHECKPOINT_RESTART => self.restart_requested = true,
            Ok(_) => {}
            Err(error) => {
                warn!(
                    author_pubkey = %self.author_pubkey,
                    error = %error,
                    "failed to load the restart request of the own replica work"
                );
            }
        }
        match services
            .projection_store
            .get_sync_checkpoint(&self.pending_checkpoint_key())
            .await
        {
            Ok(Some(value)) => {
                if let Ok(keys) = serde_json::from_str::<Vec<String>>(&value) {
                    self.pending_profile_keys =
                        keys.into_iter().take(PENDING_PROFILE_KEYS).collect();
                }
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    author_pubkey = %self.author_pubkey,
                    error = %error,
                    "failed to load the pending own profile keys"
                );
            }
        }
    }

    async fn save_pending(&self, services: &ServiceHandles) {
        let value = serde_json::to_string(&self.pending_profile_keys).unwrap_or_default();
        if let Err(error) = services
            .projection_store
            .put_sync_checkpoint(&self.pending_checkpoint_key(), &value)
            .await
        {
            warn!(
                author_pubkey = %self.author_pubkey,
                error = %error,
                "failed to save the pending own profile keys"
            );
        }
    }

    fn restart_checkpoint_key(&self) -> String {
        format!("own-replica-restart/{}", self.author_pubkey)
    }

    /// 自分の replica の event を取りこぼした。どの key かは分からないので、最初からのやり直しを依頼する。
    /// 依頼は store に書き、購読の張り直しをまたいで残す。
    pub(crate) async fn request_restart(&mut self, services: &ServiceHandles) {
        self.restart_requested = true;
        if let Err(error) = services
            .projection_store
            .put_sync_checkpoint(&self.restart_checkpoint_key(), CHECKPOINT_RESTART)
            .await
        {
            warn!(
                author_pubkey = %self.author_pubkey,
                error = %error,
                "failed to save the restart request of the own replica work"
            );
        }
    }

    /// 間隔ごとに呼ぶ。依頼があり、走っている仕事が無ければ、位置を最初に戻して起動し直す。
    pub(crate) async fn on_tick(&mut self, services: &ServiceHandles) {
        // 前の購読で覚えた key とやり直しの依頼があれば、この購読でも引き継ぐ。
        if !self.pending_loaded {
            self.load_pending(services).await;
            self.retry_pending(services).await;
        }
        if !self.restart_requested {
            return;
        }
        let running =
            |task: &Option<AbortOnDrop>| task.as_ref().is_some_and(|task| !task.is_finished());
        if running(&self.edge_sweep) || running(&self.profile_backfill) {
            return;
        }
        self.restart_requested = false;
        let projection_store = services.projection_store.as_ref();
        if let Err(error) =
            restart_own_author_edge_sweep(projection_store, self.author_pubkey.as_str()).await
        {
            warn!(
                author_pubkey = %self.author_pubkey,
                error = %error,
                "failed to restart the own follow and block edge reading"
            );
        }
        if let Err(error) =
            restart_own_profile_index_backfill(projection_store, self.author_pubkey.as_str()).await
        {
            warn!(
                author_pubkey = %self.author_pubkey,
                error = %error,
                "failed to restart the profile index backfill"
            );
        }
        // 位置を戻した後で依頼を消す(先に消すと、その間に張り直されたとき依頼が失われる)。
        if let Err(error) = projection_store
            .put_sync_checkpoint(&self.restart_checkpoint_key(), CHECKPOINT_DONE)
            .await
        {
            warn!(
                author_pubkey = %self.author_pubkey,
                error = %error,
                "failed to clear the restart request of the own replica work"
            );
        }
        self.edge_sweep = Some(spawn_own_edge_sweep(services, self.author_pubkey.as_str()));
        self.profile_backfill = Some(spawn_profile_index_backfill(
            services,
            self.author_pubkey.as_str(),
        ));
    }

    /// 自分の replica の entry の event。投稿・repost の key なら索引を足し、本体がまだ無ければ覚える。
    /// 自分の docs author が書いた entry(今の版の投稿。索引も同時に書く)は覚えない。
    pub(crate) async fn on_entry(
        &mut self,
        services: &ServiceHandles,
        key: &str,
        entry_docs_author: Option<&str>,
    ) {
        self.load_pending(services).await;
        if self.try_index(services, key).await != OwnProfileIndex::NotReadable {
            return;
        }
        if let Some(entry_docs_author) = entry_docs_author
            && services
                .docs_sync
                .local_docs_author()
                .await
                .ok()
                .flatten()
                .as_deref()
                == Some(entry_docs_author)
        {
            return;
        }
        if self.remember(key) {
            if self.restart_requested {
                // 覚えきれずに捨てた。依頼を先に残してから、空になった一覧を保存する。
                self.request_restart(services).await;
            }
            self.save_pending(services).await;
        }
    }

    /// 本体の到着・同期の区切りのときに、覚えている key を試し直す。
    pub(crate) async fn retry_pending(&mut self, services: &ServiceHandles) {
        self.load_pending(services).await;
        if self.pending_profile_keys.is_empty() {
            return;
        }
        let before = self.pending_profile_keys.len();
        for _ in 0..self.pending_profile_keys.len() {
            let Some(key) = self.pending_profile_keys.pop_front() else {
                break;
            };
            if self.try_index(services, key.as_str()).await == OwnProfileIndex::NotReadable {
                self.pending_profile_keys.push_back(key);
            }
        }
        if self.pending_profile_keys.len() != before {
            self.save_pending(services).await;
        }
    }

    /// 覚える。覚えている key が変わったら真。
    fn remember(&mut self, key: &str) -> bool {
        if self
            .pending_profile_keys
            .iter()
            .any(|pending| pending == key)
        {
            return false;
        }
        if self.pending_profile_keys.len() >= PENDING_PROFILE_KEYS {
            // 覚えきれない。覚えている分を捨て、補完のやり直しで拾う。
            self.pending_profile_keys.clear();
            self.restart_requested = true;
            return true;
        }
        self.pending_profile_keys.push_back(key.to_string());
        true
    }

    async fn try_index(&self, services: &ServiceHandles, key: &str) -> OwnProfileIndex {
        match index_own_profile_key(
            services.docs_sync.as_ref(),
            self.author_pubkey.as_str(),
            key,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                warn!(
                    author_pubkey = %self.author_pubkey,
                    key = %key,
                    error = %error,
                    "failed to index an own profile post"
                );
                OwnProfileIndex::NotReadable
            }
        }
    }
}

/// 自分の replica の follow・block の edge を、背景で小分けに読む task を起動する。
fn spawn_own_edge_sweep(services: &ServiceHandles, author_pubkey: &str) -> AbortOnDrop {
    let services = services.clone();
    let author_pubkey = author_pubkey.to_string();
    AbortOnDrop(tokio::spawn(async move {
        if let Err(error) = sweep_own_author_edges(&services, author_pubkey.as_str()).await {
            warn!(
                author_pubkey = %author_pubkey,
                error = %error,
                "failed to read the own follow and block edges"
            );
        }
    }))
}

/// 自分の replica のプロフィールの索引を、背景で補う task を起動する。
fn spawn_profile_index_backfill(services: &ServiceHandles, author_pubkey: &str) -> AbortOnDrop {
    let docs_sync = Arc::clone(&services.docs_sync);
    let projection_store = Arc::clone(&services.projection_store);
    let author_pubkey = author_pubkey.to_string();
    AbortOnDrop(tokio::spawn(async move {
        if let Err(error) = backfill_own_profile_index(
            docs_sync.as_ref(),
            projection_store.as_ref(),
            author_pubkey.as_str(),
        )
        .await
        {
            warn!(
                author_pubkey = %author_pubkey,
                error = %error,
                "failed to backfill the profile index"
            );
        }
    }))
}
