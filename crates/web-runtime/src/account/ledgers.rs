//! 端末の台帳: 内容の観測・投稿の取り下げと docs への書込みの送信待ち・account 同期の採用済みの版・Community Node の
//! private index の許可と信頼評価の観測の提供（native の `sqlite/observations.rs`・`withdrawals.rs`・`account_sync.rs`・
//! `private_index_grants.rs`・`trust_observations.rs`）。どれも回収しない（観測は件数と期間の上限で消す）。

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{EnvelopeId, KukuriEnvelope, ReplicaId};
use kukuri_store::{
    ACCOUNT_SYNC_CURSOR_LIMIT, AccountSyncCursor, AccountSyncRow, AccountSyncStore,
    CONTENT_OBSERVATION_RETENTION_MS, ContentObservationRow, ContentObservationStore,
    MAX_CONTENT_OBSERVATIONS, PostWithdrawalRow, PostWithdrawalStore, PrivateIndexGrant,
    PrivateIndexGrantStore, TrustObservationNode, TrustObservationStore, WithdrawalWriteRow,
};
use wasm_bindgen::JsValue;
use web_sys::{IdbKeyRange, IdbTransaction};

use crate::IndexedDbCache;
use crate::content_cache::{
    ACCOUNT_SYNC, ACCOUNT_SYNC_CURSORS, INDEX_GRANTS, INDEX_STOPS, META, OBJECTS, OBSERVATIONS,
    PROFILES, TRUST_OBSERVATION_NODES, TRUST_OBSERVATION_PENDING, WITHDRAWAL_OUTBOX, WITHDRAWALS,
    prefix_upper_bound,
};
use crate::idb::{self, Mode, js_error};
use crate::rows::{self, Txn, key, num, prefix, text};

pub(crate) fn observation_key(row: &ContentObservationRow) -> JsValue {
    key(&[
        text(&row.subject_kind),
        text(&row.subject_id),
        text(&row.node_base_url),
        text(&row.capability),
    ])
}

/// `observed_at < before` の観測を、時刻の索引の範囲で消す。
async fn forget_observations_before(tx: &IdbTransaction, before: i64) -> Result<()> {
    let range = IdbKeyRange::upper_bound_with_open(&num(before), true).map_err(js_error)?;
    forget_observations(tx, &range, usize::MAX).await
}

/// 時刻の索引の `range` の古い順に `limit` 件の観測を消す。
async fn forget_observations(tx: &IdbTransaction, range: &IdbKeyRange, limit: usize) -> Result<()> {
    let stale: Vec<ContentObservationRow> =
        rows::scan(tx, OBSERVATIONS, Some("observed"), range, false, limit).await?;
    for row in stale {
        rows::delete(tx, OBSERVATIONS, &observation_key(&row))?;
    }
    Ok(())
}

/// `meta` の連番を 1 つ進めて返す。
async fn next_seq(tx: &IdbTransaction, name: &str) -> Result<i64> {
    let meta = rows::store(tx, META)?;
    let current = idb::done(&meta.get(&text(name)).map_err(js_error)?)
        .await?
        .as_f64()
        .unwrap_or(0.0) as i64;
    meta.put_with_key(&num(current + 1), &text(name))
        .map_err(js_error)?;
    Ok(current + 1)
}

