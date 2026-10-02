//! 本人の別の端末からの差分の取得・変更の窓・送り直し・DB を失ったときの作り直し（ADR 0061 §10、#1218 AC-5b）。
//!
//! 各端末の account の replica は、その端末が採用した版を item ごとに 1 件持ち、採用のたびに端末ごとの変更の窓
//! （256 件）へ 1 件足す。読み手は相手ごとの cursor から窓を読み、窓を超えた相手・初めての相手とは prefix の木を
//! 辿る周回で追い付く。1 回の仕事は有界で、終わりに届くまで lease の task が背景で続ける。

use super::*;
use kukuri_core::{
    ACCOUNT_SYNC_CHANGE_WINDOW, AccountSyncChangeV1, AccountSyncItem, AccountSyncItemKey,
};
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};
use kukuri_store::{AccountSyncCursor, AccountSyncRow};

use super::account_sync_support::row_of;

/// 1 回に読む slot・item・key の数（ADR 0061 §5）。
const ACCOUNT_SYNC_PAGE: usize = 64;
/// 周回で辿る item の prefix（key の順）。
const CYCLE_ROOTS: [&str; 3] = ["profile", "trust/always-visible/", "channel/"];
/// item の key に使う文字（ASCII の順）。周回は prefix にこの順で 1 文字ずつ足して降りる。
const KEY_ALPHABET: &str = "-/0123456789abcdefghijklmnopqrstuvwxyz";

/// 周回の次の prefix。尽きたら `None`。
fn next_cycle_prefix(prefix: &str) -> Option<String> {
    if let Some(index) = CYCLE_ROOTS.iter().position(|root| *root == prefix) {
        return CYCLE_ROOTS.get(index + 1).map(|root| root.to_string());
    }
    let mut parent = prefix.to_string();
    let last = parent.pop()?;
    match KEY_ALPHABET
        .find(last)
        .and_then(|index| KEY_ALPHABET[index + 1..].chars().next())
    {
        Some(next) => {
            parent.push(next);
            Some(parent)
        }
        None => next_cycle_prefix(&parent),
    }
}

fn new_cursor(device_id: &str, head: u64) -> AccountSyncCursor {
    AccountSyncCursor {
        device_id: device_id.to_string(),
        seq: 0,
        head,
        cycle_prefix: Some(CYCLE_ROOTS[0].to_string()),
        cycle_head: head,
        updated_at: 0,
    }
}

/// account 同期の書込みの排他と、取得の結果（ADR 0061 §10）。account の runtime の handle が共有する。
#[derive(Default)]
pub(crate) struct AccountSyncState {
    /// item の書込みと変更の窓への追記を、端末の中で 1 つずつにする。
    write: tokio::sync::Mutex<()>,
    /// 書込みが成功したら、lease の task に送り直しと hint の送信をさせる（利用者の操作の経路で送信を待たない）。
    pub(super) changed: tokio::sync::Notify,
    /// rendezvous で本人の端末の候補が入ったら、lease の task に契機の取得をさせる（起動・復帰の時点では候補が
    /// まだ無い）。
    pub(super) peers: tokio::sync::Notify,
    fetch: std::sync::Mutex<FetchResults>,
}

#[derive(Default)]
struct FetchResults {
    no_candidates: bool,
    failed: BTreeSet<String>,
}

/// account 同期の状態（ADR 0061 §10）。どれかが true なら未同期。表示は W8。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct AccountSyncStatus {
    /// 本人の端末の候補が無い。
    pub no_peers: bool,
    /// 最後の取得が失敗した相手がある。
    pub fetch_failed: bool,
    /// 読み残しがある（cursor が最後に読んだ head より手前、または周回の途中）。
    pub behind: bool,
    /// replica へ書けていない行がある。
    pub pending_writes: bool,
    /// DB を失ったときの作り直しの途中。
    pub rebuilding: bool,
}

impl AccountSyncStatus {
    pub fn unsynced(&self) -> bool {
        self.no_peers || self.fetch_failed || self.behind || self.pending_writes || self.rebuilding
    }
}

impl AppService {
    /// 封をした item を自分の replica へ書き、変更の窓へ足して、書いたとする。書けなければ送信待ちのまま残す
    /// （採用は戻さない）。書けたら lease の task に、送り直しと hint の送信をさせる。
    pub(crate) async fn write_account_sync_item(&self, item: &AccountSyncItem) -> Result<()> {
        if let Err(error) = self.try_write_account_sync_item(item).await {
            warn!(key = ?item.key, %error, "account sync item stays unwritten until it is resent");
        }
        Ok(())
    }

