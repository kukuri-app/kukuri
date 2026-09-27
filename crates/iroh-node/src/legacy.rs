//! 旧 iroh store(新旧の namespace・remote 内容が同居した保存領域)の退役(#1221 R5-I)。
//!
//! node は新しい root の store で動く。endpoint secret は旧 root から原子的に写し、endpoint ID を保つ。
//! 旧 store は node とは別の instance として開き、読むだけに使う(書き換えない)。移し終えたら file ごと消す。

use std::ops::Bound;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use iroh_blobs::store::fs::{FsStore, options::Options};
use iroh_docs::store::{Query, Store as DocsStore};
use iroh_docs::{Author, AuthorId, Capability, NamespaceId, NamespaceSecret};

use crate::node::ENDPOINT_SECRET_FILE_NAME;
use crate::{DocReadRecord, IrohDocsNode};

/// `root` に endpoint secret が無く `legacy_root` にあれば、同じ内容を `root` へ原子的に写す(一時 file へ書いてから
/// 名前を変える)。旧 root の file は残す。何度呼んでも同じ結果になる。
pub fn adopt_endpoint_secret(legacy_root: &Path, root: &Path) -> Result<()> {
    let target = root.join(ENDPOINT_SECRET_FILE_NAME);
    let source = legacy_root.join(ENDPOINT_SECRET_FILE_NAME);
    if target.exists() || !source.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(root)
        .with_context(|| format!("failed to create iroh store root {}", root.display()))?;
    let staging = root.join(format!("{ENDPOINT_SECRET_FILE_NAME}.tmp"));
    std::fs::copy(&source, &staging).with_context(|| {
        format!(
            "failed to copy endpoint secret from {} to {}",
            source.display(),
            staging.display()
        )
    })?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&staging)?
        .sync_all()?;
    std::fs::rename(&staging, &target).with_context(|| {
        format!(
            "failed to move endpoint secret into place at {}",
            target.display()
        )
    })
}

/// CN の data dir の直下にある旧 store(`docs.redb`・`blobs.db` と blob の file など)を、endpoint secret と新しい store の
/// `root` を残して `retiring` へ名前を変えて移す。endpoint secret は `root` へも写し、endpoint ID を保つ。旧 store が
/// 無ければ何もしない。途中で止まっても、残った分を次の起動で移す。
pub fn retire_legacy_layout(dir: &Path, root: &Path, retiring: &Path) -> Result<()> {
    adopt_endpoint_secret(dir, root)?;
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path == root || path == retiring || entry.file_name() == ENDPOINT_SECRET_FILE_NAME {
            continue;
        }
        std::fs::create_dir_all(retiring)?;
        std::fs::rename(&path, retiring.join(entry.file_name()))
            .with_context(|| format!("failed to move {} for retirement", path.display()))?;
    }
    Ok(())
}

/// `dir` の中を深さ優先で消していき、1 回に消す file と directory は `budget` 件まで。空になれば `dir` も消して
/// `true`。全 file の一覧を先に作らず、`read_dir` を少しずつ進める。途中で止まっても、残った分から続ける。
pub fn remove_dir_step(dir: &Path, budget: usize) -> Result<bool> {
    if !dir.exists() {
        return Ok(true);
    }
    let mut removed = 0;
    if remove_entries(dir, budget, &mut removed)? {
        std::fs::remove_dir(dir)
            .with_context(|| format!("failed to remove retired directory {}", dir.display()))?;
        return Ok(true);
    }
    Ok(false)
}

/// `dir` の中身を消す。上限に達する前に中身を消し終えたら `true`。
fn remove_entries(dir: &Path, budget: usize, removed: &mut usize) -> Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        if *removed >= budget {
            return Ok(false);
        }
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if !remove_entries(&path, budget, removed)? {
                return Ok(false);
            }
            std::fs::remove_dir(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
        *removed += 1;
    }
    Ok(true)
}