#[async_trait]
impl ContentObservationStore for IndexedDbCache {
    /// 対象が端末にあるときだけ記録する。期間を過ぎた観測と、新しい順に上限を超えた観測を消す。
    async fn put_content_observation(&self, row: ContentObservationRow) -> Result<bool> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[OBSERVATIONS, OBJECTS, PROFILES], Mode::Write)?;
            let subject = match row.subject_kind.as_str() {
                "post" => Some(OBJECTS),
                "profile" => Some(PROFILES),
                _ => None,
            };
            let exists = match subject {
                Some(store) => {
                    rows::count(&tx, store, None, &rows::only(&text(&row.subject_id))?).await? > 0
                }
                None => false,
            };
            if !exists {
                return Ok(false);
            }
            let mut stored = row.clone();
            if let Some(existing) =
                rows::get::<ContentObservationRow>(&tx, OBSERVATIONS, &observation_key(&row))
                    .await?
            {
                stored.observed_at = stored.observed_at.max(existing.observed_at);
            }
            rows::put(&tx, OBSERVATIONS, &stored, &[])?;
            forget_observations_before(
                &tx,
                row.observed_at
                    .saturating_sub(CONTENT_OBSERVATION_RETENTION_MS),
            )
            .await?;
            let all = IdbKeyRange::lower_bound(&num(i64::MIN)).map_err(js_error)?;
            let count = rows::count(&tx, OBSERVATIONS, Some("observed"), &all).await?;
            if count > MAX_CONTENT_OBSERVATIONS {
                forget_observations(&tx, &all, count - MAX_CONTENT_OBSERVATIONS).await?;
            }
            tx.commit().await?;
            Ok(true)
        })
        .await
    }

    async fn list_content_observations_at(
        &self,
        subject_kind: &str,
        subject_id: &str,
        now_millis: i64,
    ) -> Result<Vec<ContentObservationRow>> {
        let (kind, id) = (subject_kind.to_owned(), subject_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[OBSERVATIONS], Mode::Write)?;
            forget_observations_before(
                &tx,
                now_millis.saturating_sub(CONTENT_OBSERVATION_RETENTION_MS),
            )
            .await?;
            let mut rows: Vec<ContentObservationRow> = rows::scan(
                &tx,
                OBSERVATIONS,
                None,
                &prefix(&[text(&kind), text(&id)])?,
                false,
                usize::MAX,
            )
            .await?;
            tx.commit().await?;
            rows.sort_by(|left, right| {
                right
                    .observed_at
                    .cmp(&left.observed_at)
                    .then_with(|| left.node_base_url.cmp(&right.node_base_url))
                    .then_with(|| left.capability.cmp(&right.capability))
            });
            Ok(rows)
        })
        .await
    }
}

#[async_trait]
impl PostWithdrawalStore for IndexedDbCache {
    /// 世代、取り下げの時刻、envelope の ID の順で新しい取り下げだけを採る。
    async fn put_post_withdrawal(&self, row: PostWithdrawalRow) -> Result<bool> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[WITHDRAWALS], Mode::Write)?;
            let id = text(row.target_object_id.as_str());
            let newer = match rows::get::<PostWithdrawalRow>(&tx, WITHDRAWALS, &id).await? {
                Some(current) => {
                    (
                        row.generation,
                        row.withdrawn_at,
                        &row.withdrawal_envelope_id,
                    ) > (
                        current.generation,
                        current.withdrawn_at,
                        &current.withdrawal_envelope_id,
                    )
                }
                None => true,
            };
            if newer {
                rows::put(&tx, WITHDRAWALS, &row, &[])?;
            }
            tx.commit().await?;
            Ok(newer)
        })
        .await
    }

    async fn get_post_withdrawal(
        &self,
        target_object_id: &EnvelopeId,
    ) -> Result<Option<PostWithdrawalRow>> {
        let id = target_object_id.as_str().to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[WITHDRAWALS], Mode::Read)?;
            rows::get(&tx, WITHDRAWALS, &text(&id)).await
        })
        .await
    }

    /// 同じ宛先の行は置き換え、積んだ順の最後へ回す（native の `INSERT OR REPLACE` の rowid と同じ）。
    async fn queue_withdrawal_writes(&self, rows: Vec<WithdrawalWriteRow>) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[WITHDRAWAL_OUTBOX, META], Mode::Write)?;
            for row in rows {
                let seq = next_seq(&tx, "withdrawal_seq").await?;
                rows::put(&tx, WITHDRAWAL_OUTBOX, &row, &[("seq", num(seq))])?;
            }
            tx.commit().await
        })
        .await
    }

    async fn pending_withdrawal_writes(&self, limit: usize) -> Result<Vec<WithdrawalWriteRow>> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[WITHDRAWAL_OUTBOX], Mode::Read)?;
            let all = IdbKeyRange::lower_bound(&num(i64::MIN)).map_err(js_error)?;
            rows::scan(&tx, WITHDRAWAL_OUTBOX, Some("seq"), &all, false, limit).await
        })
        .await
    }

    async fn finish_withdrawal_write(
        &self,
        withdrawal_envelope_id: &EnvelopeId,
        replica_id: &ReplicaId,
    ) -> Result<()> {
        let (envelope, replica) = (
            withdrawal_envelope_id.as_str().to_owned(),
            replica_id.as_str().to_owned(),
        );
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[WITHDRAWAL_OUTBOX], Mode::Write)?;
            rows::delete(
                &tx,
                WITHDRAWAL_OUTBOX,
                &key(&[text(&envelope), text(&replica)]),
            )?;
            tx.commit().await
        })
        .await
    }
}