    async fn try_write_account_sync_item(&self, item: &AccountSyncItem) -> Result<()> {
        let device_id = self.local_device_id().await?;
        {
            let _write = self.services.account_sync.write.lock().await;
            self.set_account_sync_entry(item).await?;
            let seq = self
                .read_account_sync_change(
                    self.services.docs_sync.as_ref(),
                    &AccountSyncItemKey::ChangeHead {
                        device_id: device_id.clone(),
                    },
                )
                .await?
                .map_or(0, |head| head.seq)
                + 1;
            let now = Utc::now().timestamp_millis();
            let change =
                |docs_key: String| serde_json::to_value(AccountSyncChangeV1 { seq, docs_key });
            // slot、head の順に書く。head を書く前に止まったら、次の書込みが同じ slot を上書きする。
            self.set_account_sync_entry(&AccountSyncItem::edit(
                AccountSyncItemKey::change_slot(&device_id, seq),
                now,
                Some(change(item.key.docs_key())?),
            ))
            .await?;
            self.set_account_sync_entry(&AccountSyncItem::edit(
                AccountSyncItemKey::ChangeHead {
                    device_id: device_id.clone(),
                },
                now,
                Some(change(String::new())?),
            ))
            .await?;
        }
        self.mark_account_sync_item_written(item).await?;
        self.services.account_sync.changed.notify_one();
        Ok(())
    }

    /// 自分の窓の head の seq を hint で送る。相手が居ないと gossip の送信は待ち続けうるので、期限で打ち切る（取りこぼした
    /// hint は、相手の次の契機の取得が cursor から読む）。
    pub(crate) async fn publish_account_sync_hint(&self) {
        let result = async {
            let device_id = self.local_device_id().await?;
            let Some(head) = self
                .read_account_sync_change(
                    self.services.docs_sync.as_ref(),
                    &AccountSyncItemKey::ChangeHead {
                        device_id: device_id.clone(),
                    },
                )
                .await?
            else {
                return anyhow::Ok(());
            };
            n0_future::time::timeout(
                std::time::Duration::from_secs(2),
                self.services.hint_transport.publish_hint(
                    self.services.keys.derive_account_sync().hint_topic(),
                    GossipHint::AccountSyncChanged {
                        device_id,
                        seq: head.seq,
                    },
                ),
            )
            .await??;
            anyhow::Ok(())
        }
        .await;
        if let Err(error) = result {
            warn!(%error, "failed to publish the account sync hint");
        }
    }

    async fn set_account_sync_entry(&self, item: &AccountSyncItem) -> Result<()> {
        let keys = self.services.keys.derive_account_sync();
        let sealed = keys.seal(&self.services.keys.public_key(), item)?;
        self.services
            .docs_sync
            .apply_doc_op(
                keys.replica_id(),
                DocOp::SetJson {
                    key: item.key.docs_key(),
                    value: serde_json::to_value(sealed)?,
                },
            )
            .await
    }

    async fn mark_account_sync_item_written(&self, item: &AccountSyncItem) -> Result<()> {
        let store = &self.services.projection_store;
        match &item.key {
            AccountSyncItemKey::ChannelCapability {
                channel_id,
                epoch_id,
            } => {
                store
                    .mark_private_channel_epoch_written(channel_id.as_str(), epoch_id)
                    .await
            }
            _ => store.mark_account_sync_written(&row_of(item)?).await,
        }
    }

    async fn read_account_sync_change(
        &self,
        source: &dyn DocsSync,
        key: &AccountSyncItemKey,
    ) -> Result<Option<AccountSyncChangeV1>> {
        self.read_account_sync_from(source, &key.docs_key())
            .await?
            .and_then(|item| item.value)
            .map(serde_json::from_value)
            .transpose()
            .map_err(Into::into)
    }

    /// `source`（手元の docs か、本人の別の端末の reader）から item を 1 件読む。reader の lease は読むたびに閉じる
    /// （1 lease は 32 key・1 MiB）。
    async fn read_account_sync_from(
        &self,
        source: &dyn DocsSync,
        docs_key: &str,
    ) -> Result<Option<AccountSyncItem>> {
        let item = self
            .read_account_sync_record(source, docs_key, DocFetchPolicy::LocalThenRemote)
            .await;
        source.finish_remote_object().await;
        item
    }

