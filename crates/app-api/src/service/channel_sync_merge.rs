//! 本人の別の端末の private channel の item（参加・世代の鍵・担当・鍵更新の依頼）の merge（ADR 0061 §9、#1218 AC-4c、
//! #1219 AC-3）。
//!
//! 参加・退会と依頼の版は採用の台帳（`AccountSyncStore`）の `(updated_at, op_id)` で採り、鍵は追加だけ、担当は
//! ADR 0018 §8 の規則で採る。参加者の item（#1219 AC-5）は `epoch_control_support` が参加者の表へ取り込む。各 merge は
//! 対象の channel の行だけを読む。呼び出し元は差分の取得（`account_sync_fetch`）。

use kukuri_core::{
    AccountSyncItem, AccountSyncItemKey, ChannelControllerRequestV1, ChannelMembershipV1,
    ChannelRotationRequestV1,
};
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};
use kukuri_store::{PrivateChannelEpochRange, PrivateChannelRow};

use super::account_sync_support::row_of;
use super::private_channel_rows::{audience_name, epoch_started_at};
use super::*;

/// 鍵待ちのときに、手元の account の replica から読み直すその channel の鍵の item の page（ADR 0061 §5）。
const CHANNEL_EPOCH_ITEM_PAGE: usize = 64;

/// 担当の記録を採るか（ADR 0018 §8）。世代の大きい方を採り、同じ世代で同じ端末なら、移譲中の記録を採る。
/// 同じ世代で端末が異なる記録（古い backup の復元で起きる。#1219 AC-4）は、端末 ID の大きい方を採る（どの端末でも
/// 同じ記録に決まる）。同じ世代・同じ端末でそれ以外の食い違いは、手元の記録を保つ。
fn controller_supersedes(
    local: Option<&PrivateChannelController>,
    incoming: &PrivateChannelController,
) -> bool {
    local.is_none_or(|local| {
        incoming.generation > local.generation
            || (incoming.generation == local.generation
                && if incoming.device_id == local.device_id {
                    local.transfer_to.is_none() && incoming.transfer_to.is_some()
                } else {
                    incoming.device_id > local.device_id
                })
    })
}