#[async_trait]
impl AccountSyncStore for IndexedDbCache {
    async fn get_account_sync_row(&self, key: &str) -> Result<Option<AccountSyncRow>> {
        let key = key.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC], Mode::Read)?;
            rows::get(&tx, ACCOUNT_SYNC, &text(&key)).await
        })
        .await
    }

    /// `(updated_at, op_id)` が今の行より大きいときだけ置き換える。
    async fn adopt_account_sync_row(&self, row: &AccountSyncRow) -> Result<bool> {
        let row = row.clone();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC], Mode::Strict)?;
            let newer = rows::get::<AccountSyncRow>(&tx, ACCOUNT_SYNC, &text(&row.key))
                .await?
                .is_none_or(|current| {
                    (row.updated_at, &row.op_id) > (current.updated_at, &current.op_id)
                });
            if newer {
                rows::put(&tx, ACCOUNT_SYNC, &row, &[("unwritten", text(&row.key))])?;
            }
            tx.commit().await?;
            Ok(newer)
        })
        .await
    }

    /// 行の版が `row` と同じなら、未書込みの索引から外す（行を置き直す）。
    async fn mark_account_sync_written(&self, row: &AccountSyncRow) -> Result<()> {
        let row = row.clone();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC], Mode::Strict)?;
            if rows::get::<AccountSyncRow>(&tx, ACCOUNT_SYNC, &text(&row.key))
                .await?
                .is_some_and(|current| {
                    (current.updated_at, &current.op_id) == (row.updated_at, &row.op_id)
                })
            {
                rows::put(&tx, ACCOUNT_SYNC, &row, &[])?;
            }
            tx.commit().await
        })
        .await
    }

    async fn list_unwritten_account_sync_rows(&self, limit: usize) -> Result<Vec<AccountSyncRow>> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC], Mode::Read)?;
            let range = IdbKeyRange::lower_bound(&text("")).map_err(js_error)?;
            rows::scan(&tx, ACCOUNT_SYNC, Some("unwritten"), &range, false, limit).await
        })
        .await
    }

    async fn get_account_sync_cursor(&self, device_id: &str) -> Result<Option<AccountSyncCursor>> {
        let device_id = device_id.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC_CURSORS], Mode::Read)?;
            rows::get(&tx, ACCOUNT_SYNC_CURSORS, &text(&device_id)).await
        })
        .await
    }

    /// cursor を置き、自分以外の行が上限を超えたら、最も古く更新した行を消す（行は上限 + 1 件まで）。
    async fn put_account_sync_cursor(
        &self,
        cursor: &AccountSyncCursor,
        own_device_id: &str,
    ) -> Result<()> {
        let (cursor, own) = (cursor.clone(), own_device_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC_CURSORS], Mode::Strict)?;
            rows::put(&tx, ACCOUNT_SYNC_CURSORS, &cursor, &[])?;
            let range = IdbKeyRange::lower_bound(&num(i64::MIN)).map_err(js_error)?;
            let cursors: Vec<AccountSyncCursor> = rows::scan(
                &tx,
                ACCOUNT_SYNC_CURSORS,
                Some("updated"),
                &range,
                true,
                ACCOUNT_SYNC_CURSOR_LIMIT + 2,
            )
            .await?;
            for stale in cursors
                .iter()
                .filter(|cursor| cursor.device_id != own)
                .skip(ACCOUNT_SYNC_CURSOR_LIMIT)
            {
                rows::delete(&tx, ACCOUNT_SYNC_CURSORS, &text(&stale.device_id))?;
            }
            tx.commit().await
        })
        .await
    }

    async fn list_account_sync_cursors(&self) -> Result<Vec<AccountSyncCursor>> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC_CURSORS], Mode::Read)?;
            let range = IdbKeyRange::lower_bound(&text("")).map_err(js_error)?;
            rows::scan(
                &tx,
                ACCOUNT_SYNC_CURSORS,
                None,
                &range,
                false,
                ACCOUNT_SYNC_CURSOR_LIMIT + 1,
            )
            .await
        })
        .await
    }

    /// `prefix` で始まる key の範囲を key の順に読み、値のある行の key を返す。
    async fn list_account_sync_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let prefix = prefix.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[ACCOUNT_SYNC], Mode::Read)?;
            let range = IdbKeyRange::bound_with_lower_open_and_upper_open(
                &text(&prefix),
                &prefix_upper_bound(&prefix),
                false,
                true,
            )
            .map_err(js_error)?;
            let rows: Vec<AccountSyncRow> =
                rows::scan(&tx, ACCOUNT_SYNC, None, &range, false, usize::MAX).await?;
            Ok(rows
                .into_iter()
                .filter(|row| row.value.is_some())
                .map(|row| row.key)
                .collect())
        })
        .await
    }
}

