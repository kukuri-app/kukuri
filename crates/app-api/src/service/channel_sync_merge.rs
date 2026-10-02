//! 本人の別の端末の private channel の item（参加・世代の鍵・担当）の merge（ADR 0061 §9、#1218 AC-4c）。
//!
//! 参加・退会の版は採用の台帳（`AccountSyncStore`）の `(updated_at, op_id)` で採り、鍵は追加だけ、担当は ADR 0018 §8 の
//! 規則で採る。各 merge は対象の channel の行だけを読む。呼び出し元は差分の取得（`account_sync_fetch`）。

use kukuri_core::{AccountSyncItem, AccountSyncItemKey, ChannelMembershipV1};
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};
use kukuri_store::{PrivateChannelEpochRange, PrivateChannelRow};

use super::account_sync_support::row_of;
use super::private_channel_rows::{audience_name, epoch_started_at};
use super::*;

/// 鍵待ちのときに、手元の account の replica から読み直すその channel の鍵の item の page（ADR 0061 §5）。
const CHANNEL_EPOCH_ITEM_PAGE: usize = 64;

/// 担当の記録を採るか（ADR 0018 §8）。世代の大きい方を採り、同じ世代で同じ端末なら、移譲中の記録を採る。
/// 同じ世代でそれ以外の食い違いは、手元の記録を保つ。
fn controller_supersedes(
    local: Option<&PrivateChannelController>,
    incoming: &PrivateChannelController,
) -> bool {
    local.is_none_or(|local| {
        incoming.generation > local.generation
            || (incoming.generation == local.generation
                && incoming.device_id == local.device_id
                && local.transfer_to.is_none()
                && incoming.transfer_to.is_some())
    })
}

impl AppService {
    pub(crate) async fn merge_channel_item(&self, item: &AccountSyncItem) -> Result<bool> {
        match &item.key {
            AccountSyncItemKey::ChannelMembership { channel_id } => {
                self.merge_channel_membership(channel_id, item).await
            }
            AccountSyncItemKey::ChannelCapability {
                channel_id,
                epoch_id,
            } => self.merge_channel_epoch(channel_id, epoch_id, item).await,
            AccountSyncItemKey::ChannelController { channel_id } => {
                self.merge_channel_controller(channel_id, item).await
            }
            _ => Ok(false),
        }
    }