/// 担当の記録の account 同期の item。
pub(super) fn controller_item(
    channel_id: &ChannelId,
    controller: &PrivateChannelController,
) -> Result<AccountSyncItem> {
    Ok(AccountSyncItem::edit(
        AccountSyncItemKey::ChannelController {
            channel_id: channel_id.clone(),
        },
        Utc::now().timestamp_millis(),
        Some(serde_json::to_value(controller)?),
    ))
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
            AccountSyncItemKey::ChannelRotationRequest { channel_id } => {
                self.merge_channel_rotation_request(channel_id, item).await
            }
            AccountSyncItemKey::ChannelControllerRequest { channel_id } => {
                self.merge_channel_controller_request(channel_id, item)
                    .await
            }
            AccountSyncItemKey::ChannelParticipant {
                channel_id,
                participant,
            } => {
                self.merge_channel_participant(channel_id, participant, item)
                    .await
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
        if !adopted && !self.holds_account_sync_item(item).await? {
            return Ok(false);
        }
        let row = store.get_private_channel_by_id(channel_id.as_str()).await?;
        let previous_controller = row.as_ref().and_then(|row| row.controller.clone());
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
        // 担当の item は key の順で参加の item より先に届く（周回・作り直し）。届いて手元の replica に置いたものを採る。
        let controller = match previous_controller {
            Some(controller) => Some(controller),
            None => self
                .read_account_sync_docs_key(
                    &AccountSyncItemKey::ChannelController {
                        channel_id: channel_id.clone(),
                    }
                    .docs_key(),
                    DocFetchPolicy::LocalOnly,
                )
                .await?
                .and_then(|item| item.value)
                .map(|value| {
                    serde_json::from_value::<PrivateChannelController>(value)
                        .and_then(|controller| serde_json::to_string(&controller))
                })
                .transpose()?,
        };
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
                        controller,
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

    /// 担当の記録を ADR 0018 §8 の規則で採る（書くのは担当になった端末）。参加の行がまだ無ければ（担当の item は key の
    /// 順で参加の item より先に届く）、手元の replica の記録より新しいときだけ採って自分の replica に置き、参加の行を
    /// 作るときに手元の replica から読む。同じ版は採らない（行の無い端末どうしの往復を止める。ADR 0061 §10）。
    async fn merge_channel_controller(
        &self,
        channel_id: &ChannelId,
        item: &AccountSyncItem,
    ) -> Result<bool> {
        let Some(value) = item.value.clone() else {
            return Ok(false);
        };
        let incoming: PrivateChannelController = serde_json::from_value(value)?;
        let device_id = self.local_device_id().await?;
        let adopted = {
            let _access = self.services.content_save_access.lock().await;
            let store = &self.services.projection_store;
            match store.get_private_channel_by_id(channel_id.as_str()).await? {
                None => None,
                Some(row) => {
                    let local = row
                        .controller
                        .as_deref()
                        .map(serde_json::from_str::<PrivateChannelController>)
                        .transpose()?;
                    // 移譲先がこの端末なら、取り込む代わりに次の世代で有効になる（取り込みと有効化を 1 回の書込みに
                    // する。#1219 AC-4）。有効化の行を書いた後に止まっていれば、停止の記録の再受信で同じ記録を書き直す。
                    let activation = (row.joined
                        && incoming.transfer_to.as_deref() == Some(device_id.as_str()))
                    .then(|| PrivateChannelController {
                        device_id: device_id.clone(),
                        generation: incoming.generation + 1,
                        transfer_to: None,
                    });
                    if !controller_supersedes(local.as_ref(), &incoming)
                        && (activation.is_none() || local != activation)
                    {
                        return Ok(false);
                    }
                    let next = activation.clone().unwrap_or_else(|| incoming.clone());
                    let activation = activation
                        .map(|next| controller_item(channel_id, &next))
                        .transpose()?;
                    let topic_id = row.joined.then(|| row.topic_id.clone());
                    self.put_private_channel_controller(row, &next, activation.as_ref())
                        .await?;
                    Some((topic_id, activation))
                }
            }
        };
        let Some((topic_id, activation)) = adopted else {
            let local = self
                .read_account_sync_docs_key(&item.key.docs_key(), DocFetchPolicy::LocalOnly)
                .await?
                .and_then(|item| item.value)
                .map(serde_json::from_value::<PrivateChannelController>)
                .transpose()?;
            return Ok(controller_supersedes(local.as_ref(), &incoming));
        };
        if let Some(activation) = &activation {
            self.write_account_sync_item(activation).await?;
        }
        if let Some(topic_id) = topic_id {
            self.restart_scope_subscription(&ScopeKey::Channel(
                topic_id,
                channel_id.as_str().to_string(),
            ))
            .await;
        }
        // 有効化したら、受けた停止の記録は採っていない（採ったとすると、取得がその記録を同じ key で自分の replica へ
        // 書き、有効化の記録を上書きする）。
        Ok(activation.is_none())
    }

    /// 鍵更新の依頼を `(updated_at, op_id)` で採り、この端末が担当で、現在の世代が依頼の元の世代なら鍵を更新する
    /// （#1219 AC-3、ADR 0018 §8）。同じ版の再受信でもやり直す（鍵更新が途中で失敗したら、差分の取得の再送で再開する）。
    /// 担当でなければ何もしない。採った（行を置き換えた）ら true。
    async fn merge_channel_rotation_request(
        &self,
        channel_id: &ChannelId,
        item: &AccountSyncItem,
    ) -> Result<bool> {
        let request: ChannelRotationRequestV1 = serde_json::from_value(
            item.value
                .clone()
                .context("a rotation request must carry its epoch")?,
        )?;
        let store = &self.services.projection_store;
        let adopted = store.adopt_account_sync_row(&row_of(item)?).await?;
        let current = adopted || self.holds_account_sync_item(item).await?;
        let Some(row) = store
            .get_private_channel_by_id(channel_id.as_str())
            .await?
            .filter(|row| {
                current
                    && row.joined
                    && row.current_epoch_id == request.from_epoch_id
                    && row.owner_pubkey == self.current_author_pubkey()
            })
        else {
            return Ok(adopted);
        };
        match self
            .rotate_private_channel(&row.topic_id, channel_id.as_str())
            .await
        {
            Err(error) if !error.is::<PrivateChannelControllerPending>() => Err(error),
            _ => Ok(adopted),
        }
    }

    /// 担当の移譲の依頼を `(updated_at, op_id)` で採り、この端末が依頼の世代の担当なら、移譲先への停止の記録を書く
    /// （#1219 AC-4、ADR 0018 §8）。停止の記録の書込みが、応じたことの印になる。既に同じ停止を行に書いていれば
    /// （行の後・台帳の前で止まった後の再受信）、同じ記録を書き直す。既に移った世代・別の移譲先への引継ぎ中では何も
    /// しない。採った（行を置き換えた）ら true。
    async fn merge_channel_controller_request(
        &self,
        channel_id: &ChannelId,
        item: &AccountSyncItem,
    ) -> Result<bool> {
        let request: ChannelControllerRequestV1 = serde_json::from_value(
            item.value
                .clone()
                .context("a controller request must carry its target")?,
        )?;
        let store = &self.services.projection_store;
        let adopted = store.adopt_account_sync_row(&row_of(item)?).await?;
        let device_id = self.local_device_id().await?;
        if request.to_device_id == device_id {
            return Ok(adopted);
        }
        let stop = PrivateChannelController {
            device_id: device_id.clone(),
            generation: request.generation,
            transfer_to: Some(request.to_device_id),
        };
        let stopped = {
            let _access = self.services.content_save_access.lock().await;
            let Some(row) = store
                .get_private_channel_by_id(channel_id.as_str())
                .await?
                .filter(|row| row.joined && row.owner_pubkey == self.current_author_pubkey())
            else {
                return Ok(adopted);
            };
            let local = row
                .controller
                .as_deref()
                .map(serde_json::from_str::<PrivateChannelController>)
                .transpose()?;
            if !local.is_some_and(|local| {
                local.device_id == device_id
                    && local.generation == stop.generation
                    && (local.transfer_to.is_none() || local.transfer_to == stop.transfer_to)
            }) {
                return Ok(adopted);
            }
            let item = controller_item(channel_id, &stop)?;
            let topic_id = row.topic_id.clone();
            self.put_private_channel_controller(row, &stop, Some(&item))
                .await?;
            (topic_id, item)
        };
        self.write_account_sync_item(&stopped.1).await?;
        self.restart_scope_subscription(&ScopeKey::Channel(
            stopped.0,
            channel_id.as_str().to_string(),
        ))
        .await;
        Ok(adopted)
    }

    /// 参加の行の担当の記録を置き換え、書く item があれば採用の台帳へ足す（`content_save_access` の中で呼ぶ）。replica
    /// へは呼び出し元が排他の外で書く。台帳の後に止まれば送り直しで（ADR 0061 §10）、行の後・台帳の前に止まれば同じ
    /// 遷移のやり直し（依頼・停止の記録の再受信、復元の引取りの次の起動）が書き直して届く。
    pub(crate) async fn put_private_channel_controller(
        &self,
        row: PrivateChannelRow,
        controller: &PrivateChannelController,
        item: Option<&AccountSyncItem>,
    ) -> Result<()> {
        let store = &self.services.projection_store;
        store
            .put_private_channel(
                &PrivateChannelRow {
                    controller: Some(serde_json::to_string(controller)?),
                    ..row
                },
                &[],
            )
            .await?;
        if let Some(item) = item {
            store.adopt_account_sync_row(&row_of(item)?).await?;
        }
        Ok(())
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