/// 許可の行（node・channel の停止の版を、許可を置いた時点の値として持つ）。
#[derive(serde::Serialize, serde::Deserialize)]
struct Grant {
    #[serde(flatten)]
    grant: PrivateIndexGrant,
    node_revision: i64,
    channel_revision: i64,
    last_checked_at_ms: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Stop {
    kind: String,
    id: String,
    revision: i64,
}

fn grant_key(base_url: &str, topic_id: &str, channel_id: &str) -> JsValue {
    key(&[text(base_url), text(topic_id), text(channel_id)])
}

fn channel_stop_id(topic_id: &str, channel_id: &str) -> Result<String> {
    Ok(serde_json::to_string(&[topic_id, channel_id])?)
}

async fn stop_revision(tx: &IdbTransaction, kind: &str, id: &str) -> Result<i64> {
    Ok(
        rows::get::<Stop>(tx, INDEX_STOPS, &key(&[text(kind), text(id)]))
            .await?
            .map_or(0, |stop| stop.revision),
    )
}

impl IndexedDbCache {
    async fn bump_stop(&self, kind: &'static str, id: String) -> Result<()> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[INDEX_STOPS], Mode::Write)?;
            let revision = stop_revision(&tx, kind, &id).await? + 1;
            rows::put(
                &tx,
                INDEX_STOPS,
                &Stop {
                    kind: kind.into(),
                    id,
                    revision,
                },
                &[],
            )?;
            tx.commit().await
        })
        .await
    }
}