/// 旧 root の store を読むだけに開いたもの。
pub struct LegacyStore {
    docs: Arc<StdMutex<DocsStore>>,
    blobs: FsStore,
    /// この端末が旧 store で書込みに使った docs author(アカウントの docs author と、それより前の端末ごとの鍵)。id の順。
    authors: Arc<Vec<Author>>,
}

/// 旧 store の本人の entry 1 件。
struct OwnEntry {
    secret: NamespaceSecret,
    author: AuthorId,
    key: Vec<u8>,
    hash: iroh_blobs::Hash,
}

impl LegacyStore {
    /// `root` に旧 store(`docs.redb`)があるときだけ開く。
    pub async fn open(root: &Path) -> Result<Option<Self>> {
        let path = root.join("docs.redb");
        if !path.is_file() {
            return Ok(None);
        }
        let (docs, authors) = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut store = DocsStore::persistent(&path)?;
            let mut authors = store.list_authors()?.collect::<Result<Vec<_>>>()?;
            authors.sort_by_key(|author| *author.id().as_bytes());
            Ok((store, authors))
        })
        .await??;
        let blobs = FsStore::load_with_opts(root.join("blobs.db"), Options::new(root))
            .await
            .with_context(|| format!("failed to load legacy blob store at {}", root.display()))?;
        Ok(Some(Self {
            docs: Arc::new(StdMutex::new(docs)),
            blobs,
            authors: Arc::new(authors),
        }))
    }

    /// 旧 store の file を閉じる(名前を変える前に呼ぶ)。
    pub async fn close(self) -> Result<()> {
        self.blobs.shutdown().await?;
        drop(self.docs);
        Ok(())
    }

    /// `namespace` の `key` の record(8 件まで)。content が旧 store に無いものは返さない。
    pub async fn records(&self, namespace: NamespaceId, key: &str) -> Result<Vec<DocReadRecord>> {
        let docs = self.docs.clone();
        let query = Query::key_exact(key).limit(8).build();
        let entries = tokio::task::spawn_blocking(move || -> Result<Vec<_>> {
            let mut store = docs.lock().expect("legacy docs store poisoned");
            store.get_many(namespace, query)?.collect()
        })
        .await??;
        let mut records = Vec::new();
        for entry in entries {
            let Ok(key) = String::from_utf8(entry.key().to_vec()) else {
                continue;
            };
            let Ok(value) = self.blobs.blobs().get_bytes(entry.content_hash()).await else {
                continue;
            };
            records.push(DocReadRecord {
                key,
                value: value.to_vec(),
                content_hash: entry.content_hash().to_string(),
                content_len: entry.content_len(),
                docs_author: entry.author().to_string(),
            });
        }
        Ok(records)
    }

    pub async fn read_blob(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let hash = hash.parse::<iroh_blobs::Hash>()?;
        Ok(self
            .blobs
            .blobs()
            .get_bytes(hash)
            .await
            .ok()
            .map(|bytes| bytes.to_vec()))
    }

    /// 旧 blob store の bytes を file へ写し、BLAKE3 を照合する。旧 store に無ければ `None`。
    pub async fn export_blob(&self, hash: &str, path: &Path) -> Result<Option<u64>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let hash = hash.parse::<iroh_blobs::Hash>()?;
        if !self.blobs.blobs().has(hash).await? {
            return Ok(None);
        }
        let mut reader = self.blobs.blobs().reader(hash);
        let mut file = tokio::fs::File::create(path).await?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0; 64 * 1024];
        let mut total = 0u64;
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read]).await?;
            total += read as u64;
        }
        file.flush().await?;
        anyhow::ensure!(
            hasher.finalize().as_bytes() == hash.as_bytes(),
            "legacy blob hash mismatch"
        );
        Ok(Some(total))
    }

    /// `prefix` の tag を名前順に、`after` の後ろから `limit` 件だけ返す。
    pub async fn tags(&self, prefix: &str, after: &str, limit: usize) -> Result<Vec<String>> {
        let start = if after.is_empty() {
            Bound::Included(prefix.as_bytes().to_vec())
        } else {
            Bound::Excluded(after.as_bytes().to_vec())
        };
        let mut end = prefix.as_bytes().to_vec();
        if let Some(last) = end.last_mut() {
            *last += 1;
        }
        let stream = self
            .blobs
            .tags()
            .list_range::<_, Vec<u8>>((start, Bound::Excluded(end)))
            .await?;
        let tags = stream.take(limit).collect::<Vec<_>>().await;
        tags.into_iter()
            .map(|tag| Ok(String::from_utf8(tag?.name.0.to_vec())?))
            .collect()
    }

    /// `prefix` の tag を `after` の後ろから `limit` 件、blob ごと node の store へ写す。戻り値は次の位置と終端か。
    pub async fn copy_tags(
        &self,
        node: &IrohDocsNode,
        prefix: &str,
        after: &str,
        limit: usize,
        staging: &Path,
    ) -> Result<(String, bool)> {
        let tags = self.tags(prefix, after, limit).await?;
        for tag in &tags {
            let hash = tag
                .strip_prefix(prefix)
                .unwrap_or_default()
                .parse::<iroh_blobs::Hash>()?;
            if !node.blobs().blobs().has(hash).await? {
                if self
                    .export_blob(&hash.to_string(), staging)
                    .await?
                    .is_none()
                {
                    continue;
                }
                let added = node.blobs().blobs().add_path(staging).await;
                let _ = tokio::fs::remove_file(staging).await;
                anyhow::ensure!(added?.hash == hash, "copied legacy tag hash mismatch");
            }
            node.blobs()
                .tags()
                .set(tag.as_bytes().to_vec(), hash)
                .await?;
        }
        let done = tags.len() < limit;
        Ok((
            tags.last().cloned().unwrap_or_else(|| after.to_string()),
            done,
        ))
    }

    /// 旧 store の本人の entry(この端末の docs author が書いたもの)を、位置 `cursor` の後ろから node の store へ写す。
    /// 1 回に読む entry は `limit` 件まで(他の docs author の entry は namespace の中で読み飛ばす)。
    /// 新しい store に同じ key が既にあれば(切替の後の書込み)写さない。戻り値は次の位置と終端か。
    pub async fn copy_own_entries(
        &self,
        node: &IrohDocsNode,
        cursor: &str,
        limit: usize,
    ) -> Result<(String, bool)> {
        let docs = self.docs.clone();
        let authors = self.authors.clone();
        let position = cursor.to_string();
        let (entries, next, done) = tokio::task::spawn_blocking(move || {
            let mut store = docs.lock().expect("legacy docs store poisoned");
            own_entries_page(&mut store, &authors, &position, limit)
        })
        .await??;
        for author in self.authors.iter() {
            node.docs().author_import(author.clone()).await?;
        }
        let mut open: Option<(NamespaceId, iroh_docs::api::Doc)> = None;
        for entry in entries {
            let namespace = entry.secret.id();
            if open.as_ref().map(|(id, _)| *id) != Some(namespace) {
                if let Some((_, doc)) = open.take() {
                    doc.close().await?;
                }
                let doc = node
                    .docs()
                    .import_namespace(Capability::Write(entry.secret))
                    .await?;
                open = Some((namespace, doc));
            }
            let (_, doc) = open.as_ref().expect("opened namespace");
            let existing = doc
                .get_many(Query::key_exact(&entry.key).limit(1).build())
                .await?;
            tokio::pin!(existing);
            if existing.next().await.is_some() {
                continue;
            }
            let Ok(value) = self.blobs.blobs().get_bytes(entry.hash).await else {
                continue;
            };
            doc.set_bytes(entry.author, entry.key, value).await?;
        }
        if let Some((_, doc)) = open {
            doc.close().await?;
        }
        Ok((next, done))
    }
}

