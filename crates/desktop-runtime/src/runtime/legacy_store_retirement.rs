//! 旧 iroh store(`iroh-data`)の退役(#1221 R5-I)。
//!
//! node は新しい store で動き、旧 store は読むだけの別の instance として開く。背景 task が 1 回 128 対象以内で、
//! 保護移行(R5-G)の残りと、旧 store から新しい store・cache への移行(本人の docs entry・Dome の pin・最近の
//! 他人の projection と内容)を台帳の位置から進める。移し終えたら(ADR 0048 §7)旧 store を閉じて名前を変え、
//! 中の file を 1 回 128 件以内で消す。その後、更新前の版が読取りで作った空の namespace を 1 回 128 件以内で回収する
//! (#1407)。終えたら task は止まり、以後は常駐しない。

use std::time::Duration;

use kukuri_iroh_node::remove_dir_step;
use kukuri_store::{EMPTY_NAMESPACES_KIND, LEGACY_STORE_KINDS, LEGACY_STORE_PAGE};

use super::protected_migration::DOME_PIN_TAG_PREFIX;
use super::*;

/// 1 ステップの後の進め方。
pub(crate) enum LegacyStoreProgress {
    /// 続きがある(100ms 後に次を読む)。
    More,
    /// 移し終えたが、退役の条件を待つ(60 秒後)。
    Waiting,
    /// 旧 store は無い。
    Retired,
}

impl DesktopRuntime {
    /// 1 ステップ進める。
    pub(crate) async fn legacy_store_step(&self) -> Result<LegacyStoreProgress> {
        let retiring = self.db_path.with_extension("iroh-data.retiring");
        if self.legacy_store.lock().await.is_none() {
            if !retiring.exists() {
                self.protected_migration_step().await?;
                return self.empty_namespaces_step().await;
            }
            // 消し終えたら、次のステップで空の namespace の回収へ進む。
            tokio::task::spawn_blocking(move || remove_dir_step(&retiring, LEGACY_STORE_PAGE))
                .await??;
            return Ok(LegacyStoreProgress::More);
        }
        let migrated = self.protected_migration_step().await?;
        let _guard = self.protected_migration_guard.lock().await;
        let Some(legacy) = self.legacy_store.lock().await.clone() else {
            return Ok(LegacyStoreProgress::More);
        };
        let node = self
            .iroh_stack
            .current
            .lock()
            .await
            .as_ref()
            .context("missing active iroh stack")?
            .node
            .clone();
        let local = self.author_keys.public_key_hex();
        let mut caught_up = true;
        for kind in LEGACY_STORE_KINDS {
            let (cursor, done) = self.store.legacy_store_position(kind).await?;
            // 成人向けの hash の行は、本人の projection の行に参照を置き終えてから外す。
            if done || (kind == "adult_marker" && !caught_up) {
                caught_up &= done;
                continue;
            }
            let (next, done) = match kind {
                "own_entries" => {
                    legacy
                        .copy_own_entries(&node, &cursor, LEGACY_STORE_PAGE)
                        .await?
                }
                "pin_tags" => {
                    legacy
                        .copy_tags(
                            &node,
                            DOME_PIN_TAG_PREFIX,
                            &cursor,
                            LEGACY_STORE_PAGE,
                            &self.db_path.with_extension("legacy-pin.tmp"),
                        )
                        .await?
                }
                "legacy_projection" => {
                    let page = self
                        .store
                        .retire_legacy_projection_page(&local, &cursor)
                        .await?;
                    // 最近使った他人の内容は、予算に入る分だけ cache へ写す(入らなければ写さずに進む)。
                    for hash in &page.blobs {
                        if let Err(error) = self.copy_legacy_blob(&legacy, hash).await {
                            tracing::warn!(%error, hash, "recent legacy content was not cached");
                        }
                    }
                    (page.cursor, page.done)
                }
                _ => self.store.retire_legacy_adult_marker_page(&cursor).await?,
            };
            self.store
                .finish_legacy_store_page(kind, &next, done)
                .await?;
            caught_up &= done;
        }
        drop(legacy);
        if !(migrated && caught_up) {
            return Ok(LegacyStoreProgress::More);
        }
        if !self.store.legacy_store_retirable().await? {
            return Ok(LegacyStoreProgress::Waiting);
        }
        let Some(legacy) = self.legacy_store.lock().await.take() else {
            return Ok(LegacyStoreProgress::More);
        };
        match Arc::try_unwrap(legacy) {
            Ok(legacy) => legacy.close().await?,
            Err(shared) => {
                *self.legacy_store.lock().await = Some(shared);
                return Ok(LegacyStoreProgress::More);
            }
        }
        std::fs::rename(self.db_path.with_extension("iroh-data"), &retiring)
            .context("failed to rename the legacy iroh store for retirement")?;
        Ok(LegacyStoreProgress::More)
    }

    /// 旧 store の退役の後に、更新前の版が読取りで作った空の namespace を 1 回 128 件以内で回収する(#1407)。
    /// 終端を記録した後は namespace を列挙しない。
    async fn empty_namespaces_step(&self) -> Result<LegacyStoreProgress> {
        let (cursor, done) = self
            .store
            .legacy_store_position(EMPTY_NAMESPACES_KIND)
            .await?;
        if done {
            return Ok(LegacyStoreProgress::Retired);
        }
        let docs_sync = self
            .iroh_stack
            .current
            .lock()
            .await
            .as_ref()
            .context("missing active iroh stack")?
            .docs_sync
            .clone();
        let (next, done) = docs_sync
            .drop_empty_namespaces_step(&cursor, LEGACY_STORE_PAGE)
            .await?;
        self.store
            .finish_legacy_store_page(EMPTY_NAMESPACES_KIND, &next, done)
            .await?;
        Ok(if done {
            LegacyStoreProgress::Retired
        } else {
            LegacyStoreProgress::More
        })
    }

    /// 起動後の背景 task。続きがある間は 100ms、退役の条件を待つ間は 60 秒ごとに進め、空の namespace の回収を
    /// 終えれば止まる。
    /// shutdown で止める。
    pub async fn start_legacy_store_retirement(self: &Arc<Self>) {
        let mut task = self.legacy_store_task.lock().await;
        if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        let weak = Arc::downgrade(self);
        *task = Some(tokio::spawn(async move {
            loop {
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                let delay = match runtime.legacy_store_step().await {
                    Ok(LegacyStoreProgress::More) => Duration::from_millis(100),
                    Ok(LegacyStoreProgress::Waiting) => Duration::from_secs(60),
                    Ok(LegacyStoreProgress::Retired) => return,
                    Err(error) => {
                        tracing::warn!(%error, "legacy store retirement step failed");
                        Duration::from_secs(60)
                    }
                };
                drop(runtime);
                tokio::time::sleep(delay).await;
            }
        }));
    }

    /// 旧 store の file を閉じる(shutdown)。
    pub(crate) async fn close_legacy_store(&self) {
        if let Some(legacy) = self.legacy_store.lock().await.take()
            && let Ok(legacy) = Arc::try_unwrap(legacy)
            && let Err(error) = legacy.close().await
        {
            tracing::warn!(%error, "failed to close the legacy iroh store");
        }
    }
}
