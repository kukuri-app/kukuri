//! 別の端末が書いた Dome の接続の記録(提案・選択・合意)を provider から読んで手元へ反映する(#1221 R5-H)。
//!
//! 接続の記録は提案した Dome の anchor にあり、旧 sync の撤去後は別の端末へ届かない。topology の表示と
//! `dome-topology` の hint のときに、context で知っている Dome(一覧の行・heartbeat・自分)の anchor の接続の key を、
//! Dome の数と key の数の上限の内で provider から読む。署名を確かめた記録だけを、手元より新しいときに手元へ置く。

use super::dome_connection_support::{CONNECTION_PREFIX, PROPOSAL_PREFIX, SELECTION_PREFIX};
use super::remote_read_support::REMOTE_READ_DEADLINE;
use super::*;
use kukuri_core::SpatialContextV1;
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};

/// 1 回に接続の記録を provider から読む Dome の anchor の数。
const REMOTE_CONNECTION_ANCHORS: usize = 8;
/// 1 つの anchor の 1 種類(提案・選択・合意)について、provider から読む key の数。
const REMOTE_CONNECTION_KEYS: usize = 32;

impl AppService {
    /// `stores` のうち bucket の anchor(先頭から `REMOTE_CONNECTION_ANCHORS` 件)の接続の記録を provider から読み、
    /// 手元へ反映する。読めない provider・記録は飛ばす。全体の期限は `REMOTE_READ_DEADLINE`。
    pub(crate) async fn hydrate_dome_connection_records(
        &self,
        context: &SpatialContextV1,
        stores: &[ReplicaId],
    ) {
        let deadline = n0_future::time::Instant::now() + REMOTE_READ_DEADLINE;
        let anchors = stores
            .iter()
            .filter(|replica| replica.as_str().starts_with("bucket::"))
            .take(REMOTE_CONNECTION_ANCHORS);
        for anchor in anchors {
            let readers = match context {
                SpatialContextV1::Topic { topic_id } => self
                    .remote_post_readers(topic_id.as_str(), None, anchor, None)
                    .await
                    .unwrap_or_else(|error| {
                        warn!(%error, "Dome connection reader selection failed");
                        Vec::new()
                    }),
                SpatialContextV1::Channel {
                    topic_id,
                    channel_id,
                } => {
                    self.private_dome_readers(topic_id.as_str(), channel_id.as_str(), anchor)
                        .await
                }
            };
            for reader in readers {
                let read = crate::timeout_at(
                    deadline,
                    self.hydrate_connection_anchor(reader.as_ref(), anchor),
                )
                .await;
                reader.finish_remote_object().await;
                match read {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => warn!(%error, "Dome connection records read failed"),
                    Err(_) => return,
                }
            }
        }
    }