    /// 本人の別の端末から採った item を反映し、採った（行を置き換えた）ら自分の replica へ書く。手元の replica から
    /// 読んだ item（作り直し）は、既にあるので書いたとするだけ。
    async fn merge_fetched_account_sync_item(
        &self,
        item: AccountSyncItem,
        from_own_replica: bool,
    ) -> Result<()> {
        if self.merge_account_sync_item(item.clone()).await? {
            if from_own_replica {
                self.mark_account_sync_item_written(&item).await?;
            } else {
                self.write_account_sync_item(&item).await?;
            }
        }
        Ok(())
    }

    /// 周回の 1 照会。上限に届かなければ、その prefix の key をすべて merge して次の prefix へ、届けば子の prefix へ
    /// 降りる。次に照会する prefix を返す（尽きたら `None`）。
    async fn account_sync_cycle_step(
        &self,
        source: &dyn DocsSync,
        prefix: &str,
        from_own_replica: bool,
    ) -> Result<Option<String>> {
        let replica = self
            .services
            .keys
            .derive_account_sync()
            .replica_id()
            .clone();
        let query = DocKeyQuery {
            prefix: prefix.to_string(),
            order: DocKeyOrder::Ascending,
            limit: ACCOUNT_SYNC_PAGE,
        };
        let page = match self.services.docs_sync.local_docs_author().await? {
            Some(author) => {
                source
                    .query_replica_keys_by_author(&replica, &author, query)
                    .await?
            }
            None => source.query_replica_keys(&replica, query).await?,
        };
        source.finish_remote_object().await;
        let complete = !page.reached_limit;
        for entry in page
            .entries
            .iter()
            .filter(|entry| complete || entry.key == prefix)
        {
            if let Some(item) = self.read_account_sync_from(source, &entry.key).await? {
                self.merge_fetched_account_sync_item(item, from_own_replica)
                    .await?;
            }
        }
        Ok(if complete {
            next_cycle_prefix(prefix)
        } else {
            Some(format!("{prefix}{}", &KEY_ALPHABET[..1]))
        })
    }

    /// 相手 1 台との取得の 1 回。続きがあれば true。
    async fn account_sync_fetch_step(
        &self,
        source: &dyn DocsSync,
        device_id: &str,
        own_device_id: &str,
    ) -> Result<bool> {
        let store = &self.services.projection_store;
        let head = self
            .read_account_sync_change(
                source,
                &AccountSyncItemKey::ChangeHead {
                    device_id: device_id.to_string(),
                },
            )
            .await?
            .map_or(0, |head| head.seq);
        let mut cursor = store
            .get_account_sync_cursor(device_id)
            .await?
            .unwrap_or_else(|| new_cursor(device_id, head));
        cursor.head = head;
        cursor.updated_at = Utc::now().timestamp_millis();
        if let Some(prefix) = cursor.cycle_prefix.clone() {
            cursor.cycle_prefix = self.account_sync_cycle_step(source, &prefix, false).await?;
            if cursor.cycle_prefix.is_none() {
                cursor.seq = cursor.cycle_head;
            }
        } else if head.saturating_sub(cursor.seq) > ACCOUNT_SYNC_CHANGE_WINDOW {
            cursor = AccountSyncCursor {
                updated_at: cursor.updated_at,
                ..new_cursor(device_id, head)
            };
        } else {
            for seq in cursor.seq + 1..=head.min(cursor.seq + ACCOUNT_SYNC_PAGE as u64) {
                let change = self
                    .read_account_sync_change(
                        source,
                        &AccountSyncItemKey::change_slot(device_id, seq),
                    )
                    .await?;
                // 窓を上書きされた（seq が違う）ら、周回で追い付く。
                let Some(change) = change.filter(|change| change.seq == seq) else {
                    cursor = AccountSyncCursor {
                        updated_at: cursor.updated_at,
                        ..new_cursor(device_id, head)
                    };
                    break;
                };
                if let Some(item) = self
                    .read_account_sync_from(source, &change.docs_key)
                    .await?
                {
                    self.merge_fetched_account_sync_item(item, false).await?;
                }
                cursor.seq = seq;
                store
                    .put_account_sync_cursor(&cursor, own_device_id)
                    .await?;
            }
        }
        store
            .put_account_sync_cursor(&cursor, own_device_id)
            .await?;
        Ok(cursor.cycle_prefix.is_some() || cursor.seq < head)
    }

