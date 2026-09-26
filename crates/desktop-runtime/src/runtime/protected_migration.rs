//! 旧 `iroh-data` にしか無い本人のデータを、保護所有先(remote cache と保護参照)へ移す(#1221 R5-G)。
//!
//! kind ごとに index の保存した位置の後ろを 1 回 128 件まで読み、旧領域の blob と依存 record を写して照合し
//! (blob は BLAKE3、record は content hash)、保護参照を付けてから位置を進める。旧領域に無いものは写さずに進む。
//! 途中で止まれば、保存した位置から同じ結果でやり直す。旧領域から何も読めなかった参照(復元後・旧領域の回収後)は
//! 置き換えず、既にある保護を減らさない。

use std::time::Duration;

use kukuri_core::{
    DirectMessageFrameV1, KukuriEnvelope, PayloadRef, ReplicaId, open_sent_direct_message_frame,
    parse_custom_reaction_asset,
};
use kukuri_docs_sync::{DocRecord, IrohDocsSync, author_replica_id, stable_key, topic_replica_id};
use kukuri_iroh_node::{DocReadRecord, IrohDocsNode};
use kukuri_store::{
    ObjectProjectionStore, PROTECTED_MIGRATION_KINDS, PROTECTED_MIGRATION_PAGE, ProtectedCandidate,
    ProtectedSource,
};

use super::*;

/// これ以下の blob は SQLite の行に、超えるものは `kukuri.remote-blobs/` の file に置く。
const INLINE_BLOB_BYTES: u64 = 1024 * 1024;
const DOME_PIN_TAG_PREFIX: &str = "kukuri/metaverse/pin/";
/// 本人の envelope 行は docs の record より先に入る。作成からこの秒数までは、依存 record が揃うまで位置を止める。
const OWN_RECORD_SETTLE_SECS: i64 = 600;

/// 1 件の候補の写し方。依存 record がまだ書かれていなければ、その行の手前で止まって次のステップで読み直す。
enum CandidatePlan {
    Ready(Option<ProtectionPlan>),
    NotYet,
}

/// 1 件の保護参照と、そこから守る blob と旧領域の record。
struct ProtectionPlan {
    reference: String,
    blobs: Vec<String>,
    records: Vec<(ReplicaId, DocRecord)>,
}

/// 旧領域の読み手。1 ステップの間だけ現在の stack から借りる。
struct LegacySource {
    node: Arc<IrohDocsNode>,
    docs: Arc<IrohDocsSync>,
    blobs: Arc<kukuri_blob_service::IrohBlobService>,
}

impl LegacySource {
    /// 旧領域の 1 key の record。開けない replica(退出した private など)は無いものとして扱う。
    /// 作り直しで止まった docs actor は応答しないことがあるため、時間切れはこのステップの失敗にする(次で読み直す)。
    async fn records(&self, replica: &ReplicaId, key: &str) -> Result<Vec<(ReplicaId, DocRecord)>> {
        let read = self.docs.read_legacy_records(replica, key);
        Ok(tokio::time::timeout(Duration::from_secs(10), read)
            .await
            .context("legacy docs read timed out")?
            .unwrap_or_default()
            .into_iter()
            .map(|record| (replica.clone(), record))
            .collect())
    }
}

fn json_field(record: &DocRecord, field: &str) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(&record.value)
        .ok()?
        .get(field)?
        .as_str()
        .map(str::to_string)
}