    /// 参加（値）と退会・取消（tombstone）を `(updated_at, op_id)` で採る。採用済みの同じ版の再送でも、採用の後の
    /// 処理をやり直す（途中で止まった鍵待ちの読み直しを、差分の取得の再送で再開する）。採った（行を置き換えた）ら true。
    async fn merge_channel_membership(
        &self,
        channel_id: &ChannelId,
        item: &AccountSyncItem,
    ) -> Result<bool> {
        let membership = item
            .value
            .clone()
            .map(serde_json::from_value::<ChannelMembershipV1>)
            .transpose()?;
        let store = &self.services.projection_store;
        let adopted = store.adopt_account_sync_row(&row_of(item)?).await?;
        let current = adopted
            || store
                .get_account_sync_row(&item.key.docs_key())
                .await?
                .is_some_and(|row| row.updated_at == item.updated_at && row.op_id == item.op_id);
        if !current {
            return Ok(false);
        }
        let row = store.get_private_channel_by_id(channel_id.as_str()).await?;
        let Some(membership) = membership else {
            // 退会: 参加中なら、この端末の退会と同じ順で外す（鍵待ちの参加は記録を書いていないので書かない）。
            // 鍵の行は参加中でなくても消す（参加より先に届いた鍵、再送で途中から再開する削除）。
            let Some(row) = row else {
                self.delete_private_channel_keys(channel_id.as_str())
                    .await?;
                return Ok(adopted);
            };
            let (topic_id, channel) = (row.topic_id.clone(), row.channel_id.clone());
            if row.joined {
                match self
                    .services
                    .private_channel_state_from_row(row.clone())
                    .await?
                {
                    Some(state) => {
                        let left_at = self.tombstone_private_channel(&state, Some(item)).await?;
                        self.record_private_channel_leave(&state, left_at).await?;
                    }
                    None => {
                        let _access = self.services.content_save_access.lock().await;
                        store
                            .put_private_channel(
                                &PrivateChannelRow {
                                    joined: false,
                                    updated_at: item.updated_at,
                                    op_id: item.op_id.clone(),
                                    ..row
                                },
                                &[],
                            )
                            .await?;
                    }
                }
            }
            self.forget_private_channel_keys(&topic_id, &channel)
                .await?;
            return Ok(adopted);
        };
        if let Some(row) = row.filter(|row| row.joined) {
            // 参加中の channel では membership の現在の世代を使わない。版だけを写し、鍵待ちなら読み直す。
            let waiting = {
                let _access = self.services.content_save_access.lock().await;
                let waiting = store
                    .get_private_channel_epoch(channel_id.as_str(), &row.current_epoch_id)
                    .await?
                    .is_none();
                store
                    .put_private_channel(
                        &PrivateChannelRow {
                            updated_at: item.updated_at,
                            op_id: item.op_id.clone(),
                            ..row
                        },
                        &[],
                    )
                    .await?;
                waiting
            };
            if waiting {
                self.refill_channel_keys(channel_id).await?;
            }
            return Ok(adopted);
        }
        // 参加の無い channel: 鍵の行があれば開始時刻の最も新しい世代、無ければ membership の世代で鍵待ち。
        let newest = self
            .services
            .private_channel_epoch_ids(
                channel_id.as_str(),
                PrivateChannelEpochRange::AtOrBefore(i64::MAX),
                1,
            )
            .await?
            .pop();
        let waiting = newest.is_none();
        let topic_id = membership.topic_id.clone();
        {
            let _access = self.services.content_save_access.lock().await;
            self.services
                .projection_store
                .put_private_channel(
                    &PrivateChannelRow {
                        channel_key: joined_private_channel_key(&topic_id, channel_id.as_str()),
                        topic_id: topic_id.clone(),
                        channel_id: channel_id.as_str().to_string(),
                        label: membership.label,
                        creator_pubkey: membership.creator_pubkey,
                        owner_pubkey: membership.owner_pubkey,
                        joined_via_pubkey: membership.joined_via_pubkey,
                        audience_kind: audience_name(&membership.audience_kind)?,
                        current_epoch_id: newest.unwrap_or(membership.current_epoch_id),
                        controller: None,
                        joined: true,
                        updated_at: item.updated_at,
                        op_id: item.op_id.clone(),
                    },
                    &[],
                )
                .await?;
            self.private_channel_rows_changed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if waiting {
            self.refill_channel_keys(channel_id).await?;
        } else {
            self.start_synced_channel(&topic_id, channel_id.as_str(), item.updated_at)
                .await?;
        }
        Ok(adopted)
    }

    /// 世代の鍵は追加だけ。退会の版より古いか同じ鍵は保存しない。参加中の channel では、開始時刻の新しい鍵で現在の
    /// 世代を進める（本人の端末の鍵は検証済みの世代。ADR 0018 §8 の例外）。
    async fn merge_channel_epoch(
        &self,
        channel_id: &ChannelId,
        epoch_id: &str,
        item: &AccountSyncItem,
    ) -> Result<bool> {
        let capability: PrivateChannelEpochCapability = serde_json::from_value(
            item.value
                .clone()
                .context("an epoch item must carry its key")?,
        )?;
        anyhow::ensure!(
            capability.epoch_id == epoch_id,
            "the epoch item does not match its key"
        );
        let membership_key = AccountSyncItemKey::ChannelMembership {
            channel_id: channel_id.clone(),
        }
        .docs_key();
        if self
            .services
            .projection_store
            .get_account_sync_row(&membership_key)
            .await?
            .is_some_and(|row| row.value.is_none() && row.updated_at >= item.updated_at)
        {
            return Ok(false);
        }
        let epoch_row = self.services.private_channel_epoch_row(
            channel_id.as_str(),
            epoch_id,
            &capability.namespace_secret_hex,
            item.updated_at,
        )?;
        let advanced = {
            let _access = self.services.content_save_access.lock().await;
            let store = &self.services.projection_store;
            if store
                .get_private_channel_epoch(channel_id.as_str(), epoch_id)
                .await?
                .is_some()
            {
                return Ok(false);
            }
            match store
                .get_private_channel_by_id(channel_id.as_str())
                .await?
                .filter(|row| row.joined)
            {
                Some(row) => {
                    let waiting = store
                        .get_private_channel_epoch(channel_id.as_str(), &row.current_epoch_id)
                        .await?
                        .is_none();
                    if waiting || epoch_row.started_at > epoch_started_at(&row.current_epoch_id) {
                        let next = (row.topic_id.clone(), waiting, row.updated_at);
                        store
                            .put_private_channel(
                                &PrivateChannelRow {
                                    current_epoch_id: epoch_id.to_string(),
                                    ..row
                                },
                                std::slice::from_ref(&epoch_row),
                            )
                            .await?;
                        Some(next)
                    } else {
                        store.put_private_channel_epoch(&epoch_row).await?;
                        None
                    }
                }
                None => {
                    store.put_private_channel_epoch(&epoch_row).await?;
                    None
                }
            }
        };
        match advanced {
            Some((topic_id, true, membership_at)) => {
                self.start_synced_channel(&topic_id, channel_id.as_str(), membership_at)
                    .await?
            }
            Some((topic_id, false, _)) => {
                self.restart_scope_subscription(&ScopeKey::Channel(
                    topic_id,
                    channel_id.as_str().to_string(),
                ))
                .await
            }
            None => {}
        }
        Ok(true)
    }

    /// 担当の記録を ADR 0018 §8 の規則で採る（書くのは W6）。
    async fn merge_channel_controller(
        &self,
        channel_id: &ChannelId,
        item: &AccountSyncItem,
    ) -> Result<bool> {
        let Some(value) = item.value.clone() else {
            return Ok(false);
        };
        let incoming: PrivateChannelController = serde_json::from_value(value)?;
        let topic_id = {
            let _access = self.services.content_save_access.lock().await;
            let store = &self.services.projection_store;
            let Some(row) = store.get_private_channel_by_id(channel_id.as_str()).await? else {
                return Ok(false);
            };
            let local = row
                .controller
                .as_deref()
                .map(serde_json::from_str::<PrivateChannelController>)
                .transpose()?;
            if !controller_supersedes(local.as_ref(), &incoming) {
                return Ok(false);
            }
            let topic_id = row.joined.then(|| row.topic_id.clone());
            store
                .put_private_channel(
                    &PrivateChannelRow {
                        controller: Some(serde_json::to_string(&incoming)?),
                        ..row
                    },
                    &[],
                )
                .await?;
            topic_id
        };
        if let Some(topic_id) = topic_id {
            self.restart_scope_subscription(&ScopeKey::Channel(
                topic_id,
                channel_id.as_str().to_string(),
            ))
            .await;
        }
        Ok(true)
    }

    /// 鍵待ちの channel の鍵の item を、手元の account の replica から新しい順に 1 page 読み直す。退会を採ったときに
    /// 消した鍵は、差分の取得の cursor を既に過ぎているため。最初の page で現在の世代が決まる。
    async fn refill_channel_keys(&self, channel_id: &ChannelId) -> Result<()> {
        let keys = self.services.keys.derive_account_sync();
        let page = self
            .services
            .docs_sync
            .query_replica_keys(
                keys.replica_id(),
                DocKeyQuery {
                    prefix: format!("channel/{}/epoch/", hex::encode(channel_id.as_str())),
                    order: DocKeyOrder::Descending,
                    limit: CHANNEL_EPOCH_ITEM_PAGE,
                },
            )
            .await?;
        for entry in page.entries {
            if let Some(item) = self
                .read_account_sync_docs_key(&entry.key, DocFetchPolicy::LocalOnly)
                .await?
                && let AccountSyncItemKey::ChannelCapability {
                    channel_id,
                    epoch_id,
                } = &item.key
            {
                self.merge_channel_epoch(channel_id, epoch_id, &item)
                    .await?;
            }
        }
        Ok(())
    }

    /// 本人の別の端末から届いた参加で、鍵がそろった channel を使い始める。参加の holder の lease を取り（上限なら
    /// 購読しない）、この端末の参加の記録を、参加の版の時刻で owner へ書く。
    async fn start_synced_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
        membership_at: i64,
    ) -> Result<()> {
        self.restore_joined_private_channel(topic_id, channel_id)
            .await;
        let Some(state) = self
            .joined_private_channel_state(topic_id, channel_id)
            .await
        else {
            return Ok(());
        };
        let local = self.current_author_pubkey();
        self.record_private_channel_participant(
            &PrivateChannelParticipantDocV1 {
                channel_id: state.channel_id.clone(),
                topic_id: TopicId::new(topic_id),
                epoch_id: state.current_epoch_id.clone(),
                participant_pubkey: Pubkey::from(local.clone()),
                joined_at: membership_at,
                is_owner: state.owner_pubkey == local,
                join_mode: None,
                sponsor_pubkey: None,
                share_token_id: None,
                left_at: None,
            },
            &state.owner_pubkey,
            &state.current_epoch_secret_hex,
            &current_private_channel_replica_id(&state),
        )
        .await
    }
}