    /// 相手 1 台から、head（周回なら周回の終わり）に届くまで取得する。失敗したら再試行せず、cursor の位置から次の
    /// 契機で再開する。
    pub(crate) async fn fetch_account_sync_from(&self, device_id: &str) -> Result<()> {
        let own_device_id = self.local_device_id().await?;
        if device_id == own_device_id {
            return Ok(());
        }
        let result = async {
            let keys = self.services.keys.derive_account_sync();
            let mut secret = [0_u8; 32];
            hex::decode_to_slice(keys.expose_namespace_secret_hex(), &mut secret)?;
            let peer = kukuri_transport::SeedPeer {
                endpoint_id: device_id.to_string(),
                addr_hint: None,
            };
            let source = self
                .services
                .docs_sync
                .remote_readers(keys.replica_id(), Some(secret), vec![peer])
                .await?
                .into_iter()
                .next()
                .context("no reader for the device")?;
            while self
                .account_sync_fetch_step(source.as_ref(), device_id, &own_device_id)
                .await?
            {}
            anyhow::Ok(())
        }
        .await;
        let mut fetch = self
            .services
            .account_sync
            .fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if result.is_ok() {
            fetch.failed.remove(device_id);
            fetch.no_candidates = false;
        } else {
            fetch.failed.insert(device_id.to_string());
        }
        result
    }