type RecordKey<'a> = (&'a [u8; 32], &'a [u8; 32], &'a [u8]);

/// records の表の位置。`inclusive` ならその key から、でなければその key の次から読む。
struct Position {
    inclusive: bool,
    namespace: [u8; 32],
    author: [u8; 32],
    key: Vec<u8>,
}

impl Position {
    /// `i` か `e` と、namespace・docs author・key の hex。空は先頭。
    fn parse(cursor: &str) -> Result<Self> {
        let Some((mode, rest)) = cursor.split_at_checked(1) else {
            return Ok(Self::start_of([0; 32], [0; 32]));
        };
        let bytes = hex::decode(rest)?;
        anyhow::ensure!(bytes.len() >= 64, "invalid legacy store position");
        Ok(Self {
            inclusive: mode == "i",
            namespace: bytes[..32].try_into()?,
            author: bytes[32..64].try_into()?,
            key: bytes[64..].to_vec(),
        })
    }

    fn start_of(namespace: [u8; 32], author: [u8; 32]) -> Self {
        Self {
            inclusive: true,
            namespace,
            author,
            key: Vec::new(),
        }
    }

    fn encode(&self) -> String {
        format!(
            "{}{}{}{}",
            if self.inclusive { "i" } else { "e" },
            hex::encode(self.namespace),
            hex::encode(self.author),
            hex::encode(&self.key)
        )
    }
}