impl DesktopRuntime {
    /// 各 kind の保存した位置の後ろを 1 ページ(128 件まで)ずつ写す。全 kind が終端へ達していれば `true`。
    pub(crate) async fn protected_migration_step(&self) -> Result<bool> {
        let _guard = self.protected_migration_guard.lock().await;
        let source = {
            let current = self.iroh_stack.current.lock().await;
            let stack = current.as_ref().context("missing active iroh stack")?;
            LegacySource {
                node: stack.node.clone(),
                docs: stack.docs_sync.clone(),
                blobs: stack.blob_service.clone(),
            }
        };
        let local = self.author_keys.public_key_hex();
        let mut caught_up = true;
        for kind in PROTECTED_MIGRATION_KINDS {
            if kind == "private"
                && self
                    .private_migration_dirty
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                && let Err(error) = self.store.reset_protected_migration(kind).await
            {
                self.private_migration_dirty
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return Err(error);
            }
            // pin の tag は hash の順に並ぶため、追いついた後に pin した asset を拾えるよう、起動時と pin したときに
            // 先頭から読み直す。
            if kind == "dome_pin" {
                let generation = source.blobs.pin_generation();
                if self
                    .dome_pin_generation
                    .swap(generation, std::sync::atomic::Ordering::SeqCst)
                    != generation
                {
                    self.store.reset_protected_migration(kind).await?;
                }
            }
            let cursor = self.store.protected_migration_cursor(kind).await?;
            let (plans, cursor, done) = match kind {
                "private" => {
                    let channels = self
                        .app_service
                        .joined_private_channel_replicas()
                        .await
                        .into_iter()
                        .filter(|(key, _)| key.as_str() > cursor.as_str())
                        .take(PROTECTED_MIGRATION_PAGE)
                        .collect::<Vec<_>>();
                    let mut plans = Vec::new();
                    for (key, replica) in &channels {
                        plans.extend(self.private_plan(&source, key, replica, &local).await?);
                    }
                    let done = channels.len() < PROTECTED_MIGRATION_PAGE;
                    let next = channels.last().map_or(cursor, |(key, _)| key.clone());
                    (plans, next, done)
                }
                "dome_pin" => {
                    let tags = source
                        .node
                        .list_local_tags(DOME_PIN_TAG_PREFIX, &cursor, PROTECTED_MIGRATION_PAGE)
                        .await?;
                    let plans = tags
                        .iter()
                        .filter_map(|tag| tag.strip_prefix(DOME_PIN_TAG_PREFIX))
                        .map(|hash| ProtectionPlan {
                            reference: format!("dome_pin:{hash}"),
                            blobs: vec![hash.to_string()],
                            records: Vec::new(),
                        })
                        .collect();
                    let done = tags.len() < PROTECTED_MIGRATION_PAGE;
                    (plans, tags.last().cloned().unwrap_or(cursor), done)
                }
                _ => {
                    let page = self.store.protected_migration_page(kind, &local).await?;
                    let (mut next, mut done) = (page.cursor, page.done);
                    let mut plans = Vec::new();
                    for candidate in page.candidates {
                        let rowid = candidate.rowid;
                        match self.candidate_plan(&source, candidate, &local).await? {
                            CandidatePlan::Ready(plan) => plans.extend(plan),
                            CandidatePlan::NotYet => {
                                next =
                                    rowid.map_or(cursor.clone(), |rowid| (rowid - 1).to_string());
                                done = false;
                                break;
                            }
                        }
                    }
                    (plans, next, done)
                }
            };
            for plan in plans {
                self.protect(&source, plan).await?;
            }
            self.store
                .finish_protected_migration_page(kind, &cursor, done)
                .await?;
            caught_up &= done;
        }
        Ok(caught_up)
    }

