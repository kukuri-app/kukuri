//! docs から反映する live session・game room の検証(#1252)。
//!
//! `sessions/live/<id>/state`・`sessions/game/<id>/state` の doc と manifest blob は署名を持たない。書く側は manifest 全体を
//! content にした envelope(`live-session` / `game-session`)に署名して同じ replica の `envelopes/<envelope id>` へ置き、
//! `state.last_envelope_id` がそれを指す(Dome の Instance / Preset と同じ形)。読む側は、署名された manifest と、読んだ
//! replica が受け入れる topic / channel に照らして確かめる。検証に通った session だけが `VerifiedLiveSession` /
//! `VerifiedGameRoom` になり、projection の行と、操作が使う state / manifest は、そこからしか作れない。
//!
//! 読む docs の record は、session 1 件あたり `state` の key と `envelopes/<id>` の key の、それぞれ上限つきの件数だけ。

use super::*;
use kukuri_core::SpatialContextV1;

pub(crate) const LIVE_SESSION_ENVELOPE_KIND: &str = "live-session";
pub(crate) const GAME_SESSION_ENVELOPE_KIND: &str = "game-session";

/// 欠損は到着eventまたは表示要求で解決する。拒否をretryの契機にしない。
pub(crate) enum SessionRead<T> {
    Ready(T),
    MissingDocs,
    MissingManifest(kukuri_core::BlobHash),
    Rejected,
}

impl<T> SessionRead<T> {
    pub(crate) fn verified(self) -> Option<T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// manifest の blob。remote から読む state(hint の exact 読取り・読み直し)は、manifest も provider から取得する
/// (#1221 R5-H。旧 sync の後の取得 worker は無い)。
async fn read_session_blob<T: DeserializeOwned>(
    blobs: &dyn BlobService,
    blob: &ManifestBlobRef,
    policy: DocFetchPolicy,
) -> Result<Option<T>> {
    let bytes = match policy {
        DocFetchPolicy::LocalOnly => blobs.fetch_local_blob(&blob.hash).await?,
        _ => blobs.fetch_blob(&blob.hash).await?,
    };
    match bytes {
        Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        None => Ok(None),
    }
}

/// manifest blob は、その session の state を返した peer が持つと分かっている(候補の順序の手がかりだけ。#1395)。
async fn learn_manifest_source(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    blob: &ManifestBlobRef,
) {
    if let Some(provider) = docs_sync.remote_reader_id() {
        let _ = blob_service
            .learn_content_source(&blob.hash, &provider)
            .await;
    }
}

/// 新しく作る session id の末尾に置く、owner の pubkey の先頭の桁数(64 bit)。
const OWNER_BOUND_ID_SUFFIX_LEN: usize = 16;
/// 読む側が受け付ける最小の桁数。修正前の id(`short_id_suffix` の 8 桁)を通すための下限。
const MIN_OWNER_BOUND_ID_SUFFIX_LEN: usize = 8;

/// 検証に通らなかった理由。I/O の失敗は含まない(呼び出し側へ `Err` で返す)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRejection {
    /// session を置く replica ではない、または replica と購読の topic が合わない。
    UnsupportedReplica,
    /// record が state doc として読めない。
    UnreadableState,
    /// state の id が key の id と違う。
    KeyMismatch,
    /// owner の署名つき envelope がまだ無い、または読めない(後から届けば反映できる)。
    SignedManifestUnavailable,
    /// 署名者が manifest の owner と違う。
    SignerIsNotOwner,
    /// id が owner の pubkey と結び付いていない。
    IdNotBoundToOwner,
    /// state・manifest blob が、署名された manifest と合わない。
    ManifestMismatch,
    /// owner の署名対象となる単調増加 revision が無い、または不正。
    InvalidRevision,
    /// manifest が申告する topic・channel が、読んだ replica と合わない。
    ScopeMismatch,
    /// 署名された更新時刻が、読んだ bucket と合わない。
    BucketTimeMismatch,
}

impl SessionRejection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedReplica => "the replica does not hold sessions for this topic",
            Self::UnreadableState => "the record is not a session state",
            Self::KeyMismatch => "the session id does not match its key",
            Self::SignedManifestUnavailable => "no signed manifest backs the session state",
            Self::SignerIsNotOwner => "the manifest was not signed by the session owner",
            Self::IdNotBoundToOwner => "the session id is not bound to its owner",
            Self::ManifestMismatch => "the state or manifest blob differs from the signed manifest",
            Self::InvalidRevision => "the signed session revision is missing or invalid",
            Self::ScopeMismatch => "the session does not belong to the replica it was read from",
            Self::BucketTimeMismatch => "the signed session update is outside its bucket",
        }
    }
}