    /// 送信待ちの行を、未書込みの索引から 64 件ずつ、索引が空になるまで送り直す。手元の replica の同じ key の版が
    /// 行の版と同じなら書いたとし、違えば行の版を書く。書けなければ止め、次の契機で再開する。
    pub(crate) async fn resend_account_sync_items(&self) -> Result<()> {
        let store = &self.services.projection_store;
        loop {
            let rows = store
                .list_unwritten_account_sync_rows(ACCOUNT_SYNC_PAGE)
                .await?;
            let epochs = store
                .list_unwritten_private_channel_epochs(ACCOUNT_SYNC_PAGE)
                .await?;
            if rows.is_empty() && epochs.is_empty() {
                return Ok(());
            }
            let mut items = rows
                .into_iter()
                .map(|row: AccountSyncRow| {
                    Ok(AccountSyncItem {
                        key: AccountSyncItemKey::from_docs_key(&row.key)?,
                        op_id: row.op_id,
                        updated_at: row.updated_at,
                        value: row.value.as_deref().map(serde_json::from_str).transpose()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            for epoch in epochs {
                let (channel_id, epoch_id) = (epoch.channel_id.clone(), epoch.epoch_id.clone());
                let updated_at = epoch.updated_at;
                let (_, secret) = self.services.open_epoch_row(epoch)?;
                items.push(AccountSyncItem::channel_epoch(
                    &ChannelId::new(channel_id),
                    &epoch_id,
                    updated_at,
                    serde_json::to_value(PrivateChannelEpochCapability {
                        epoch_id: epoch_id.clone(),
                        namespace_secret_hex: secret,
                    })?,
                )?);
            }
            for item in items {
                let replica = self
                    .read_account_sync_docs_key(&item.key.docs_key(), DocFetchPolicy::LocalOnly)
                    .await?;
                let same = match &item.key {
                    AccountSyncItemKey::ChannelCapability { .. } => replica.is_some(),
                    _ => replica.is_some_and(|replica| {
                        (replica.updated_at, &replica.op_id) == (item.updated_at, &item.op_id)
                    }),
                };
                if same {
                    self.mark_account_sync_item_written(&item).await?;
                } else {
                    self.try_write_account_sync_item(&item).await?;
                }
            }
        }
    }

    /// DB を失ったときの作り直し（ADR 0061 §10）。自分の cursor の行が無い・周回の途中なら、手元の replica を周回して
    /// merge する。終わったら行に終わりを記録する。
    pub(crate) async fn rebuild_account_sync(&self, own_device_id: &str) -> Result<()> {
        let store = &self.services.projection_store;
        let mut cursor = store
            .get_account_sync_cursor(own_device_id)
            .await?
            .unwrap_or_else(|| new_cursor(own_device_id, 0));
        while let Some(prefix) = cursor.cycle_prefix.clone() {
            cursor.cycle_prefix = self
                .account_sync_cycle_step(self.services.docs_sync.as_ref(), &prefix, true)
                .await?;
            cursor.updated_at = Utc::now().timestamp_millis();
            store
                .put_account_sync_cursor(&cursor, own_device_id)
                .await?;
        }
        Ok(())
    }

    /// 契機（lease の開始・endpoint の世代の変化・日の境界）の取得。作り直し、送り直し、本人の端末の候補それぞれから
    /// の取得を行い、最後に hint を 1 回送る（offline の間の自分の変更を、online の端末に読ませる）。
    pub(crate) async fn catch_up_account_sync(&self) {
        let own_device_id = match self.local_device_id().await {
            Ok(id) => id,
            Err(error) => {
                warn!(%error, "account sync cannot start without the device id");
                return;
            }
        };
        if let Err(error) = self.rebuild_account_sync(&own_device_id).await {
            warn!(%error, "account sync rebuild stopped; it resumes at the next trigger");
        }
        if let Err(error) = self.resend_account_sync_items().await {
            warn!(%error, "account sync resend stopped; it resumes at the next trigger");
        }
        let hint_topic = self
            .services
            .keys
            .derive_account_sync()
            .hint_topic()
            .clone();
        let peers = self
            .services
            .hint_transport
            .topic_read_candidates(&hint_topic)
            .await
            .unwrap_or_default();
        self.services
            .account_sync
            .fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .no_candidates = peers.is_empty();
        for peer in peers {
            if let Err(error) = self.fetch_account_sync_from(&peer.endpoint_id).await {
                warn!(%error, "account sync fetch stopped; it resumes at the next trigger");
            }
        }
        self.publish_account_sync_hint().await;
    }

    /// rendezvous の応答で、本人の端末の候補を account の hint topic へ入れたとき（desktop-runtime が呼ぶ）。lease の
    /// task が契機の取得を行う（ADR 0061 §10）。
    pub async fn account_sync_peers_joined(&self, topic: &str) {
        let hint_topic = self
            .services
            .keys
            .derive_account_sync()
            .hint_topic()
            .clone();
        if topic == kukuri_core::wire::hint_topic_id(&hint_topic).as_str() {
            self.services.account_sync.peers.notify_one();
        }
    }

    /// hint を受けたとき: 書いた端末から取得し、届かなければ中継した peer から読む。
    pub(crate) async fn fetch_account_sync_for_hint(&self, device_id: &str, source_peer: &str) {
        if self.fetch_account_sync_from(device_id).await.is_ok() || source_peer == device_id {
            return;
        }
        if let Err(error) = self.fetch_account_sync_from(source_peer).await {
            warn!(%error, "account sync fetch stopped; it resumes at the next trigger");
        }
    }

    /// account 同期の状態（ADR 0061 §10）。cursor の行（上限 + 1）と、未書込みの索引の 1 件だけを読む。
    pub async fn account_sync_status(&self) -> Result<AccountSyncStatus> {
        let store = &self.services.projection_store;
        let own_device_id = self.local_device_id().await?;
        let cursors = store.list_account_sync_cursors().await?;
        let (no_peers, fetch_failed) = {
            let fetch = self
                .services
                .account_sync
                .fetch
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (fetch.no_candidates, !fetch.failed.is_empty())
        };
        Ok(AccountSyncStatus {
            no_peers,
            fetch_failed,
            behind: cursors.iter().any(|cursor| {
                cursor.device_id != own_device_id
                    && (cursor.cycle_prefix.is_some() || cursor.seq < cursor.head)
            }),
            pending_writes: !store.list_unwritten_account_sync_rows(1).await?.is_empty()
                || !store
                    .list_unwritten_private_channel_epochs(1)
                    .await?
                    .is_empty(),
            rebuilding: cursors
                .iter()
                .find(|cursor| cursor.device_id == own_device_id)
                .is_none_or(|cursor| cursor.cycle_prefix.is_some()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cycle_walks_every_prefix_in_key_order() {
        assert_eq!(
            next_cycle_prefix("profile").as_deref(),
            Some("trust/always-visible/")
        );
        assert_eq!(next_cycle_prefix("channel/").as_deref(), None);
        assert_eq!(next_cycle_prefix("channel/a").as_deref(), Some("channel/b"));
        assert_eq!(
            next_cycle_prefix("channel/az").as_deref(),
            Some("channel/b")
        );
        assert_eq!(next_cycle_prefix("channel/z").as_deref(), None);
        assert_eq!(next_cycle_prefix("profile-").as_deref(), Some("profile/"));
    }
}