#[async_trait]
impl PrivateIndexGrantStore for IndexedDbCache {
    async fn save_private_index_grant(&self, grant: &PrivateIndexGrant) -> Result<()> {
        let grant = grant.clone();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[INDEX_GRANTS, INDEX_STOPS], Mode::Write)?;
            let id = grant_key(&grant.base_url, &grant.topic_id, &grant.channel_id);
            let last_checked_at_ms = rows::get::<Grant>(&tx, INDEX_GRANTS, &id)
                .await?
                .map_or(0, |existing| existing.last_checked_at_ms);
            let row = Grant {
                node_revision: stop_revision(&tx, "node", &grant.base_url).await?,
                channel_revision: stop_revision(
                    &tx,
                    "channel",
                    &channel_stop_id(&grant.topic_id, &grant.channel_id)?,
                )
                .await?,
                last_checked_at_ms,
                grant,
            };
            rows::put(&tx, INDEX_GRANTS, &row, &[])?;
            tx.commit().await
        })
        .await
    }

    /// 確かめた時刻の古い順に 1 行。停止の後に残った許可は消して次へ進む（native と同じく 8 回まで）。
    async fn next_private_index_grant(
        &self,
        base_url: &str,
        now_ms: i64,
    ) -> Result<Option<PrivateIndexGrant>> {
        let base = base_url.to_owned();
        self.run(move |db| async move {
            for _ in 0..8 {
                let tx = Txn::begin(&db.idb, &[INDEX_GRANTS, INDEX_STOPS], Mode::Write)?;
                let Some(mut row) = rows::scan::<Grant>(
                    &tx,
                    INDEX_GRANTS,
                    Some("due"),
                    &prefix(&[text(&base)])?,
                    false,
                    1,
                )
                .await?
                .pop() else {
                    return Ok(None);
                };
                let grant = &row.grant;
                let id = grant_key(&grant.base_url, &grant.topic_id, &grant.channel_id);
                let valid = row.node_revision
                    == stop_revision(&tx, "node", &grant.base_url).await?
                    && row.channel_revision
                        == stop_revision(
                            &tx,
                            "channel",
                            &channel_stop_id(&grant.topic_id, &grant.channel_id)?,
                        )
                        .await?;
                if !valid {
                    rows::delete(&tx, INDEX_GRANTS, &id)?;
                    tx.commit().await?;
                    continue;
                }
                row.last_checked_at_ms = now_ms;
                rows::put(&tx, INDEX_GRANTS, &row, &[])?;
                tx.commit().await?;
                return Ok(Some(row.grant));
            }
            Ok(None)
        })
        .await
    }

    async fn mark_private_index_grant_applied(
        &self,
        grant: &PrivateIndexGrant,
        epoch_id: &str,
    ) -> Result<()> {
        let (grant, epoch) = (grant.clone(), epoch_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[INDEX_GRANTS], Mode::Write)?;
            let id = grant_key(&grant.base_url, &grant.topic_id, &grant.channel_id);
            if let Some(mut row) = rows::get::<Grant>(&tx, INDEX_GRANTS, &id).await?
                && row.grant.applied_epoch_id == grant.applied_epoch_id
            {
                row.grant.applied_epoch_id = epoch;
                rows::put(&tx, INDEX_GRANTS, &row, &[])?;
            }
            tx.commit().await
        })
        .await
    }

    async fn stop_private_index_grant(
        &self,
        base_url: &str,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        let id = [base_url, topic_id, channel_id].map(str::to_owned);
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[INDEX_GRANTS], Mode::Write)?;
            rows::delete(&tx, INDEX_GRANTS, &grant_key(&id[0], &id[1], &id[2]))?;
            tx.commit().await
        })
        .await
    }

    /// node の停止の版を進める。許可の行は `next_private_index_grant` が 1 行ずつ消す（全件を書き換えない）。
    async fn stop_private_index_grants_for_node(&self, base_url: &str) -> Result<()> {
        self.bump_stop("node", base_url.to_owned()).await
    }

    async fn stop_private_index_grants_for_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        self.bump_stop("channel", channel_stop_id(topic_id, channel_id)?)
            .await
    }
}

/// CN ごとの観測の提供の状態（native の `cn_trust_observation_nodes`）。提供中（有効で、削除要求が未完了でない）の
/// 行だけが `sharing` の索引に載る。
#[derive(serde::Serialize, serde::Deserialize)]
struct ObservationNode {
    base_url: String,
    #[serde(flatten)]
    node: TrustObservationNode,
}

/// 送信待ちの観測（native の `cn_trust_observation_pending`。key は `[base_url, observation_key]`）。
#[derive(serde::Serialize, serde::Deserialize)]
struct QueuedObservation {
    base_url: String,
    observation_key: String,
    envelope: KukuriEnvelope,
}

/// 提供中の node を `limit` 件まで。
async fn sharing_nodes(tx: &IdbTransaction, limit: usize) -> Result<Vec<ObservationNode>> {
    let all = IdbKeyRange::lower_bound(&num(i64::MIN)).map_err(js_error)?;
    rows::scan(
        tx,
        TRUST_OBSERVATION_NODES,
        Some("sharing"),
        &all,
        false,
        limit,
    )
    .await
}

