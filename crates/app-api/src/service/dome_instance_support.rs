//! Dome の記録の置き場所と読取り(#1221 R5-H、2026-09-27 ユーザー決定)。
//!
//! - Dome の anchor: Dome の作成時刻(session の state の `created_at`)の context の scope bucket。private は現 epoch。
//!   接続の記録はここに置く(ADR 0054 §2)。書き手と読み手は同じ作成時刻から同じ anchor を導くので、日が変わっても
//!   変わらない。session の state は live・game と同じく更新の日の bucket へ移る(locator で読む)。
//! - Instance・hosting・owner だけが書く記録(layout・削除)は、公開の context なら owner の制御領域
//!   (`author::<owner>`)の instance の id の key、private の context なら Dome の anchor に置く(公開の領域へ置かない)。
//! - 旧 topic/channel replica へは書かない。更新前に置いた分は手元から読む。
//! - private の Dome を行の無い端末が見つけるため、owner の端末の hosting は、channel の現 epoch のその日の bucket に
//!   anchor を指す locator を 1 日 1 回置く。

use super::remote_read_support::{private_epochs_for_bucket, read_local_then_remote};
use super::*;
use kukuri_core::{SignedDomeLayoutCommitV1, SpatialContextV1};
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

const DOME_LOCATOR_PREFIX: &str = "metaverse/dome-locators";
const LAYOUT_COMMIT_PREFIX: &str = "metaverse/dome-layout-commits";

/// owner の制御領域の、Dome Instance の現在値の key。instance の id は context と owner から決まる(#1221 R5-H)。
fn dome_instance_state_key(instance_id: &str) -> String {
    stable_key("metaverse/dome-instances", &format!("{instance_id}/state"))
}

/// 署名済み envelope の Dome Instance と、署名された content(manifest blob と同じ bytes)の hash。
async fn load_signed_dome_instance_manifest(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    owner_pubkey: &Pubkey,
    policy: DocFetchPolicy,
) -> Result<Option<(DomeInstanceManifestV1, kukuri_core::BlobHash)>> {
    let key = stable_key("envelopes", envelope_id.as_str());
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            key.as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    for record in records {
        let envelope = match serde_json::from_slice::<KukuriEnvelope>(&record.value) {
            Ok(envelope) => envelope,
            Err(error) => {
                warn!(replica = %replica.as_str(), key, %error, "ignored an unreadable Dome Instance envelope");
                continue;
            }
        };
        if envelope.verify().is_err()
            || envelope.id != *envelope_id
            || envelope.kind != "dome-instance"
            || envelope.pubkey != *owner_pubkey
        {
            warn!(replica = %replica.as_str(), key, "ignored an invalid Dome Instance envelope");
            continue;
        }
        match serde_json::from_str::<DomeInstanceManifestV1>(&envelope.content) {
            Ok(manifest) => {
                return Ok(Some((
                    manifest,
                    kukuri_core::blob_hash(envelope.content.as_bytes()),
                )));
            }
            Err(error) => {
                warn!(replica = %replica.as_str(), key, %error, "ignored an unreadable signed Dome Instance manifest");
            }
        }
    }
    Ok(None)
}