pub(crate) fn warn_rejected_session(replica: &ReplicaId, key: &str, reason: SessionRejection) {
    warn!(
        replica = %replica.as_str(),
        key,
        reason = reason.as_str(),
        "ignored a session that cannot be verified"
    );
}

/// 新しく作る live session・ScoreGame の id の末尾。owner の pubkey の先頭 16 桁。
pub(crate) fn owner_bound_id_suffix(owner_pubkey: &str) -> &str {
    owner_pubkey
        .get(..OWNER_BOUND_ID_SUFFIX_LEN)
        .unwrap_or(owner_pubkey)
}

/// id の末尾(最後の `-` より後)が、owner の pubkey の先頭と一致するか。
///
/// docs は同じ key を別の鍵でも書けるので、id と owner を結び付けないと、別の鍵で署名した state で他人の session を
/// 上書きできる。修正前の id は 8 桁(32 bit)、新しい id は 16 桁(64 bit)。
fn id_is_bound_to_owner(id: &str, owner: &Pubkey) -> bool {
    let Some((_, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    suffix.len() >= MIN_OWNER_BOUND_ID_SUFFIX_LEN
        && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        && owner.as_str().starts_with(suffix)
}

/// Dome の id の作り方(`create_metaverse_room_in_channel`・Dome の移動)。Spatial Context と owner から決まる(96 bit)。
pub(crate) fn dome_instance_id(spatial_context: &SpatialContextV1, owner: &Pubkey) -> String {
    let hash = kukuri_core::blob_hash(format!(
        "dome-instance:{}:{}",
        spatial_context.canonical_id(),
        owner.as_str()
    ));
    format!("dome-{}", &hash.as_str()[..24])
}

fn session_id_from_key<'a>(key: &'a str, prefix: &str) -> Option<&'a str> {
    let id = key.strip_prefix(prefix)?.strip_suffix("/state")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// 検証した session の record と manifest の中身(#1395)。remote から読んで受け入れた版を保持し、他の参加者へ提供する。
#[derive(Clone, Debug)]
pub(crate) struct SessionRelay {
    state_key: String,
    state_author: Option<String>,
    envelope_key: String,
    envelope_author: Option<String>,
    /// 署名された content(manifest blob の bytes と同じ)と mime。
    manifest: Vec<u8>,
    mime: String,
}

impl SessionRelay {
    /// remote の reader から読んだときだけ、state と署名つき envelope の record と manifest blob を保持する。
    pub(crate) async fn keep(
        &self,
        docs_sync: &dyn DocsSync,
        blob_service: &dyn BlobService,
        replica: &ReplicaId,
    ) -> Result<()> {
        if docs_sync.remote_reader_id().is_none() {
            return Ok(());
        }
        for (key, author) in [
            (&self.state_key, &self.state_author),
            (&self.envelope_key, &self.envelope_author),
        ] {
            docs_sync
                .persist_verified_record(replica, key, author.as_deref(), &[])
                .await?;
        }
        blob_service
            .put_remote_blob(self.manifest.clone(), &self.mime)
            .await?;
        Ok(())
    }
}

/// 署名と id と kind が正しい envelope の署名者、manifest、署名された生 bytes の hash と、その record の名義と content。
struct SignedManifest<T> {
    signer: Pubkey,
    manifest: T,
    hash: kukuri_core::BlobHash,
    signed_at: i64,
    envelope_key: String,
    envelope_author: Option<String>,
    content: String,
}

impl<T> SignedManifest<T> {
    fn relay(&self, state: &DocRecord, mime: &str) -> SessionRelay {
        SessionRelay {
            state_key: state.key.clone(),
            state_author: state.docs_author.clone(),
            envelope_key: self.envelope_key.clone(),
            envelope_author: self.envelope_author.clone(),
            manifest: self.content.clone().into_bytes(),
            mime: mime.to_string(),
        }
    }
}

async fn load_signed_manifest<T: DeserializeOwned>(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope_id: &EnvelopeId,
    expected_kind: &str,
    policy: DocFetchPolicy,
) -> Result<SessionRead<SignedManifest<T>>> {
    let envelope_key = stable_key("envelopes", envelope_id.as_str());
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            envelope_key.as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    let verified = records.iter().find_map(|record| {
        let envelope = serde_json::from_slice::<KukuriEnvelope>(&record.value).ok()?;
        (envelope.verify().is_ok()
            && envelope.id == *envelope_id
            && envelope.kind == expected_kind)
            .then_some(())?;
        let manifest = serde_json::from_str::<T>(envelope.content.as_str()).ok()?;
        Some(SignedManifest {
            signer: envelope.pubkey,
            manifest,
            hash: kukuri_core::blob_hash(envelope.content.as_bytes()),
            signed_at: envelope.created_at,
            envelope_key: envelope_key.clone(),
            envelope_author: record.docs_author.clone(),
            content: envelope.content,
        })
    });
    Ok(match verified {
        Some(value) => SessionRead::Ready(value),
        None if records.is_empty() => SessionRead::MissingDocs,
        None => SessionRead::Rejected,
    })
}

/// 署名者と、署名された manifest と、読んだ replica の scope に照らして確かめた live session。
#[derive(Clone, Debug)]
pub(crate) struct VerifiedLiveSession {
    state: LiveSessionStateDocV1,
    manifest: LiveSessionManifestBlobV1,
    topic_id: String,
    replica: ReplicaId,
    relay: Option<SessionRelay>,
}

impl VerifiedLiveSession {
    /// docs も blob も読まない。`signer` は、`manifest` を content にした envelope の署名者。
    pub(crate) fn verify(
        state: LiveSessionStateDocV1,
        signer: &Pubkey,
        signed_at: i64,
        manifest: LiveSessionManifestBlobV1,
        replica: &ReplicaId,
        subscription_topic_id: &str,
    ) -> std::result::Result<Self, SessionRejection> {
        let scope = ReplicaPostScope::for_replica(replica, subscription_topic_id)
            .ok_or(SessionRejection::UnsupportedReplica)?;
        if !scope.accepts_created_at(signed_at) {
            return Err(SessionRejection::BucketTimeMismatch);
        }
        if *signer != manifest.owner_pubkey {
            return Err(SessionRejection::SignerIsNotOwner);
        }
        if manifest.revision < 1 {
            return Err(SessionRejection::InvalidRevision);
        }
        if !id_is_bound_to_owner(manifest.session_id.as_str(), &manifest.owner_pubkey) {
            return Err(SessionRejection::IdNotBoundToOwner);
        }
        if state.session_id != manifest.session_id
            || state.topic_id != manifest.topic_id
            || state.channel_id != manifest.channel_id
            || state.owner_pubkey != manifest.owner_pubkey
            || state.status != manifest.status
        {
            return Err(SessionRejection::ManifestMismatch);
        }
        if !scope.accepts(&manifest.topic_id, manifest.channel_id.as_ref()) {
            return Err(SessionRejection::ScopeMismatch);
        }
        Ok(Self {
            state,
            manifest,
            topic_id: subscription_topic_id.to_string(),
            replica: replica.clone(),
            relay: None,
        })
    }

    pub(crate) fn relay(&self) -> Option<&SessionRelay> {
        self.relay.as_ref()
    }

    pub(crate) fn state(&self) -> &LiveSessionStateDocV1 {
        &self.state
    }

    pub(crate) fn manifest(&self) -> &LiveSessionManifestBlobV1 {
        &self.manifest
    }

    pub(crate) fn revision(&self) -> i64 {
        self.manifest.revision
    }

    pub(crate) fn topic_id(&self) -> &str {
        self.topic_id.as_str()
    }

    pub(crate) fn replica(&self) -> &ReplicaId {
        &self.replica
    }

    pub(crate) fn into_parts(
        self,
    ) -> (ReplicaId, LiveSessionStateDocV1, LiveSessionManifestBlobV1) {
        (self.replica, self.state, self.manifest)
    }
}

/// `sessions/live/<id>/state` の record を 1 件検証する。
///
/// 署名された manifest を確かめてから manifest blob を読む(未検証の state が指す blob を取りに行かない)。
/// blob がまだ無ければ `Ok(None)`(後から届けば反映できる)。検証に通らなければ warn を出して `Ok(None)`。
pub(crate) async fn verify_live_session_record(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    record: &DocRecord,
    policy: DocFetchPolicy,
) -> Result<Option<VerifiedLiveSession>> {
    Ok(inspect_live_session_record(
        docs_sync,
        blob_service,
        replica,
        subscription_topic_id,
        record,
        policy,
    )
    .await?
    .verified())
}

pub(crate) async fn inspect_live_session_record(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    record: &DocRecord,
    policy: DocFetchPolicy,
) -> Result<SessionRead<VerifiedLiveSession>> {
    let rejected = |reason| {
        warn_rejected_session(replica, record.key.as_str(), reason);
        Ok(SessionRead::Rejected)
    };
    let Ok(state) = serde_json::from_slice::<LiveSessionStateDocV1>(&record.value) else {
        return rejected(SessionRejection::UnreadableState);
    };
    if session_id_from_key(record.key.as_str(), "sessions/live/") != Some(state.session_id.as_str())
    {
        return rejected(SessionRejection::KeyMismatch);
    }
    let signed = load_signed_manifest::<LiveSessionManifestBlobV1>(
        docs_sync,
        replica,
        &state.last_envelope_id,
        LIVE_SESSION_ENVELOPE_KIND,
        policy,
    )
    .await?;
    let signed = match signed {
        SessionRead::Ready(value) => value,
        SessionRead::MissingDocs => return Ok(SessionRead::MissingDocs),
        _ => return rejected(SessionRejection::SignedManifestUnavailable),
    };
    let current_manifest = state.current_manifest.clone();
    let relay = signed.relay(record, &current_manifest.mime);
    let mut verified = match VerifiedLiveSession::verify(
        state,
        &signed.signer,
        signed.signed_at,
        signed.manifest,
        replica,
        subscription_topic_id,
    ) {
        Ok(verified) => verified,
        Err(reason) => return rejected(reason),
    };
    if current_manifest.hash != signed.hash {
        return rejected(SessionRejection::ManifestMismatch);
    }
    verified.relay = Some(relay);
    learn_manifest_source(docs_sync, blob_service, &current_manifest).await;
    match read_session_blob::<LiveSessionManifestBlobV1>(blob_service, &current_manifest, policy)
        .await
    {
        Ok(Some(blob)) if blob == verified.manifest => Ok(SessionRead::Ready(verified)),
        Ok(Some(_)) | Err(_) => rejected(SessionRejection::ManifestMismatch),
        Ok(None) => Ok(SessionRead::MissingManifest(current_manifest.hash)),
    }
}

/// live session を id で読む。同じ key の record は上限つきで読み、検証に通るもののうち最も新しい 1 件を使う。
pub(crate) async fn load_verified_live_session(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    session_id: &str,
    policy: DocFetchPolicy,
) -> Result<Option<VerifiedLiveSession>> {
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            stable_key("sessions/live", &format!("{session_id}/state")).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    let mut newest: Option<VerifiedLiveSession> = None;
    for record in &records {
        if let Some(verified) = verify_live_session_record(
            docs_sync,
            blob_service,
            replica,
            subscription_topic_id,
            record,
            policy,
        )
        .await?
            && newest
                .as_ref()
                .is_none_or(|current| verified.revision() > current.revision())
        {
            newest = Some(verified);
        }
    }
    Ok(newest)
}

/// 署名された manifest(または Dome の id)と、読んだ replica の scope に照らして確かめた game room。
#[derive(Clone, Debug)]
pub(crate) struct VerifiedGameRoom {
    state: GameRoomStateDocV1,
    manifest: GameRoomManifestBlobV1,
    topic_id: String,
    replica: ReplicaId,
    relay: Option<SessionRelay>,
}

impl VerifiedGameRoom {
    /// docs も blob も読まない。`signed` は、`manifest` を content にした envelope の署名者と署名時刻(無ければ `None`)。
    /// 署名時刻が読んだ bucket の日に属さなければ受け付けない(書く側も同じ検証を通す)。
    ///
    /// - ScoreGame: owner の署名と、owner に結び付いた id を要求する。
    /// - metaverse room: 訪問者も chat で manifest を書く設計なので、owner の署名は要求しない。id が Spatial Context と
    ///   owner から決まる値であることだけを確かめる。一覧は、署名つきの Dome Instance でさらに確かめる(ADR 0036)。
    pub(crate) fn verify(
        state: GameRoomStateDocV1,
        signed: Option<(&Pubkey, i64)>,
        manifest: GameRoomManifestBlobV1,
        replica: &ReplicaId,
        subscription_topic_id: &str,
    ) -> std::result::Result<Self, SessionRejection> {
        let scope = ReplicaPostScope::for_replica(replica, subscription_topic_id)
            .ok_or(SessionRejection::UnsupportedReplica)?;
        if signed.is_some_and(|(_, signed_at)| !scope.accepts_created_at(signed_at)) {
            return Err(SessionRejection::BucketTimeMismatch);
        }
        let signer = signed.map(|(signer, _)| signer);
        match manifest.room_kind {
            GameRoomKind::ScoreGame => {
                let Some(signer) = signer else {
                    return Err(SessionRejection::SignedManifestUnavailable);
                };
                if *signer != manifest.owner_pubkey {
                    return Err(SessionRejection::SignerIsNotOwner);
                }
                if !id_is_bound_to_owner(manifest.room_id.as_str(), &manifest.owner_pubkey) {
                    return Err(SessionRejection::IdNotBoundToOwner);
                }
                if manifest.score_revision.is_none_or(|revision| revision < 1) {
                    return Err(SessionRejection::InvalidRevision);
                }
            }
            GameRoomKind::MetaverseRoom => {
                let Some(metaverse) = manifest.metaverse.as_ref() else {
                    return Err(SessionRejection::ManifestMismatch);
                };
                let context = &metaverse.spatial_context;
                let context_channel = match context {
                    SpatialContextV1::Topic { .. } => None,
                    SpatialContextV1::Channel { channel_id, .. } => Some(channel_id),
                };
                if context.topic_id() != &manifest.topic_id
                    || context_channel != manifest.channel_id.as_ref()
                {
                    return Err(SessionRejection::ScopeMismatch);
                }
                if manifest.room_id != dome_instance_id(context, &manifest.owner_pubkey) {
                    return Err(SessionRejection::IdNotBoundToOwner);
                }
            }
        }
        if state.room_id != manifest.room_id
            || state.topic_id != manifest.topic_id
            || state.channel_id != manifest.channel_id
            || state.owner_pubkey != manifest.owner_pubkey
            || state.status != manifest.status
        {
            return Err(SessionRejection::ManifestMismatch);
        }
        if !scope.accepts(&manifest.topic_id, manifest.channel_id.as_ref()) {
            return Err(SessionRejection::ScopeMismatch);
        }
        Ok(Self {
            state,
            manifest,
            topic_id: subscription_topic_id.to_string(),
            replica: replica.clone(),
            relay: None,
        })
    }

    pub(crate) fn relay(&self) -> Option<&SessionRelay> {
        self.relay.as_ref()
    }

    pub(crate) fn state(&self) -> &GameRoomStateDocV1 {
        &self.state
    }

    pub(crate) fn manifest(&self) -> &GameRoomManifestBlobV1 {
        &self.manifest
    }

    pub(crate) fn score_revision(&self) -> Option<i64> {
        self.manifest.score_revision
    }

    pub(crate) fn topic_id(&self) -> &str {
        self.topic_id.as_str()
    }

    pub(crate) fn replica(&self) -> &ReplicaId {
        &self.replica
    }

    pub(crate) fn into_parts(self) -> (ReplicaId, GameRoomStateDocV1, GameRoomManifestBlobV1) {
        (self.replica, self.state, self.manifest)
    }
}

/// `sessions/game/<id>/state` の record を 1 件検証する。
///
/// 署名つきの manifest があれば、それを確かめてから manifest blob を読む。署名つきの manifest が無い state は、
/// state の scope と owner に結び付いた Dome の id のみ manifest blob から確かめる(旧 client の互換例外)。
pub(crate) async fn verify_game_room_record(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    record: &DocRecord,
    policy: DocFetchPolicy,
) -> Result<Option<VerifiedGameRoom>> {
    Ok(inspect_game_room_record(
        docs_sync,
        blob_service,
        replica,
        subscription_topic_id,
        record,
        policy,
    )
    .await?
    .verified())
}

pub(crate) async fn inspect_game_room_record(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    record: &DocRecord,
    policy: DocFetchPolicy,
) -> Result<SessionRead<VerifiedGameRoom>> {
    let rejected = |reason| {
        warn_rejected_session(replica, record.key.as_str(), reason);
        Ok(SessionRead::Rejected)
    };
    let Ok(state) = serde_json::from_slice::<GameRoomStateDocV1>(&record.value) else {
        return rejected(SessionRejection::UnreadableState);
    };
    if session_id_from_key(record.key.as_str(), "sessions/game/") != Some(state.room_id.as_str()) {
        return rejected(SessionRejection::KeyMismatch);
    }
    let signed = load_signed_manifest::<GameRoomManifestBlobV1>(
        docs_sync,
        replica,
        &state.last_envelope_id,
        GAME_SESSION_ENVELOPE_KIND,
        policy,
    )
    .await?;
    let current_manifest = state.current_manifest.clone();
    match signed {
        SessionRead::Ready(signed) => {
            let relay = signed.relay(record, &current_manifest.mime);
            let mut verified = match VerifiedGameRoom::verify(
                state,
                Some((&signed.signer, signed.signed_at)),
                signed.manifest,
                replica,
                subscription_topic_id,
            ) {
                Ok(verified) => verified,
                Err(reason) => return rejected(reason),
            };
            if current_manifest.hash != signed.hash {
                return rejected(SessionRejection::ManifestMismatch);
            }
            verified.relay = Some(relay);
            learn_manifest_source(docs_sync, blob_service, &current_manifest).await;
            match read_session_blob::<GameRoomManifestBlobV1>(
                blob_service,
                &current_manifest,
                policy,
            )
            .await
            {
                Ok(Some(blob)) if blob == verified.manifest => Ok(SessionRead::Ready(verified)),
                Ok(Some(_)) | Err(_) => rejected(SessionRejection::ManifestMismatch),
                Ok(None) => Ok(SessionRead::MissingManifest(current_manifest.hash)),
            }
        }
        SessionRead::MissingDocs if state.room_id.starts_with("dome-") => {
            // 未署名 hash の取得を許す旧 Dome 互換例外。ID の整合は認証ではないが、
            // prefix だけ・別 scope の state を契機に remote 取得へ進めない(#1261)。
            let Some(scope) = ReplicaPostScope::for_replica(replica, subscription_topic_id) else {
                return rejected(SessionRejection::UnsupportedReplica);
            };
            if !scope.accepts(&state.topic_id, state.channel_id.as_ref()) {
                return rejected(SessionRejection::ScopeMismatch);
            }
            let context = match &state.channel_id {
                Some(channel_id) => SpatialContextV1::Channel {
                    topic_id: state.topic_id.clone(),
                    channel_id: channel_id.clone(),
                },
                None => SpatialContextV1::Topic {
                    topic_id: state.topic_id.clone(),
                },
            };
            if state.room_id != dome_instance_id(&context, &state.owner_pubkey) {
                return rejected(SessionRejection::IdNotBoundToOwner);
            }
            match read_session_blob::<GameRoomManifestBlobV1>(
                blob_service,
                &current_manifest,
                policy,
            )
            .await
            {
                Ok(Some(manifest)) => {
                    match VerifiedGameRoom::verify(
                        state,
                        None,
                        manifest,
                        replica,
                        subscription_topic_id,
                    ) {
                        Ok(verified) => Ok(SessionRead::Ready(verified)),
                        Err(reason) => rejected(reason),
                    }
                }
                Ok(None) => Ok(SessionRead::MissingManifest(current_manifest.hash)),
                Err(_) => rejected(SessionRejection::ManifestMismatch),
            }
        }
        SessionRead::MissingDocs => Ok(SessionRead::MissingDocs),
        _ => rejected(SessionRejection::SignedManifestUnavailable),
    }
}

/// game room を id で読む。同じ key の record は上限つきで読み、検証に通るもののうち最も新しい 1 件を使う。
pub(crate) async fn load_verified_game_room(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    replica: &ReplicaId,
    subscription_topic_id: &str,
    room_id: &str,
    policy: DocFetchPolicy,
) -> Result<Option<VerifiedGameRoom>> {
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            stable_key("sessions/game", &format!("{room_id}/state")).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    let mut newest: Option<VerifiedGameRoom> = None;
    for record in &records {
        if let Some(verified) = verify_game_room_record(
            docs_sync,
            blob_service,
            replica,
            subscription_topic_id,
            record,
            policy,
        )
        .await?
            && newest
                .as_ref()
                .is_none_or(|current| match verified.manifest.room_kind {
                    GameRoomKind::ScoreGame => verified.score_revision() > current.score_revision(),
                    GameRoomKind::MetaverseRoom => {
                        verified.state.updated_at > current.state.updated_at
                    }
                })
        {
            newest = Some(verified);
        }
    }
    Ok(newest)
}