#[async_trait]
impl TrustObservationStore for IndexedDbCache {
    async fn trust_observation_node(&self, base_url: &str) -> Result<Option<TrustObservationNode>> {
        let base = base_url.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[TRUST_OBSERVATION_NODES], Mode::Read)?;
            Ok(
                rows::get::<ObservationNode>(&tx, TRUST_OBSERVATION_NODES, &text(&base))
                    .await?
                    .map(|row| row.node),
            )
        })
        .await
    }

    async fn save_trust_observation_node(
        &self,
        base_url: &str,
        node: Option<TrustObservationNode>,
    ) -> Result<()> {
        let base_url = base_url.to_owned();
        self.run(move |db| async move {
            let stores = [TRUST_OBSERVATION_NODES, TRUST_OBSERVATION_PENDING];
            let tx = Txn::begin(&db.idb, &stores, Mode::Write)?;
            let id = text(&base_url);
            match node {
                Some(node) => {
                    let sharing = if node.enabled && !node.revocation_pending {
                        num(1)
                    } else {
                        JsValue::UNDEFINED
                    };
                    let row = ObservationNode { base_url, node };
                    rows::put(&tx, TRUST_OBSERVATION_NODES, &row, &[("sharing", sharing)])?;
                }
                None => rows::delete(&tx, TRUST_OBSERVATION_NODES, &id)?,
            }
            if !node.is_some_and(|node| node.enabled) {
                let pending = prefix(&[id])?;
                rows::delete(&tx, TRUST_OBSERVATION_PENDING, &pending)?;
            }
            tx.commit().await
        })
        .await
    }

    async fn trust_observation_sharing(&self) -> Result<bool> {
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[TRUST_OBSERVATION_NODES], Mode::Read)?;
            Ok(!sharing_nodes(&tx, 1).await?.is_empty())
        })
        .await
    }

    /// `[対象|種別, created_at]` の索引の末尾の 1 件。
    async fn latest_queued_trust_observation_at(&self, key: &str) -> Result<Option<i64>> {
        let key = key.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[TRUST_OBSERVATION_PENDING], Mode::Read)?;
            let range = prefix(&[text(&key)])?;
            let latest: Vec<QueuedObservation> =
                rows::scan(&tx, TRUST_OBSERVATION_PENDING, Some("key"), &range, true, 1).await?;
            Ok(latest.first().map(|row| row.envelope.created_at))
        })
        .await
    }

    async fn queue_trust_observation(
        &self,
        base_url: Option<&str>,
        key: &str,
        envelope: &KukuriEnvelope,
    ) -> Result<()> {
        let (base_url, key, envelope) = (
            base_url.map(str::to_owned),
            key.to_owned(),
            envelope.clone(),
        );
        self.run(move |db| async move {
            let stores = [TRUST_OBSERVATION_NODES, TRUST_OBSERVATION_PENDING];
            let tx = Txn::begin(&db.idb, &stores, Mode::Write)?;
            let nodes = match &base_url {
                Some(base_url) => rows::get(&tx, TRUST_OBSERVATION_NODES, &text(base_url))
                    .await?
                    .into_iter()
                    .collect(),
                None => sharing_nodes(&tx, usize::MAX).await?,
            };
            for ObservationNode { base_url, node } in nodes {
                if node.enabled && !node.revocation_pending {
                    let row = QueuedObservation {
                        base_url,
                        observation_key: key.clone(),
                        envelope: envelope.clone(),
                    };
                    rows::put(&tx, TRUST_OBSERVATION_PENDING, &row, &[])?;
                }
            }
            tx.commit().await
        })
        .await
    }

    async fn queued_trust_observations(
        &self,
        base_url: &str,
        limit: usize,
    ) -> Result<Vec<(String, KukuriEnvelope)>> {
        let base_url = base_url.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[TRUST_OBSERVATION_PENDING], Mode::Read)?;
            let range = prefix(&[text(&base_url)])?;
            let queued: Vec<QueuedObservation> =
                rows::scan(&tx, TRUST_OBSERVATION_PENDING, None, &range, false, limit).await?;
            Ok(queued
                .into_iter()
                .map(|row| (row.observation_key, row.envelope))
                .collect())
        })
        .await
    }

    async fn dequeue_trust_observation(
        &self,
        base_url: &str,
        key: &str,
        envelope_id: &str,
    ) -> Result<()> {
        let [base_url, key, envelope_id] = [base_url, key, envelope_id].map(str::to_owned);
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[TRUST_OBSERVATION_PENDING], Mode::Write)?;
            let id = rows::key(&[text(&base_url), text(&key)]);
            if rows::get::<QueuedObservation>(&tx, TRUST_OBSERVATION_PENDING, &id)
                .await?
                .is_some_and(|row| row.envelope.id.0 == envelope_id)
            {
                rows::delete(&tx, TRUST_OBSERVATION_PENDING, &id)?;
            }
            tx.commit().await
        })
        .await
    }

    async fn count_queued_trust_observations(&self, base_url: &str) -> Result<i64> {
        let base_url = base_url.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[TRUST_OBSERVATION_PENDING], Mode::Read)?;
            let range = prefix(&[text(&base_url)])?;
            let count = rows::count(&tx, TRUST_OBSERVATION_PENDING, None, &range).await?;
            Ok(i64::try_from(count).unwrap_or(i64::MAX))
        })
        .await
    }
}