impl AppService {
    /// Dome Instance を置く。公開の context は owner の制御領域、private の context は Dome の anchor(`created_at` は
    /// Dome の作成時刻)。
    pub(crate) async fn persist_dome_instance_manifest(
        &self,
        manifest: &DomeInstanceManifestV1,
        created_at: i64,
    ) -> Result<DomeInstanceStateDocV1> {
        let envelope = build_dome_instance_envelope(self.services.keys.as_ref(), manifest)?;
        let stored = store_manifest_blob(
            self.services.blob_service.as_ref(),
            manifest,
            DOME_INSTANCE_MANIFEST_MIME,
        )
        .await?;
        let state = DomeInstanceStateDocV1 {
            instance_id: manifest.instance_id.clone(),
            spatial_context: manifest.spatial_context.clone(),
            owner_pubkey: manifest.owner_pubkey.clone(),
            generation: manifest.generation,
            status: manifest.status,
            created_at,
            updated_at: manifest.updated_at,
            current_manifest: ManifestBlobRef {
                hash: stored.hash.clone(),
                mime: stored.mime.clone(),
                bytes: stored.bytes,
            },
            last_envelope_id: envelope.id.clone(),
        };
        let owner = manifest.owner_pubkey.as_str();
        let replica = match &manifest.spatial_context {
            SpatialContextV1::Topic { .. } => {
                self.services.persist_author_event(owner, &envelope).await?;
                author_replica_id(owner)
            }
            context @ SpatialContextV1::Channel { .. } => self
                .dome_anchor_candidates(context, created_at)
                .await?
                .swap_remove(0),
        };
        self.services.docs_sync.open_replica(&replica).await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key("envelopes", envelope.id.as_str()),
                    value: serde_json::to_value(&envelope)?,
                },
            )
            .await?;
        self.services
            .docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: dome_instance_state_key(&manifest.instance_id),
                    value: serde_json::to_value(&state)?,
                },
            )
            .await?;
        Ok(state)
    }

    /// owner の Dome Instance。記録の場所(`read_dome_record`)を exact に読み、無ければ更新前に旧 context replica へ
    /// 置いた分を手元から読む。
    pub(crate) async fn fetch_dome_instance_manifest(
        &self,
        spatial_context: &SpatialContextV1,
        owner_pubkey: &Pubkey,
    ) -> Result<Option<(DomeInstanceStateDocV1, DomeInstanceManifestV1)>> {
        let key = dome_instance_state_key(&dome_instance_id(spatial_context, owner_pubkey));
        let current = self
            .read_dome_record(spatial_context, owner_pubkey, |docs, replica, policy| {
                let key = &key;
                async move {
                    self.read_dome_instance(docs.as_ref(), &replica, key, owner_pubkey, policy)
                        .await
                }
            })
            .await?;
        if current.is_some() {
            return Ok(current);
        }
        let Some(legacy) = self.legacy_dome_replica(spatial_context).await else {
            return Ok(None);
        };
        let key = stable_key(
            "metaverse/dome-instances",
            &format!("{}/state", owner_pubkey.as_str()),
        );
        self.read_dome_instance(
            self.services.docs_sync.as_ref(),
            &legacy,
            &key,
            owner_pubkey,
            DocFetchPolicy::LocalOnly,
        )
        .await
    }

    /// 更新前に Dome の記録を置いていた旧 context replica。読むだけなので、private は参加中の現 epoch を回転なしで使う。
    pub(crate) async fn legacy_dome_replica(
        &self,
        context: &SpatialContextV1,
    ) -> Option<ReplicaId> {
        match context {
            SpatialContextV1::Topic { topic_id } => Some(topic_replica_id(topic_id.as_str())),
            SpatialContextV1::Channel {
                topic_id,
                channel_id,
            } => self
                .joined_private_channel_state(topic_id.as_str(), channel_id.as_str())
                .await
                .map(|state| current_private_channel_replica_id(&state)),
        }
    }

    async fn read_dome_instance(
        &self,
        docs: &dyn DocsSync,
        replica: &ReplicaId,
        key: &str,
        owner_pubkey: &Pubkey,
        policy: DocFetchPolicy,
    ) -> Result<Option<(DomeInstanceStateDocV1, DomeInstanceManifestV1)>> {
        let records = docs
            .query_replica_exact_bounded(replica, key, MAX_ENVELOPE_RECORDS_PER_OBJECT, policy)
            .await?;
        let mut newest: Option<(DomeInstanceStateDocV1, DomeInstanceManifestV1)> = None;
        let mut unavailable = None;
        for record in records {
            let state = match serde_json::from_slice::<DomeInstanceStateDocV1>(&record.value) {
                Ok(state) => state,
                Err(error) => {
                    warn!(replica = %replica.as_str(), key, %error, "ignored an unreadable Dome Instance state");
                    continue;
                }
            };
            if state.owner_pubkey != *owner_pubkey {
                warn!(replica = %replica.as_str(), key, "ignored a Dome Instance state with a mismatched owner");
                continue;
            }
            // manifest は owner が署名した envelope の content から読む(manifest blob と同じ bytes。hash で照合する)。
            // blob を別の経路で取りに行かない(#1221 R5-H: 取得の失敗・cooldown で、読めた Instance を欠けさせない)。
            let Some((manifest, signed_hash)) = load_signed_dome_instance_manifest(
                docs,
                replica,
                &state.last_envelope_id,
                &state.owner_pubkey,
                policy,
            )
            .await?
            else {
                unavailable = Some(DomeReadUnavailable::Envelope);
                continue;
            };
            if signed_hash != state.current_manifest.hash {
                warn!(replica = %replica.as_str(), key, "ignored a Dome Instance state that does not match its signed manifest");
                continue;
            }
            if let Err(error) = kukuri_core::validate_dome_instance_manifest(&manifest) {
                warn!(replica = %replica.as_str(), key, %error, "ignored an invalid Dome Instance manifest");
                continue;
            }
            if state.instance_id != manifest.instance_id
                || state.owner_pubkey != manifest.owner_pubkey
                || state.spatial_context != manifest.spatial_context
                || state.generation != manifest.generation
                || state.status != manifest.status
            {
                warn!(replica = %replica.as_str(), key, "ignored a Dome Instance state that does not match its manifest");
                continue;
            }
            if newest.as_ref().is_none_or(|(_, current)| {
                (manifest.generation, manifest.updated_at)
                    > (current.generation, current.updated_at)
            }) {
                newest = Some((state, manifest));
            }
        }
        match newest {
            Some(value) => Ok(Some(value)),
            None => match unavailable {
                Some(reason) => Err(reason.into()),
                None => Ok(None),
            },
        }
    }

    /// instance の id の Dome Instance。owner は、自分・一覧の行・hosting の heartbeat から instance の id を導ける
    /// ものを使う(旧 replica の session は読まない。#1221 R5-H)。
    pub(crate) async fn hosting_instance(
        &self,
        spatial_context: &SpatialContextV1,
        instance_id: &str,
    ) -> Result<Option<DomeInstanceManifestV1>> {
        let row_owner = self
            .services
            .projection_store
            .get_game_room(spatial_context.topic_id().as_str(), instance_id)
            .await?
            .map(|row| Pubkey::from(row.host_pubkey));
        let owner = std::iter::once(self.services.keys.public_key())
            .chain(row_owner)
            .chain(self.heartbeat_dome_owners(spatial_context).await)
            .find(|owner| dome_instance_id(spatial_context, owner) == instance_id);
        match owner {
            Some(owner) => {
                self.hosting_instance_for_owner(spatial_context, instance_id, &owner)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn hosting_instance_for_owner(
        &self,
        spatial_context: &SpatialContextV1,
        instance_id: &str,
        owner_pubkey: &Pubkey,
    ) -> Result<Option<DomeInstanceManifestV1>> {
        if dome_instance_id(spatial_context, owner_pubkey) != instance_id {
            return Ok(None);
        }
        let resolved = self
            .fetch_dome_instance_manifest(spatial_context, owner_pubkey)
            .await?;
        let Some((_, manifest)) = resolved else {
            return Ok(None);
        };
        if manifest.instance_id != instance_id
            || manifest.spatial_context != *spatial_context
            || manifest.owner_pubkey != *owner_pubkey
        {
            warn!(
                instance_id,
                owner_pubkey = %owner_pubkey.as_str(),
                "ignored a Dome Instance that does not match its lookup identity"
            );
            return Ok(None);
        }
        Ok(Some(manifest))
    }

    /// Dome の作成時刻(ミリ秒)から導く anchor(書く先)と、読むだけの候補。書く先は作成時刻の context の scope bucket
    /// (private は現 epoch。切替前は旧 context replica)。private は、作成時刻の bucket に重なる回転の前の epoch の
    /// bucket(回転の前に置いた記録)も読む。
    async fn dome_anchor_candidates(
        &self,
        context: &SpatialContextV1,
        created_at: i64,
    ) -> Result<Vec<ReplicaId>> {
        let topic = context.topic_id().as_str();
        let Some(channel_id) = context.channel_id() else {
            return Ok(vec![self.services.scope_write_replica(
                topic,
                None,
                created_at / 1_000,
            )?]);
        };
        let private = self
            .joined_private_channel_state(topic, channel_id.as_str())
            .await
            .context("private channel is not joined")?;
        let mut anchors =
            vec![
                self.services
                    .scope_write_replica(topic, Some(&private), created_at / 1_000)?,
            ];
        if self.services.writes_buckets() {
            let bucket = TimeBucket::from_unix_seconds(created_at / 1_000)?;
            for (epoch_id, _) in private_epochs_for_bucket(&private, bucket) {
                let replica = BucketReplica::new(
                    BucketScope::PrivateChannel {
                        channel_id: channel_id.as_str().to_owned(),
                        epoch_id,
                    },
                    bucket,
                )?
                .replica_id();
                if !anchors.contains(&replica) {
                    anchors.push(replica);
                }
            }
        }
        Ok(anchors)
    }

    /// 既にある Dome の anchor の候補(書く先を先に)。作成時刻は、一覧の行があれば session の state、無ければ公開は
    /// owner の制御領域の Instance から知る。行の無い private は、owner の hosting が置く locator から。行の session の
    /// replica も読む(更新前の記録)。
    pub(crate) async fn dome_anchors(
        &self,
        context: &SpatialContextV1,
        instance_id: &str,
        owner: &Pubkey,
    ) -> Result<Vec<ReplicaId>> {
        let topic = context.topic_id().as_str();
        let row = self
            .services
            .projection_store
            .get_game_room(topic, instance_id)
            .await?
            .filter(|row| row.channel_id == channel_storage_id(context.channel_id()));
        let created_at = match (&row, context) {
            (Some(row), _) => self.dome_session_created_at(topic, row).await?,
            (None, SpatialContextV1::Topic { .. }) => {
                let author = author_replica_id(owner.as_str());
                let key = dome_instance_state_key(instance_id);
                self.read_author_object(owner.as_str(), |docs, policy| {
                    let (author, key) = (&author, &key);
                    async move {
                        self.read_dome_instance(docs.as_ref(), author, key, owner, policy)
                            .await
                    }
                })
                .await?
                .map(|(state, _)| state.created_at)
            }
            (None, SpatialContextV1::Channel { .. }) => {
                return self.read_dome_locators(context, instance_id).await;
            }
        };
        let mut anchors = match created_at {
            Some(created_at) => self.dome_anchor_candidates(context, created_at).await?,
            None => Vec::new(),
        };
        if let Some(row) = row
            && !anchors.contains(&row.source_replica_id)
        {
            anchors.push(row.source_replica_id);
        }
        Ok(anchors)
    }

    /// 一覧の行の Dome の作成時刻。行の session の state を手元から読み、無ければ session を読む(locator・provider)。
    async fn dome_session_created_at(
        &self,
        topic: &str,
        row: &GameRoomProjectionRow,
    ) -> Result<Option<i64>> {
        let local = self
            .services
            .docs_sync
            .query_replica_with_policy(
                &row.source_replica_id,
                DocQuery::Exact(row.source_key.clone()),
                DocFetchPolicy::LocalOnly,
            )
            .await?
            .into_iter()
            .find_map(|record| serde_json::from_slice::<GameRoomStateDocV1>(&record.value).ok())
            .filter(|state| state.room_id == row.room_id);
        if let Some(state) = local {
            return Ok(Some(state.created_at));
        }
        // 呼び出し側の future が `Send` のまま spawn できるように、型を消してから待つ。
        let session: std::pin::Pin<
            Box<dyn Future<Output = Result<Option<VerifiedGameRoom>>> + Send + '_>,
        > = Box::pin(self.fetch_verified_game_room(
            topic,
            row.channel_id.as_str(),
            row.room_id.as_str(),
        ));
        Ok(session.await?.map(|verified| verified.state().created_at))
    }

    /// Instance・hosting・owner だけが書く記録を書く replica。公開は owner の制御領域、private は Dome の anchor。
    pub(crate) async fn dome_record_replica(
        &self,
        instance: &DomeInstanceManifestV1,
    ) -> Result<ReplicaId> {
        let owner = &instance.owner_pubkey;
        match &instance.spatial_context {
            SpatialContextV1::Topic { .. } => Ok(author_replica_id(owner.as_str())),
            context @ SpatialContextV1::Channel { .. } => self
                .dome_anchors(context, &instance.instance_id, owner)
                .await?
                .into_iter()
                .next()
                .context("the private Dome's anchor is unknown"),
        }
    }

    /// Dome の記録を、公開は owner の制御領域(手元→owner を含む有界な provider)、private は anchor の候補ごとに
    /// 手元→capability を持つ channel の provider の順に exact に読む。
    pub(crate) async fn read_dome_record<T, Fut>(
        &self,
        context: &SpatialContextV1,
        owner: &Pubkey,
        read: impl Fn(Arc<dyn DocsSync>, ReplicaId, DocFetchPolicy) -> Fut,
    ) -> Result<Option<T>>
    where
        Fut: Future<Output = Result<Option<T>>>,
    {
        let SpatialContextV1::Channel {
            topic_id,
            channel_id,
        } = context
        else {
            let author = author_replica_id(owner.as_str());
            return self
                .read_author_object(owner.as_str(), |docs, policy| {
                    read(docs, author.clone(), policy)
                })
                .await;
        };
        for anchor in self
            .dome_anchors(context, &dome_instance_id(context, owner), owner)
            .await?
        {
            let readers =
                self.private_dome_readers(topic_id.as_str(), channel_id.as_str(), &anchor);
            let found =
                read_local_then_remote(self.services.docs_sync.clone(), readers, |docs, policy| {
                    read(docs, anchor.clone(), policy)
                })
                .await?;
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// private の bucket を、その epoch の capability で channel の provider から読む reader。
    pub(crate) async fn private_dome_readers(
        &self,
        topic: &str,
        channel: &str,
        replica: &ReplicaId,
    ) -> Vec<Arc<dyn DocsSync>> {
        if !replica.as_str().starts_with("bucket::") {
            return Vec::new();
        }
        let readers = async {
            let Some((epoch, secret)) = self
                .private_epoch_for_source(topic, channel, replica)
                .await?
            else {
                return Ok(Vec::new());
            };
            self.remote_post_readers(topic, Some(channel), replica, Some((&epoch, &secret)))
                .await
        };
        readers.await.unwrap_or_else(|error: anyhow::Error| {
            warn!(%error, "private Dome reader selection failed");
            Vec::new()
        })
    }

    /// 現在と直前の channel の bucket に置かれた、private の Dome の anchor を指す locator。
    async fn read_dome_locators(
        &self,
        context: &SpatialContextV1,
        instance_id: &str,
    ) -> Result<Vec<ReplicaId>> {
        let SpatialContextV1::Channel {
            topic_id,
            channel_id,
        } = context
        else {
            return Ok(Vec::new());
        };
        let scope = TimelineScope::Channel {
            channel_id: channel_id.clone(),
        };
        let key = stable_key(DOME_LOCATOR_PREFIX, instance_id);
        let mut anchors = Vec::new();
        for (bucket, _) in self
            .remote_page_replicas(topic_id.as_str(), &scope, None, false, false)
            .await?
            .into_iter()
            .filter(|(replica, _)| replica.as_str().starts_with("bucket::"))
        {
            let readers =
                self.private_dome_readers(topic_id.as_str(), channel_id.as_str(), &bucket);
            let found =
                read_local_then_remote(self.services.docs_sync.clone(), readers, |docs, policy| {
                    let (bucket, key) = (bucket.clone(), key.clone());
                    async move {
                        Ok(docs
                            .query_replica_with_policy(&bucket, DocQuery::Exact(key), policy)
                            .await?
                            .into_iter()
                            .next())
                    }
                })
                .await?;
            let anchor = found
                .and_then(|record| serde_json::from_slice::<String>(&record.value).ok())
                .map(ReplicaId::new)
                .filter(|anchor| {
                    BucketReplica::parse(anchor).is_ok_and(|parsed| {
                        matches!(parsed.scope(), BucketScope::PrivateChannel { channel_id: id, .. } if id == channel_id.as_str())
                    })
                });
            if let Some(anchor) = anchor
                && !anchors.contains(&anchor)
            {
                anchors.push(anchor);
            }
        }
        Ok(anchors)
    }

    /// owner だけが書く記録を読む手元の replica(記録の場所と、更新前の旧 context replica)。
    pub(crate) async fn owned_dome_record_replicas(
        &self,
        instance: &DomeInstanceManifestV1,
    ) -> Result<Vec<ReplicaId>> {
        let mut replicas = vec![self.dome_record_replica(instance).await?];
        replicas.extend(self.legacy_dome_replica(&instance.spatial_context).await);
        Ok(replicas)
    }

    pub(crate) async fn find_dome_layout_commit(
        &self,
        instance: &DomeInstanceManifestV1,
        operation_id: &str,
    ) -> Result<Option<SignedDomeLayoutCommitV1>> {
        let key = stable_key(
            LAYOUT_COMMIT_PREFIX,
            &format!("{}/{operation_id}", instance.instance_id),
        );
        for replica in self.owned_dome_record_replicas(instance).await? {
            let record = self
                .services
                .docs_sync
                .query_replica(&replica, DocQuery::Exact(key.clone()))
                .await?
                .into_iter()
                .next();
            if let Some(record) = record {
                return Ok(Some(serde_json::from_slice(&record.value)?));
            }
        }
        Ok(None)
    }

    pub(crate) async fn last_dome_layout_commit_at(
        &self,
        instance: &DomeInstanceManifestV1,
    ) -> Result<Option<i64>> {
        let prefix = stable_key(LAYOUT_COMMIT_PREFIX, &format!("{}/", instance.instance_id));
        let mut records = Vec::new();
        for replica in self.owned_dome_record_replicas(instance).await? {
            records.extend(
                self.services
                    .docs_sync
                    .query_replica(&replica, DocQuery::Prefix(prefix.clone()))
                    .await?,
            );
        }
        let mut latest: Option<i64> = None;
        for record in records {
            let commit: SignedDomeLayoutCommitV1 = serde_json::from_slice(&record.value)?;
            latest = Some(
                latest
                    .unwrap_or(commit.commit.committed_at)
                    .max(commit.commit.committed_at),
            );
        }
        Ok(latest)
    }

    pub(crate) async fn persist_dome_layout_commit(
        &self,
        replica: &ReplicaId,
        signed: &SignedDomeLayoutCommitV1,
    ) -> Result<()> {
        self.services.docs_sync.open_replica(replica).await?;
        self.services
            .docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: stable_key(
                        LAYOUT_COMMIT_PREFIX,
                        &format!(
                            "{}/{}",
                            signed.commit.instance_id, signed.commit.operation_id
                        ),
                    ),
                    value: serde_json::to_value(signed)?,
                },
            )
            .await
    }

    /// context の Dome Instance(#1221 R5-H)。replica は走査しない。手元の一覧の metaverse の行の owner、hosting の
    /// heartbeat で知った owner、自分、`owners`(提案・接続の端点)について、owner の制御領域を exact に読む。
    pub(crate) async fn list_context_dome_instances(
        &self,
        context: &SpatialContextV1,
        owners: impl IntoIterator<Item = Pubkey>,
    ) -> Result<Vec<DomeInstanceManifestV1>> {
        let rows = self
            .services
            .projection_store
            .list_channel_game_rooms(
                context.topic_id().as_str(),
                channel_storage_id(context.channel_id()).as_str(),
                LIVE_GAME_LIST_LIMIT,
            )
            .await?;
        let owners = rows
            .into_iter()
            .filter(|row| row.room_kind == GameRoomKind::MetaverseRoom)
            .map(|row| row.host_pubkey)
            .chain(
                owners
                    .into_iter()
                    .chain(self.heartbeat_dome_owners(context).await)
                    .map(|owner| owner.as_str().to_string()),
            )
            .chain([self.current_author_pubkey()])
            .collect::<BTreeSet<_>>();
        let mut instances = Vec::new();
        for owner in owners {
            let resolved = match self
                .fetch_dome_instance_manifest(context, &Pubkey::from(owner))
                .await
            {
                Ok(value) => value,
                Err(error) if error.downcast_ref::<DomeReadUnavailable>().is_some() => continue,
                Err(error) => return Err(error),
            };
            if let Some((_, manifest)) = resolved
                && manifest.status == kukuri_core::DomeInstanceStatusV1::Active
                && manifest.relationship_detach.is_none()
            {
                instances.push(manifest);
            }
        }
        instances.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        instances.dedup_by(|left, right| left.instance_id == right.instance_id);
        Ok(instances)
    }
}

/// private の Dome の anchor を指す locator を、channel の現 epoch のその日の bucket へ置く(owner の端末の hosting が
/// 1 日 1 回置く。#1221 R5-H)。置いた日の bucket を返す。
pub(crate) async fn persist_dome_locator(
    services: &ServiceHandles,
    context: &SpatialContextV1,
    instance_id: &str,
    anchor: &ReplicaId,
    now_secs: i64,
) -> Result<TimeBucket> {
    let SpatialContextV1::Channel {
        topic_id,
        channel_id,
    } = context
    else {
        anyhow::bail!("a Dome locator is only for a private context");
    };
    let state = services
        .joined_private_channels
        .lock()
        .await
        .get(&joined_private_channel_key(
            topic_id.as_str(),
            channel_id.as_str(),
        ))
        .cloned()
        .context("private channel is not joined")?;
    let bucket = TimeBucket::from_unix_seconds(now_secs)?;
    let replica = BucketReplica::new(
        BucketScope::PrivateChannel {
            channel_id: channel_id.as_str().to_owned(),
            epoch_id: state.current_epoch_id.clone(),
        },
        bucket,
    )?
    .replica_id();
    services.docs_sync.open_replica(&replica).await?;
    services
        .docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key(DOME_LOCATOR_PREFIX, instance_id),
                value: serde_json::to_value(anchor.as_str())?,
            },
        )
        .await?;
    Ok(bucket)
}