    async fn hydrate_connection_anchor(
        &self,
        reader: &dyn DocsSync,
        anchor: &ReplicaId,
    ) -> Result<()> {
        let stores = [anchor.clone()];
        for prefix in [PROPOSAL_PREFIX, SELECTION_PREFIX, CONNECTION_PREFIX] {
            let page = reader
                .query_replica_keys(
                    anchor,
                    DocKeyQuery {
                        prefix: stable_key(prefix, ""),
                        order: DocKeyOrder::Descending,
                        limit: REMOTE_CONNECTION_KEYS,
                    },
                )
                .await?;
            for entry in page.entries {
                if prefix != SELECTION_PREFIX && !entry.key.ends_with("/state") {
                    continue;
                }
                let Some(record) = read_remote(reader, anchor, &entry.key).await? else {
                    continue;
                };
                let local = self.local_connection_record(anchor, &entry.key).await?;
                let (envelopes, newer) = match prefix {
                    PROPOSAL_PREFIX => {
                        let state: DomeProposalStateDocV1 = serde_json::from_slice(&record)?;
                        let local = local
                            .map(|value| serde_json::from_slice::<DomeProposalStateDocV1>(&value))
                            .transpose()?;
                        let newer = local.is_none_or(|local| {
                            local.terminal_reason.is_none() && state.terminal_reason.is_some()
                        });
                        let mut ids = vec![
                            state.proposal_envelope_id.clone(),
                            state.proposer_agreement_envelope_id.clone(),
                        ];
                        ids.extend(state.terminal_event_envelope_id.clone());
                        (ids, newer)
                    }
                    SELECTION_PREFIX => {
                        let state: DomeSelectionStateDocV1 = serde_json::from_slice(&record)?;
                        (vec![state.envelope_id], local.is_none())
                    }
                    _ => {
                        let state: DomeConnectionStateDocV1 = serde_json::from_slice(&record)?;
                        let local = local
                            .map(|value| serde_json::from_slice::<DomeConnectionStateDocV1>(&value))
                            .transpose()?;
                        let newer = local.is_none_or(|local| {
                            state.record.lifecycle_generation > local.record.lifecycle_generation
                        });
                        let ids = vec![
                            state.proposer_agreement_envelope_id,
                            state.receiver_agreement_envelope_id,
                            state.lifecycle_envelope_id,
                        ];
                        (ids, newer)
                    }
                };
                if !newer {
                    continue;
                }
                for envelope_id in envelopes {
                    self.copy_connection_envelope(reader, anchor, &envelope_id)
                        .await?;
                }
                // 署名を確かめてから state を手元へ置く(手元の一覧が不正な state で失敗しないように)。
                let verified = match prefix {
                    PROPOSAL_PREFIX => {
                        self.verify_dome_proposal_state(&stores, &serde_json::from_slice(&record)?)
                            .await
                    }
                    SELECTION_PREFIX => {
                        let state: DomeSelectionStateDocV1 = serde_json::from_slice(&record)?;
                        self.fetch_signed_connection_content::<DomeProposalSelectionV1>(
                            &stores,
                            &state.envelope_id,
                            "dome-connection-selection",
                            &state.selection.receiver.owner_pubkey,
                        )
                        .await
                        .and_then(|signed| {
                            anyhow::ensure!(signed == state.selection, "selection mismatch");
                            Ok(())
                        })
                    }
                    _ => {
                        self.verify_dome_connection_state(
                            &stores,
                            &serde_json::from_slice(&record)?,
                        )
                        .await
                    }
                };
                if let Err(error) = verified {
                    warn!(%error, key = %entry.key, "ignored an invalid remote Dome connection record");
                    continue;
                }
                self.services
                    .docs_sync
                    .apply_doc_op(
                        anchor,
                        DocOp::SetJson {
                            key: entry.key,
                            value: serde_json::from_slice(&record)?,
                        },
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn local_connection_record(
        &self,
        anchor: &ReplicaId,
        key: &str,
    ) -> Result<Option<Vec<u8>>> {
        Ok(self
            .services
            .docs_sync
            .query_replica(anchor, DocQuery::Exact(key.to_string()))
            .await?
            .into_iter()
            .next()
            .map(|record| record.value))
    }

    /// 署名と id を確かめた envelope を、手元に無ければ置く。
    async fn copy_connection_envelope(
        &self,
        reader: &dyn DocsSync,
        anchor: &ReplicaId,
        envelope_id: &EnvelopeId,
    ) -> Result<()> {
        let key = stable_key("envelopes", envelope_id.as_str());
        if self.local_connection_record(anchor, &key).await?.is_some() {
            return Ok(());
        }
        let Some(value) = read_remote(reader, anchor, &key).await? else {
            return Ok(());
        };
        let envelope: KukuriEnvelope = serde_json::from_slice(&value)?;
        if envelope.verify().is_err() || envelope.id != *envelope_id {
            return Ok(());
        }
        self.persist_connection_envelope(anchor, &envelope).await
    }
}

async fn read_remote(
    reader: &dyn DocsSync,
    anchor: &ReplicaId,
    key: &str,
) -> Result<Option<Vec<u8>>> {
    Ok(reader
        .query_replica_with_policy(
            anchor,
            DocQuery::Exact(key.to_string()),
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        .into_iter()
        .next()
        .map(|record| record.value))
}
