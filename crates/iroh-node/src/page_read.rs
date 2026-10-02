//! Demand-owned, bounded reads from an already local docs namespace.
//! Bounded bucket pages are served without starting docs sync. Private reads prove the epoch capability.

use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use iroh::EndpointAddr;
use iroh::endpoint::{Connection, Endpoint};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::api::Store as BlobStore;
use iroh_docs::actor::{OpenOpts, SyncHandle};
use iroh_docs::store::{Query, SortBy, SortDirection};
use iroh_docs::sync::SignedEntry;
use iroh_docs::{NamespaceId, NamespaceSecret};
use irpc::channel::mpsc;
use kukuri_core::ReplicaId;
use kukuri_store::ContentCacheStore;
use n0_future::time::timeout;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

pub const DOC_READ_ALPN: &[u8] = b"/kukuri/docs-read/1";

fn private_replica(replica: &str) -> bool {
    replica.starts_with("bucket::v1::channel::")
        || replica.starts_with("channel::")
        || replica.starts_with(kukuri_core::wire::ACCOUNT_SYNC_REPLICA_PREFIX)
}

/// 公開 topic の bucket(と旧形式)。保持している他の参加者の record も一覧で提供する(#1395)。
fn topic_replica(replica: &str) -> bool {
    replica.starts_with("bucket::v1::topic::") || replica.starts_with("topic::")
}

