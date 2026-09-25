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
use iroh_docs::actor::SyncHandle;
use iroh_docs::store::{Query, SortBy, SortDirection};
use iroh_docs::sync::SignedEntry;
use iroh_docs::{NamespaceId, NamespaceSecret};
use irpc::channel::mpsc;
use kukuri_store::SqliteStore;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;
use tokio::time::timeout;

pub const DOC_READ_ALPN: &[u8] = b"/kukuri/docs-read/1";
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
        let capability_proof = replica
            .starts_with("bucket::v1::channel::")
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
        let private = self.replica.starts_with("bucket::v1::channel::");
        ensure!(
            (private || self.replica.starts_with("bucket::v1::topic::"))
                && self.replica.len() <= MAX_KEY_BYTES,
            "only topic and private channel buckets are readable"
        );
        ensure!(
            self.capability_proof.is_some() == private,
            "private bucket capability proof is required"
        );
        match &self.query {
            DocReadQuery::Keys { prefix, limit, .. } => {
                ensure!(
                    (1..=MAX_KEYS).contains(limit),
                    "docs key page limit exceeded"
                );
                ensure!(prefix.len() <= MAX_KEY_BYTES, "docs key prefix too long");
            }
            DocReadQuery::Exact { key, limit, author } => {
                ensure!(
                    (1..=MAX_RECORDS).contains(limit),
                    "docs exact limit exceeded"
                );
                ensure!(key.len() <= MAX_KEY_BYTES, "docs exact key too long");
                if let Some(author) = author {
                    ensure!(author.len() <= 128, "docs author id too long");
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct DocReadProtocol {
    sync: SyncHandle,
    blobs: BlobStore,
    remote_cache: Arc<OnceLock<Arc<SqliteStore>>>,
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
        remote_cache: Arc<OnceLock<Arc<SqliteStore>>>,
    ) -> Self {
        Self {
            sync,
            blobs,
            remote_cache,
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
        if request.replica.starts_with("bucket::v1::channel::") {
            let secret = self.sync.export_secret_key(namespace).await?;
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
            } => {
                let direction = if descending {
                    SortDirection::Desc
                } else {
                    SortDirection::Asc
                };
                let stream = self
                    .local_entries(
                        namespace,
                        Query::key_prefix(prefix)
                            .sort_by(SortBy::KeyAuthor, direction)
                            .limit((limit + 1) as u64)
                            .build(),
                    )
                    .await?;
                let reached_limit = stream.len() > limit;
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
                DocReadResponse::Keys {
                    entries,
                    reached_limit,
                }
            }
            DocReadQuery::Exact { key, limit, author } => {
                let cached = match self.remote_cache.get() {
                    Some(cache) => {
                        cache
                            .get_remote_records(&request.replica, &key, author.as_deref(), limit)
                            .await?
                    }
                    None => Vec::new(),
                };
                let builder = match author.as_deref() {
                    Some(author) => Query::author(author.parse()?).key_exact(&key),
                    None => Query::key_exact(&key),
                };
                let stream = self
                    .local_entries(
                        namespace,
                        builder
                            .sort_by(SortBy::KeyAuthor, SortDirection::Asc)
                            .limit(limit as u64)
                            .build(),
                    )
                    .await;
                let stream = match stream {
                    Ok(stream) => stream,
                    Err(_) if !cached.is_empty() => Vec::new(),
                    Err(error) => return Err(error),
                };
                let mut records = Vec::new();
                for entry in stream {
                    ensure!(
                        entry.content_len() <= MAX_RECORD_BYTES as u64,
                        "docs record too large"
                    );
                    let mut reader = self
                        .blobs
                        .blobs()
                        .reader(entry.content_hash())
                        .take((MAX_RECORD_BYTES + 1) as u64);
                    let mut value = Vec::new();
                    reader.read_to_end(&mut value).await?;
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
        let bytes = serde_json::to_vec(&response)?;
        ensure!(
            bytes.len() <= MAX_RESPONSE_BYTES,
            "docs response budget exceeded"
        );
        send.write_all(&bytes).await?;
        send.finish()?;
        send.stopped().await?;
        Ok(())
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
