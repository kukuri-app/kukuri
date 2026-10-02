//! private channel の参加と世代の鍵の行（ADR 0061 §9）。正本は store の行で、メモリは `Channel` の key に lease の
//! ある channel の参加状態だけを持つ。各操作は対象の行だけを読み書きし、全件の読み込み・書き直しをしない。

use kukuri_core::{
    AccountSyncItem, AccountSyncItemKey, PrivateChannelKeyRowSeal, receive_epoch_key_id,
};
use kukuri_docs_sync::PrivateEpochSecrets;
use kukuri_store::{
    PrivateChannelEpochRange, PrivateChannelEpochRow, PrivateChannelFilter, PrivateChannelRow,
};

use super::remote_read_support::epoch_start_millis;
use super::*;

/// 参加中の channel を 1 回に読む行数（topic の一覧・起動時の復元・退会の鍵の削除。ADR 0055 の参加者の page と同じ）。
pub(crate) const PRIVATE_CHANNEL_PAGE: usize = 128;
/// 過去の世代を 1 回に読む上限。
pub(crate) const PRIVATE_CHANNEL_EPOCH_WINDOW: usize = 8;

/// 世代の開始時刻（epoch id の時刻。`legacy` は最小）。
pub(crate) fn epoch_started_at(epoch_id: &str) -> i64 {
    epoch_start_millis(epoch_id).unwrap_or(i64::MIN)
}

/// generation を除いて同じ参加状態か。
pub(crate) fn same_private_channel_state(
    current: &JoinedPrivateChannelState,
    next: &JoinedPrivateChannelState,
) -> bool {
    *current
        == JoinedPrivateChannelState {
            generation: current.generation,
            ..next.clone()
        }
}

/// docs が、登録簿に無い世代の replica の秘密を引く参照（store の世代の鍵の行）。
struct StoredEpochSecrets {
    store: Arc<dyn ProjectionStore>,
    seal: PrivateChannelKeyRowSeal,
}

#[async_trait::async_trait]
impl PrivateEpochSecrets for StoredEpochSecrets {
    async fn epoch_secret_hex(&self, channel_id: &str, epoch_id: &str) -> Option<String> {
        let row = self
            .store
            .get_private_channel_epoch(channel_id, epoch_id)
            .await
            .ok()??;
        self.seal
            .open(channel_id, epoch_id, &row.sealed_secret)
            .inspect_err(|error| warn!(channel_id, epoch_id, %error, "private channel key row is unreadable"))
            .ok()
    }
}

impl ServiceHandles {
    fn key_row_seal(&self) -> PrivateChannelKeyRowSeal {
        self.keys.derive_private_channel_key_row_seal()
    }

    pub(crate) fn private_channel_epoch_row(
        &self,
        channel_id: &str,
        epoch_id: &str,
        secret_hex: &str,
        updated_at: i64,
    ) -> Result<PrivateChannelEpochRow> {
        let mut secret = [0_u8; 32];
        hex::decode_to_slice(secret_hex.trim(), &mut secret)?;
        Ok(PrivateChannelEpochRow {
            channel_id: channel_id.to_string(),
            epoch_id: epoch_id.to_string(),
            started_at: epoch_started_at(epoch_id),
            receive_key_id: receive_epoch_key_id(&secret, channel_id, epoch_id)?,
            updated_at,
            sealed_secret: self.key_row_seal().seal(channel_id, epoch_id, secret_hex)?,
        })
    }

    fn open_epoch_row(&self, row: PrivateChannelEpochRow) -> Result<(String, String)> {
        let secret =
            self.key_row_seal()
                .open(&row.channel_id, &row.epoch_id, &row.sealed_secret)?;
        Ok((row.epoch_id, secret))
    }