fn public_replica(replica: &str) -> bool {
    replica.starts_with("bucket::v1::topic::")
        || replica.starts_with("topic::")
        || replica.starts_with("bucket::v1::author::")
        || replica.starts_with("author::")
}
const DEADLINE: Duration = Duration::from_secs(30);
const MAX_REQUEST_BYTES: usize = 4 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_KEY_BYTES: usize = 2048;
const MAX_KEYS: usize = 512;
const MAX_RECORDS: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DocReadQuery {
    Keys {
        prefix: String,
        descending: bool,
        limit: usize,
        /// Only this docs author's entries fill and count toward the page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    Exact {
        key: String,
        limit: usize,
        author: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocReadKey {
    pub key: String,
    pub content_hash: String,
    pub content_len: u64,
    pub docs_author: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocReadRecord {
    pub key: String,
    #[serde(with = "base64_value")]
    pub value: Vec<u8>,
    pub content_hash: String,
    pub content_len: u64,
    pub docs_author: String,
}

mod base64_value {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let value = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DocReadResponse {
    Keys {
        entries: Vec<DocReadKey>,
        reached_limit: bool,
    },
    Records(Vec<DocReadRecord>),
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    version: u8,
    replica: String,
    namespace: String,
    query: DocReadQuery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capability_proof: Option<String>,
}

fn private_capability_proof(
    secret: &NamespaceSecret,
    replica: &str,
    namespace: &str,
    query: &DocReadQuery,
) -> Result<String> {
    let mut hasher = blake3::Hasher::new_keyed(&secret.to_bytes());
    hasher.update(b"kukuri:private-doc-read:v1\0");
    hasher.update(&serde_json::to_vec(&(replica, namespace, query))?);
    Ok(hasher.finalize().to_hex().to_string())
}

impl Request {
    pub(crate) fn new(
        replica: &str,
        secret: &NamespaceSecret,
        query: DocReadQuery,
    ) -> Result<Self> {
        let namespace = secret.id().to_string();
        let capability_proof = private_replica(replica)
            .then(|| private_capability_proof(secret, replica, &namespace, &query))
            .transpose()?;
        let request = Self {
            version: 1,
            replica: replica.to_string(),
            namespace,
            query,
            capability_proof,
        };
        request.check_budget()?;
        Ok(request)
    }

    fn check_budget(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported docs read version");
        let private = private_replica(&self.replica);
        ensure!(
            (private || public_replica(&self.replica)) && self.replica.len() <= MAX_KEY_BYTES,
            "only topic, author and private channel replicas are readable"
        );
        ensure!(
            self.capability_proof.is_some() == private,
            "private bucket capability proof is required"
        );
        match &self.query {
            DocReadQuery::Keys {
                prefix,
                limit,
                author,
                ..
            } => {
                ensure!(
                    (1..=MAX_KEYS).contains(limit),
                    "docs key page limit exceeded"
                );
                ensure!(prefix.len() <= MAX_KEY_BYTES, "docs key prefix too long");
                ensure!(
                    author.as_ref().is_none_or(|author| author.len() <= 128),
                    "docs author id too long"
                );
            }
            DocReadQuery::Exact { key, limit, author } => {
                ensure!(
                    (1..=MAX_RECORDS).contains(limit),
                    "docs exact limit exceeded"
                );
                ensure!(key.len() <= MAX_KEY_BYTES, "docs exact key too long");
                ensure!(
                    author.as_ref().is_none_or(|author| author.len() <= 128),
                    "docs author id too long"
                );
            }
        }
        Ok(())
    }
}

/// 手元の private の capability から replica の secret を引く。登録・削除と引き方は docs-sync が持ち、相手への
/// private の応答もこれで確かめる(参加中の channel の世代の秘密は store の行から引く。ADR 0061 §9)。
#[async_trait::async_trait]
pub trait PrivateSecretLookup: Send + Sync {
    async fn private_secret(&self, replica: &ReplicaId) -> Option<NamespaceSecret>;
}

#[derive(Clone, Default)]
pub(crate) struct PrivateCapabilities {
    pub(crate) lookup: Arc<OnceLock<Arc<dyn PrivateSecretLookup>>>,
}

#[derive(Clone)]
pub(crate) struct DocReadProtocol {
    sync: SyncHandle,
    blobs: BlobStore,
    remote_cache: Arc<OnceLock<Arc<dyn ContentCacheStore>>>,
    capabilities: PrivateCapabilities,
    permits: Arc<Semaphore>,
}

impl std::fmt::Debug for DocReadProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocReadProtocol").finish_non_exhaustive()
    }
}

impl DocReadProtocol {
    pub(crate) fn new(
        sync: SyncHandle,
        blobs: BlobStore,
        remote_cache: Arc<OnceLock<Arc<dyn ContentCacheStore>>>,
        capabilities: PrivateCapabilities,
    ) -> Self {
        Self {
            sync,
            blobs,
            remote_cache,
            capabilities,
            permits: Arc::new(Semaphore::new(8)),
        }
    }

    async fn local_entries(
        &self,
        namespace: NamespaceId,
        query: Query,
    ) -> Result<Vec<SignedEntry>> {
        let (reply, mut receiver) = mpsc::channel(8);
        self.sync.get_many(namespace, query, reply).await?;
        let mut entries = Vec::new();
        while let Some(entry) = receiver.recv().await? {
            entries.push(entry?);
        }
        Ok(entries)
    }

    async fn serve(&self, connection: &Connection) -> Result<()> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let bytes = recv.read_to_end(MAX_REQUEST_BYTES).await?;
        let request: Request = serde_json::from_slice(&bytes)?;
        request.check_budget()?;
        let namespace = NamespaceId::from_str(&request.namespace)?;
        // R5-H: 書き手の docs handle は上限(128)で閉じるので、提供する namespace は読む間だけ開く。
        // 手元に無い namespace は開けない。公開 topic の bucket では手元の entry を空とし、保持している record だけで
        // 答える(#1395)。それ以外(author の bucket 等)は、保持分が無ければ読取りの失敗にして、呼出元が次の provider
        // (書き手本人)へ進めるようにする。
        let opened = self.sync.open(namespace, OpenOpts::default()).await;
        let response = self.respond(request, namespace, opened.is_ok()).await;
        if opened.is_ok() {
            let _ = self.sync.close(namespace).await;
        }
        let bytes = serde_json::to_vec(&response?)?;
        ensure!(
            bytes.len() <= MAX_RESPONSE_BYTES,
            "docs response budget exceeded"
        );
        send.write_all(&bytes).await?;
        send.finish()?;
        send.stopped().await?;
        Ok(())
    }

    async fn respond(
        &self,
        request: Request,
        namespace: NamespaceId,
        opened: bool,
    ) -> Result<DocReadResponse> {
        if private_replica(&request.replica) {
            // 保持分は要求の replica の文字列で引くので、証明は docs の namespace ではなく、要求の replica に登録した
            // capability で確かめる(#1459)。
            let lookup = self
                .capabilities
                .lookup
                .get()
                .context("private replica capability is not registered")?;
            let secret = lookup
                .private_secret(&ReplicaId::new(request.replica.as_str()))
                .await
                .context("private replica capability is not registered")?;
            ensure!(
                secret.id() == namespace,
                "docs namespace is not the requested private replica"
            );
            let expected = private_capability_proof(
                &secret,
                &request.replica,
                &request.namespace,
                &request.query,
            )?;
            ensure!(
                request.capability_proof.as_deref() == Some(expected.as_str()),
                "private bucket capability proof did not match"
            );
        } else {
            let public_secret = NamespaceSecret::from_bytes(
                blake3::hash(format!("kukuri-docs:{}", request.replica).as_bytes()).as_bytes(),
            );
            ensure!(
                public_secret.id() == namespace,
                "docs namespace is not the requested public bucket"
            );
        }
        let response = match request.query {
            DocReadQuery::Keys {
                prefix,
                descending,
                limit,
                author,
            } => {
                let direction = if descending {
                    SortDirection::Desc
                } else {
                    SortDirection::Asc
                };
                let builder = match author.as_deref() {
                    Some(author) => Query::author(author.parse()?).key_prefix(&prefix),
                    None => Query::key_prefix(&prefix),
                };
                let stream = if opened {
                    self.local_entries(
                        namespace,
                        builder
                            .sort_by(SortBy::KeyAuthor, direction)
                            .limit((limit + 1) as u64)
                            .build(),
                    )
                    .await?
                } else {
                    Vec::new()
                };
                let mut reached_limit = stream.len() > limit;
                let mut entries = Vec::new();
                for entry in stream.into_iter().take(limit) {
                    let Ok(key) = String::from_utf8(entry.key().to_vec()) else {
                        continue;
                    };
                    if key.len() > MAX_KEY_BYTES {
                        continue;
                    }
                    entries.push(DocReadKey {
                        key,
                        content_hash: entry.content_hash().to_string(),
                        content_len: entry.content_len(),
                        docs_author: entry.author_bytes().to_string(),
                    });
                }
                // 保持分を合わせる。公開 topic の replica は保持分のすべて(#1395)、それ以外は自分の record だけ
                // (ADR 0058 §7。namespace の無い Web の reload・native の restore の後も一覧から消えない)。
                // 手元の先頭 `limit` 件と保持分の先頭 `limit` 件を合わせれば、和集合の先頭 `limit` 件が決まる。
                let topic = topic_replica(&request.replica);
                let mut held_any = false;
                if let Some(cache) = self.remote_cache.get() {
                    let (held, more) = cache
                        .remote_record_keys(
                            &request.replica,
                            &prefix,
                            descending,
                            author.as_deref(),
                            limit,
                            !topic,
                        )
                        .await?;
                    held_any = !held.is_empty();
                    let held = held
                        .into_iter()
                        .filter(|held| held.key.len() <= MAX_KEY_BYTES)
                        .map(|held| DocReadKey {
                            key: held.key,
                            content_hash: held.content_hash,
                            content_len: held.content_len,
                            docs_author: held.author,
                        });
                    let truncated;
                    (entries, truncated) = kukuri_store::merge_record_keys(
                        entries,
                        held,
                        |entry| (entry.key.as_str(), entry.docs_author.as_str()),
                        descending,
                        limit,
                    );
                    reached_limit |= more || truncated;
                }
                // 手元に無い namespace は、公開 topic の bucket か保持分があるときだけ答える。それ以外は読取りの失敗に
                // して、呼出元が次の provider(書き手本人)へ進めるようにする。
                ensure!(opened || topic || held_any, "docs namespace is not held");
                DocReadResponse::Keys {
                    entries,
                    reached_limit,
                }
            }
            DocReadQuery::Exact { key, limit, author } => {
                let cached = match self.remote_cache.get() {
                    Some(cache) => {
                        cache
                            .get_remote_records(
                                &request.replica,
                                &key,
                                author.as_deref(),
                                limit,
                                false,
                            )
                            .await?
                    }
                    None => Vec::new(),
                };
                let builder = match author.as_deref() {
                    Some(author) => Query::author(author.parse()?).key_exact(&key),
                    None => Query::key_exact(&key),
                };
                let stream = if opened {
                    self.local_entries(
                        namespace,
                        builder
                            .sort_by(SortBy::KeyAuthor, SortDirection::Asc)
                            .limit(limit as u64)
                            .build(),
                    )
                    .await?
                } else {
                    ensure!(
                        topic_replica(&request.replica) || !cached.is_empty(),
                        "docs namespace is not held"
                    );
                    Vec::new()
                };
                let mut records = Vec::new();
                for entry in stream {
                    ensure!(
                        entry.content_len() <= MAX_RECORD_BYTES as u64,
                        "docs record too large"
                    );
                    let blobs = self.blobs.clone();
                    let hash = entry.content_hash();
                    let value = crate::confine_local(async move {
                        let mut value = Vec::new();
                        blobs
                            .blobs()
                            .reader(hash)
                            .take((MAX_RECORD_BYTES + 1) as u64)
                            .read_to_end(&mut value)
                            .await
                            .map(|_| value)
                    })
                    .await?;
                    ensure!(value.len() <= MAX_RECORD_BYTES, "docs record too large");
                    records.push(DocReadRecord {
                        key: String::from_utf8(entry.key().to_vec())?,
                        value,
                        content_hash: entry.content_hash().to_string(),
                        content_len: entry.content_len(),
                        docs_author: entry.author_bytes().to_string(),
                    });
                }
                if records.len() < limit {
                    for bytes in cached.into_iter().take(limit - records.len()) {
                        let record: DocReadRecord = serde_json::from_slice(&bytes)?;
                        ensure!(
                            record.key == key
                                && record.value.len() <= MAX_RECORD_BYTES
                                && author
                                    .as_ref()
                                    .is_none_or(|author| record.docs_author == author.as_str())
                                && record.content_len == record.value.len() as u64
                                && iroh_blobs::Hash::new(&record.value).to_string()
                                    == record.content_hash,
                            "cached docs record is invalid"
                        );
                        if !records
                            .iter()
                            .any(|existing| existing.docs_author == record.docs_author)
                        {
                            records.push(record);
                        }
                    }
                }
                DocReadResponse::Records(records)
            }
        };
        Ok(response)
    }
}

impl ProtocolHandler for DocReadProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let Ok(_permit) = self.permits.try_acquire() else {
            return Ok(());
        };
        timeout(DEADLINE, self.serve(&connection))
            .await
            .context("docs read request timed out")
            .and_then(|result| result)
            .map_err(|error| AcceptError::from_boxed(error.into_boxed_dyn_error()))
    }
}

pub(crate) async fn fetch(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    request: Request,
) -> Result<DocReadResponse> {
    let connection = endpoint.connect(peer, DOC_READ_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    let bytes = serde_json::to_vec(&request)?;
    ensure!(
        bytes.len() <= MAX_REQUEST_BYTES,
        "docs request budget exceeded"
    );
    send.write_all(&bytes).await?;
    send.finish()?;
    let bytes = recv.read_to_end(MAX_RESPONSE_BYTES).await?;
    connection.close(0u32.into(), b"docs read complete");
    Ok(serde_json::from_slice(&bytes)?)
}