/// 位置から records の表を key の順に読み、本人の docs author の entry を `limit` 件まで集める。本人でない docs author と
/// 書込みのできない namespace は、次の本人の docs author・次の namespace へ飛ぶ(飛ぶのも 1 件と数える)。
fn own_entries_page(
    store: &mut DocsStore,
    authors: &[Author],
    cursor: &str,
    limit: usize,
) -> Result<(Vec<OwnEntry>, String, bool)> {
    let tables = store.snapshot_owned()?;
    let mut position = Position::parse(cursor)?;
    let mut entries = Vec::new();
    for _ in 0..limit {
        let start: RecordKey<'_> = (&position.namespace, &position.author, &position.key);
        let lower = if position.inclusive {
            Bound::Included(start)
        } else {
            Bound::Excluded(start)
        };
        let Some(item) = tables
            .records
            .range::<RecordKey<'_>>((lower, Bound::Unbounded))?
            .next()
        else {
            return Ok((entries, String::new(), true));
        };
        let (id, value) = item?;
        let (namespace, author, key) = id.value();
        let (namespace, author, key) = (*namespace, *author, key.to_vec());
        let (_, _, _, len, hash) = value.value();
        let secret = match tables.namespaces.get(&namespace)? {
            Some(capability) if capability.value().0 == 1 => {
                Some(NamespaceSecret::from_bytes(capability.value().1))
            }
            _ => None,
        };
        let own = authors.binary_search_by(|candidate| candidate.id().as_bytes().cmp(&author));
        position = match (secret, own) {
            (Some(secret), Ok(index)) => {
                if len > 0 {
                    entries.push(OwnEntry {
                        secret,
                        author: authors[index].id(),
                        key: key.clone(),
                        hash: (*hash).into(),
                    });
                }
                Position {
                    inclusive: false,
                    namespace,
                    author,
                    key,
                }
            }
            (Some(_), Err(next)) if next < authors.len() => {
                Position::start_of(namespace, *authors[next].id().as_bytes())
            }
            _ => {
                let Some(next) = next_namespace(&namespace) else {
                    return Ok((entries, String::new(), true));
                };
                Position::start_of(next, [0; 32])
            }
        };
    }
    Ok((entries, position.encode(), false))
}

fn next_namespace(namespace: &[u8; 32]) -> Option<[u8; 32]> {
    let mut next = *namespace;
    for byte in next.iter_mut().rev() {
        if *byte == u8::MAX {
            *byte = 0;
        } else {
            *byte += 1;
            return Some(next);
        }
    }
    None
}