    /// backup の前に、残りを 128 件ずつ終端まで写す。
    pub async fn finish_protected_migration(&self) -> Result<()> {
        // 依存 record を待つ行があれば同じページを読み直すため、空回りしないよう間を置く。
        while !self.protected_migration_step().await? {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// 起動後の背景 task。満杯のページが続く間は 100ms、追いついたら 60 秒後に次を読む。shutdown で止める。
    pub async fn start_protected_migration(self: &Arc<Self>) {
        let mut task = self.protected_migration_task.lock().await;
        if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        let weak = Arc::downgrade(self);
        *task = Some(tokio::spawn(async move {
            loop {
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                let delay = match runtime.protected_migration_step().await {
                    Ok(false) => Duration::from_millis(100),
                    Ok(true) => Duration::from_secs(60),
                    Err(error) => {
                        tracing::warn!(%error, "protected data migration step failed");
                        Duration::from_secs(60)
                    }
                };
                drop(runtime);
                tokio::time::sleep(delay).await;
            }
        }));
    }

    async fn candidate_plan(
        &self,
        source: &LegacySource,
        candidate: ProtectedCandidate,
        local: &str,
    ) -> Result<CandidatePlan> {
        let mut plan = ProtectionPlan {
            reference: candidate.reference,
            blobs: candidate.blobs,
            records: Vec::new(),
        };
        match candidate.source {
            ProtectedSource::Blobs => {}
            ProtectedSource::Envelope(envelope) => {
                if !self
                    .own_envelope_plan(source, &envelope, local, &mut plan)
                    .await?
                {
                    return Ok(CandidatePlan::NotYet);
                }
            }
            ProtectedSource::DirectMessageFrame => {
                // 暗号化添付の hash は frame の中にしか無いため、送信者として frame を開く。
                let frame_hash = plan.blobs.first().cloned().unwrap_or_default();
                let bytes = match source.node.read_local_blob(&frame_hash).await {
                    Ok(Some(bytes)) => Some(bytes),
                    _ => self.store.get_remote_content("blob", &frame_hash).await?,
                };
                let payload = bytes
                    .and_then(|bytes| serde_json::from_slice::<DirectMessageFrameV1>(&bytes).ok())
                    .and_then(|frame| {
                        open_sent_direct_message_frame(self.author_keys.as_ref(), &frame).ok()
                    });
                if let Some(manifest) = payload.and_then(|payload| payload.attachment_manifest) {
                    plan.blobs.extend(
                        std::iter::once(&manifest.original)
                            .chain(manifest.poster.as_ref())
                            .map(|blob| blob.hash.as_str().to_string()),
                    );
                }
            }
            ProtectedSource::Session { replica, state_key } => {
                // metaverse は訪問者も manifest を書くため、state が指す envelope の署名者で本人を判定する。
                let states = source.records(&replica, &state_key).await?;
                if states.is_empty() {
                    return Ok(CandidatePlan::Ready(None));
                }
                let mut signed = Vec::new();
                for (_, state) in &states {
                    let Some(envelope_id) = json_field(state, "last_envelope_id") else {
                        continue;
                    };
                    for (replica, record) in source
                        .records(&replica, &stable_key("envelopes", &envelope_id))
                        .await?
                    {
                        if serde_json::from_slice::<KukuriEnvelope>(&record.value).is_ok_and(
                            |envelope| {
                                envelope.verify().is_ok() && envelope.pubkey.as_str() == local
                            },
                        ) {
                            signed.push((replica, record));
                        }
                    }
                }
                if signed.is_empty() {
                    plan.blobs.clear();
                } else {
                    plan.records = states;
                    plan.records.extend(signed);
                }
            }
        }
        Ok(CandidatePlan::Ready(Some(plan)))
    }

    /// 本人の投稿(本文・添付・state・envelope・media manifest・プロフィールの行)と custom reaction asset。
    /// 依存 record がまだ書かれていない新しい行なら `false`(位置を止めて読み直す)。作成から
    /// [`OWN_RECORD_SETTLE_SECS`] を過ぎた行は、読めた分だけ写して進む。
    async fn own_envelope_plan(
        &self,
        source: &LegacySource,
        envelope: &KukuriEnvelope,
        local: &str,
        plan: &mut ProtectionPlan,
    ) -> Result<bool> {
        if envelope.verify().is_err() {
            return Ok(true);
        }
        let settled =
            envelope.created_at <= chrono::Utc::now().timestamp() - OWN_RECORD_SETTLE_SECS;
        let author = author_replica_id(local);
        if let Ok(Some(post)) = envelope.to_post_object() {
            if let PayloadRef::BlobText { hash, .. } = &post.payload_ref {
                plan.blobs.push(hash.as_str().to_string());
            }
            plan.blobs.extend(
                post.attachments
                    .iter()
                    .chain(post.repost_of.iter().flat_map(|repost| &repost.attachments))
                    .map(|asset| asset.hash.as_str().to_string()),
            );
            let replica = match self.store.get_object_projection(&post.object_id).await? {
                Some(row) => row.source_replica_id,
                None if post.channel_id.is_none() => topic_replica_id(post.topic_id.as_str()),
                None => return Ok(settled),
            };
            let mut keys = vec![
                stable_key("objects", &format!("{}/state", post.object_id.as_str())),
                stable_key("objects", &format!("{}/envelope", post.object_id.as_str())),
            ];
            for manifest_id in &post.media_manifest_refs {
                keys.push(stable_key(
                    "manifests/media",
                    &format!("{manifest_id}/state"),
                ));
                keys.push(stable_key(
                    "manifests/media",
                    &format!("{manifest_id}/envelope"),
                ));
            }
            for key in keys {
                plan.records.extend(source.records(&replica, &key).await?);
            }
            let has_state = !plan.records.is_empty();
            if post.channel_id.is_none() {
                let prefix = if post.object_kind == "repost" {
                    "profile/reposts"
                } else {
                    "profile/posts"
                };
                let profile = source
                    .records(&author, &stable_key(prefix, post.object_id.as_str()))
                    .await?;
                for (_, record) in &profile {
                    if let Some(envelope_id) = json_field(record, "envelope_id") {
                        plan.records.extend(
                            source
                                .records(&author, &stable_key("envelopes", &envelope_id))
                                .await?,
                        );
                    }
                }
                let has_profile = !profile.is_empty();
                plan.records.extend(profile);
                return Ok(settled || (has_state && has_profile));
            }
            return Ok(settled || has_state);
        } else if let Ok(Some(asset)) = parse_custom_reaction_asset(envelope) {
            plan.blobs.push(asset.blob_hash.as_str().to_string());
            for key in [
                stable_key("reactions/assets", &format!("{}/state", asset.asset_id)),
                stable_key("reactions/assets", &format!("{}/envelope", asset.asset_id)),
                stable_key("envelopes", envelope.id.as_str()),
            ] {
                plan.records.extend(source.records(&author, &key).await?);
            }
            return Ok(settled || !plan.records.is_empty());
        }
        Ok(true)
    }

    /// 参加中の channel の現在の epoch の metadata・policy・自分の参加 record・自分宛の grant。
    async fn private_plan(
        &self,
        source: &LegacySource,
        key: &str,
        replica: &ReplicaId,
        local: &str,
    ) -> Result<Option<ProtectionPlan>> {
        let mut records = Vec::new();
        for record_key in [
            stable_key("channels", "metadata"),
            stable_key("channels", "policy/envelope"),
            stable_key("channels/participants", &format!("{local}/envelope")),
            stable_key("channels/rotation-grants", &format!("{local}/envelope")),
        ] {
            records.extend(source.records(replica, &record_key).await?);
        }
        Ok((!records.is_empty()).then(|| ProtectionPlan {
            reference: format!("private:{key}"),
            blobs: Vec::new(),
            records,
        }))
    }

    /// 参照を置き換えてから、旧領域の内容を写す(照合に通ったものだけ)。index 行が消えていれば写さない。
    async fn protect(&self, source: &LegacySource, plan: ProtectionPlan) -> Result<()> {
        let blobs = plan
            .blobs
            .into_iter()
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .collect::<Vec<_>>();
        let records = plan
            .records
            .into_iter()
            .filter_map(|(replica, record)| {
                let author = record.docs_author.clone()?;
                (blake3::hash(&record.value).to_hex().as_str() == record.content_hash
                    && record.content_len == record.value.len() as u64)
                    .then_some((replica, author, record))
            })
            .collect::<Vec<_>>();
        let mut desired = blobs
            .iter()
            .map(|hash| ("blob".to_string(), hash.clone()))
            .chain(records.iter().map(|(replica, author, record)| {
                (
                    "record".to_string(),
                    SqliteStore::remote_record_cache_key(replica.as_str(), &record.key, author),
                )
            }))
            .collect::<Vec<_>>();
        desired.sort();
        desired.dedup();
        if !self
            .store
            .set_protected_refs(&plan.reference, &desired)
            .await?
        {
            return Ok(());
        }
        for hash in &blobs {
            self.copy_legacy_blob(&source.node, hash).await?;
        }
        for (replica, author, record) in records {
            let payload = serde_json::to_vec(&DocReadRecord {
                key: record.key.clone(),
                value: record.value,
                content_hash: record.content_hash,
                content_len: record.content_len,
                docs_author: author.clone(),
            })?;
            self.store
                .put_remote_record(replica.as_str(), &record.key, &author, &payload)
                .await?;
        }
        Ok(())
    }

    async fn copy_legacy_blob(&self, node: &IrohDocsNode, hash: &str) -> Result<()> {
        if self.store.has_remote_content("blob", hash).await? {
            return Ok(());
        }
        let staging = self.db_path.with_extension("protected-migration.tmp");
        let length = match node.export_local_blob(hash, &staging).await {
            Ok(Some(length)) => length,
            Ok(None) => return Ok(()),
            Err(error) => {
                tracing::warn!(%error, hash, "legacy blob could not be copied");
                let _ = tokio::fs::remove_file(&staging).await;
                return Ok(());
            }
        };
        let result = if length <= INLINE_BLOB_BYTES {
            let bytes = tokio::fs::read(&staging).await?;
            self.store
                .put_remote_content("blob", hash, "blob", &bytes)
                .await
                .map(|_| ())
        } else {
            self.store.put_remote_blob_file(hash, &staging).await
        };
        let _ = tokio::fs::remove_file(&staging).await;
        result
    }
}