    /// (channel, epoch) の秘密の hex。行が無ければ `None`。
    pub(crate) async fn private_channel_epoch_secret(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<Option<String>> {
        self.projection_store
            .get_private_channel_epoch(channel_id, epoch_id)
            .await?
            .map(|row| self.open_epoch_row(row).map(|(_, secret)| secret))
            .transpose()
    }

    /// 開始時刻の範囲の世代（epoch id と秘密）を、上限つきで読む。
    pub(crate) async fn private_channel_epochs(
        &self,
        channel_id: &str,
        range: PrivateChannelEpochRange,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        self.projection_store
            .list_private_channel_epochs(channel_id, range, limit)
            .await?
            .into_iter()
            .map(|row| self.open_epoch_row(row))
            .collect()
    }

    /// 開始時刻の範囲の世代の ID を、上限つきで読む(秘密は開かない)。
    pub(crate) async fn private_channel_epoch_ids(
        &self,
        channel_id: &str,
        range: PrivateChannelEpochRange,
        limit: usize,
    ) -> Result<Vec<String>> {
        Ok(self
            .projection_store
            .list_private_channel_epochs(channel_id, range, limit)
            .await?
            .into_iter()
            .map(|row| row.epoch_id)
            .collect())
    }

    /// 参加の行から参加状態を作る。参加中で、現在の世代の鍵の行があるときだけ。
    pub(crate) async fn private_channel_state_from_row(
        &self,
        row: PrivateChannelRow,
    ) -> Result<Option<JoinedPrivateChannelState>> {
        if !row.joined {
            return Ok(None);
        }
        let Some(current_epoch_secret_hex) = self
            .private_channel_epoch_secret(&row.channel_id, &row.current_epoch_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(JoinedPrivateChannelState {
            generation: 0,
            topic_id: row.topic_id,
            channel_id: ChannelId::new(row.channel_id),
            label: row.label,
            creator_pubkey: row.creator_pubkey,
            owner_pubkey: row.owner_pubkey,
            joined_via_pubkey: row.joined_via_pubkey,
            audience_kind: serde_json::from_value(serde_json::Value::String(row.audience_kind))?,
            current_epoch_id: row.current_epoch_id,
            current_epoch_secret_hex,
            controller: row
                .controller
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
        }))
    }

    /// 参加状態。メモリ（lease のある channel）に無ければ、参加の行を key で読む。
    pub(crate) async fn joined_private_channel_state(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Option<JoinedPrivateChannelState> {
        let key = joined_private_channel_key(topic_id, channel_id);
        if let Some(state) = self.joined_private_channels.lock().await.get(&key) {
            return Some(state.clone());
        }
        self.stored_private_channel_state(&key)
            .await
            .inspect_err(|error| warn!(%key, %error, "private channel row is unreadable"))
            .ok()
            .flatten()
    }

    pub(crate) async fn stored_private_channel_state(
        &self,
        key: &str,
    ) -> Result<Option<JoinedPrivateChannelState>> {
        let Some(row) = self.projection_store.get_private_channel(key).await? else {
            return Ok(None);
        };
        self.private_channel_state_from_row(row).await
    }

    /// 受信 route の識別子で、参加中の channel の世代を 1 件引く（参加状態、epoch id、秘密）。
    pub(crate) async fn private_channel_epoch_by_receive_key(
        &self,
        epoch_key_id: &str,
    ) -> Result<Option<(JoinedPrivateChannelState, String, [u8; 32])>> {
        let Some(epoch) = self
            .projection_store
            .find_private_channel_epoch(epoch_key_id)
            .await?
        else {
            return Ok(None);
        };
        let Some(row) = self
            .projection_store
            .get_private_channel_by_id(&epoch.channel_id)
            .await?
        else {
            return Ok(None);
        };
        let Some(state) = self
            .joined_private_channel_state(&row.topic_id, &row.channel_id)
            .await
        else {
            return Ok(None);
        };
        let (epoch_id, secret_hex) = self.open_epoch_row(epoch)?;
        let mut secret = [0_u8; 32];
        hex::decode_to_slice(&secret_hex, &mut secret)?;
        Ok(Some((state, epoch_id, secret)))
    }

    /// lease の外れた channel の参加状態をメモリから捨てる（行は残る）。
    pub(crate) async fn evict_private_channels(&self, released: &[ScopeKey]) {
        let mut joined = self.joined_private_channels.lock().await;
        let mut evicted = false;
        for key in released {
            if let ScopeKey::Channel(topic, channel) = key {
                evicted |= joined
                    .remove(&joined_private_channel_key(topic, channel))
                    .is_some();
            }
        }
        drop(joined);
        if evicted {
            let generation = self
                .content_scope_generation
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .saturating_add(1);
            self.content_scope_changes.send_replace(generation);
        }
    }
}

impl AppService {
    pub(crate) async fn joined_private_channel_state(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Option<JoinedPrivateChannelState> {
        self.services
            .joined_private_channel_state(topic_id, channel_id)
            .await
    }

    /// docs が参加中の channel の世代の秘密を store の行から引けるようにする。docs を作り直したら入れ直す。
    pub(crate) async fn install_private_epoch_secrets(&self) -> Result<()> {
        self.docs_sync()
            .install_private_epoch_secrets(Arc::new(StoredEpochSecrets {
                store: self.services.projection_store.clone(),
                seal: self.services.key_row_seal(),
            }))
            .await
    }

    /// 参加の行と、現在の世代（と `archived` の過去の世代）の鍵の行を書く（全件を書き直さない）。参加し直したとき
    /// だけ参加の版を新しくする。
    pub(crate) async fn persist_private_channel(
        &self,
        state: &JoinedPrivateChannelState,
        updated_at: i64,
        archived: &[PrivateChannelEpochCapability],
    ) -> Result<()> {
        let key = joined_private_channel_key(&state.topic_id, state.channel_id.as_str());
        let version = match self
            .services
            .projection_store
            .get_private_channel(&key)
            .await?
        {
            Some(row) if row.joined => (row.updated_at, row.op_id),
            _ => {
                let item = AccountSyncItem::edit(
                    AccountSyncItemKey::ChannelLeave {
                        channel_id: state.channel_id.clone(),
                    },
                    updated_at,
                    None,
                );
                (item.updated_at, item.op_id)
            }
        };
        let row = private_channel_row(state, true, version)?;
        let epochs = std::iter::once((&state.current_epoch_id, &state.current_epoch_secret_hex))
            .chain(
                archived
                    .iter()
                    .map(|epoch| (&epoch.epoch_id, &epoch.namespace_secret_hex)),
            )
            .map(|(epoch_id, secret_hex)| {
                self.services.private_channel_epoch_row(
                    state.channel_id.as_str(),
                    epoch_id,
                    secret_hex,
                    updated_at,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        self.services
            .projection_store
            .put_private_channel(&row, &epochs)
            .await?;
        self.private_channel_rows_changed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// 旧 registry の 1 件を、参加の行と世代の鍵の行へ移す(ADR 0061 §9。起動時の移行の入口で、試験も別の端末の
    /// 状態をこれで作る)。移した行の時刻は 0(どの編集よりも古い)。
    pub async fn restore_private_channel_capability(
        &self,
        capability: PrivateChannelCapability,
    ) -> Result<()> {
        let unrecorded = capability.controller.is_none();
        let archived = capability.archived_epochs.clone();
        let mut state = joined_private_channel_state_from_capability(capability)?;
        // #1219 W6: 担当の欄が無い本変更前の保存の自分の channel は、保存していた端末を担当にする。
        if unrecorded && state.owner_pubkey == self.current_author_pubkey() {
            state.controller = Some(self.first_controller().await?);
        }
        self.install_private_epoch_secrets().await?;
        self.persist_private_channel(&state, 0, &archived).await?;
        self.restore_joined_private_channel(state.topic_id.as_str(), state.channel_id.as_str())
            .await;
        Ok(())
    }
    /// topic の参加中の channel を、`cursor` の後から 1 page(128 件まで)。続きは `next_cursor` で読む。
    pub async fn list_joined_private_channels(
        &self,
        topic_id: &str,
        cursor: Option<&str>,
    ) -> Result<JoinedPrivateChannelPage> {
        let after = cursor.unwrap_or_default();
        let (states, _) = self
            .joined_private_channel_states_for_topic(topic_id, after)
            .await?;
        for state in states {
            self.maybe_redeem_epoch_handoff_grants_for_channel(topic_id, state.channel_id.as_str())
                .await?;
        }
        let (states, next_cursor) = self
            .joined_private_channel_states_for_topic(topic_id, after)
            .await?;
        let mut items = Vec::with_capacity(states.len());
        for state in states {
            items.push(self.joined_private_channel_view_for_state(&state).await?);
        }
        Ok(JoinedPrivateChannelPage { items, next_cursor })
    }
    /// 起動時の復元（ADR 0061 §9）。参加中の行を key の順に読み、参加の holder の lease を取る。上限に達したら
    /// 読むのをやめる（残りは購読しない。行は残る）。参加状態は lease の task がメモリへ読む。
    pub async fn restore_joined_private_channels(&self) -> Result<()> {
        self.install_private_epoch_secrets().await?;
        let mut after = String::new();
        loop {
            let page = self
                .services
                .projection_store
                .list_joined_private_channels(
                    PrivateChannelFilter::All,
                    &after,
                    PRIVATE_CHANNEL_PAGE,
                )
                .await?;
            for row in &page {
                if let Err(error) = self
                    .hold_joined_private_channel(&row.topic_id, &row.channel_id)
                    .await
                {
                    if error.downcast_ref::<ScopeLimitReached>().is_none() {
                        return Err(error);
                    }
                    warn!(
                        topic = %row.topic_id,
                        channel_id = %row.channel_id,
                        "private channels are not subscribed beyond the active scope limit"
                    );
                    return Ok(());
                }
            }
            match page.last() {
                Some(last) if page.len() == PRIVATE_CHANNEL_PAGE => {
                    after = last.channel_key.clone();
                }
                _ => return Ok(()),
            }
        }
    }

    /// `Channel` の key の lease の task を起こすときに、参加の行をメモリへ読む。同じ内容なら generation を変えない。
    pub(crate) async fn load_leased_private_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Option<JoinedPrivateChannelState> {
        let key = joined_private_channel_key(topic_id, channel_id);
        let stored = self
            .services
            .stored_private_channel_state(&key)
            .await
            .inspect_err(|error| warn!(%key, %error, "private channel row is unreadable"))
            .ok()
            .flatten()?;
        let mut joined = self.joined_private_channels.lock().await;
        if let Some(current) = joined.get(&key)
            && same_private_channel_state(current, &stored)
        {
            return Some(current.clone());
        }
        let generation = self
            .services
            .content_scope_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        let state = JoinedPrivateChannelState {
            generation,
            ..stored
        };
        joined.insert(key, state.clone());
        drop(joined);
        self.services.content_scope_changes.send_replace(generation);
        Some(state)
    }

    /// topic の参加中の channel の、`after` の後の 1 page と、続きの cursor(page が埋まったときだけ)。
    pub(crate) async fn joined_private_channel_states_for_topic(
        &self,
        topic_id: &str,
        after: &str,
    ) -> Result<(Vec<JoinedPrivateChannelState>, Option<String>)> {
        let rows = self
            .services
            .projection_store
            .list_joined_private_channels(
                PrivateChannelFilter::Topic(topic_id),
                after,
                PRIVATE_CHANNEL_PAGE,
            )
            .await?;
        let next_cursor = rows
            .last()
            .filter(|_| rows.len() == PRIVATE_CHANNEL_PAGE)
            .map(|row| row.channel_key.clone());
        let mut states = Vec::with_capacity(rows.len());
        for row in rows {
            let key = row.channel_key.clone();
            if let Some(state) = self.joined_private_channels.lock().await.get(&key) {
                states.push(state.clone());
            } else if let Some(state) = self.services.private_channel_state_from_row(row).await? {
                states.push(state);
            }
        }
        Ok((states, next_cursor))
    }

    /// 参加中の channel の現在の世代の replica を、key の順に `after` から `limit` 件（#1221 R5-G）。
    pub async fn joined_private_channel_replicas(
        &self,
        after: &str,
        limit: usize,
    ) -> Result<Vec<(String, ReplicaId)>> {
        Ok(self
            .services
            .projection_store
            .list_joined_private_channels(PrivateChannelFilter::All, after, limit)
            .await?
            .into_iter()
            .map(|row| {
                let replica =
                    private_channel_replica_for_epoch(&row.channel_id, &row.current_epoch_id);
                (row.channel_key, replica)
            })
            .collect())
    }

    /// 退会の印（参加の行を tombstone にし、メモリから外す）。鍵の行は `forget_private_channel_keys` で消す。
    pub(crate) async fn tombstone_private_channel(
        &self,
        state: &JoinedPrivateChannelState,
    ) -> Result<()> {
        let _access = self.services.content_save_access.lock().await;
        let key = joined_private_channel_key(&state.topic_id, state.channel_id.as_str());
        let now = Utc::now().timestamp_millis();
        let item = AccountSyncItem::edit(
            AccountSyncItemKey::ChannelLeave {
                channel_id: state.channel_id.clone(),
            },
            now,
            None,
        );
        let row = private_channel_row(state, false, (item.updated_at, item.op_id))?;
        self.services
            .projection_store
            .put_private_channel(&row, &[])
            .await?;
        self.private_channel_rows_changed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.joined_private_channels.lock().await.remove(&key);
        let generation = self
            .services
            .content_scope_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        self.services.content_scope_changes.send_replace(generation);
        Ok(())
    }

    /// 退会の後に、その channel の世代の鍵の行を page で消しながら、その世代の replica の参照を外し、lease を外す。
    pub(crate) async fn forget_private_channel_keys(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        loop {
            let epochs = self
                .services
                .projection_store
                .delete_private_channel_epochs(channel_id, PRIVATE_CHANNEL_PAGE)
                .await?;
            if epochs.is_empty() {
                break;
            }
            let replicas = epochs
                .iter()
                .map(|epoch| private_channel_replica_for_epoch(channel_id, epoch))
                .collect::<Vec<_>>();
            self.services
                .session_projections
                .remove_replicas(&replicas)
                .await;
            for replica in &replicas {
                self.docs_sync()
                    .remove_private_replica_secret(replica)
                    .await?;
            }
        }
        self.release_scope_holder(&private_channel_holder(topic_id, channel_id))
            .await;
        // 列が channel を開いたままでも、参加していない channel は購読しない。
        self.restart_scope_subscription(&ScopeKey::Channel(
            topic_id.to_string(),
            channel_id.to_string(),
        ))
        .await;
        Ok(())
    }
}

fn private_channel_row(
    state: &JoinedPrivateChannelState,
    joined: bool,
    (updated_at, op_id): (i64, String),
) -> Result<PrivateChannelRow> {
    Ok(PrivateChannelRow {
        channel_key: joined_private_channel_key(&state.topic_id, state.channel_id.as_str()),
        topic_id: state.topic_id.clone(),
        channel_id: state.channel_id.as_str().to_string(),
        label: state.label.clone(),
        creator_pubkey: state.creator_pubkey.clone(),
        owner_pubkey: state.owner_pubkey.clone(),
        joined_via_pubkey: state.joined_via_pubkey.clone(),
        audience_kind: serde_json::to_value(&state.audience_kind)?
            .as_str()
            .context("audience kind is not a string")?
            .to_string(),
        current_epoch_id: state.current_epoch_id.clone(),
        controller: state
            .controller
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
        joined,
        updated_at,
        op_id,
    })
}
