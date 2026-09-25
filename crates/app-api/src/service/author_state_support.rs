//! author replica の状態(profile・follow・block)の反映(#1239)。replica は走査しない。
//!
//! docs の event はその key だけを反映する。購読の開始時(最初の表示)は、`profile/latest`、自分を指す follow・block の key、
//! follow・block の key の上限つきの一覧から反映する。取りこぼし・同期の区切りの後に読み直すのは、自分を指す follow・block の
//! key(相互 follow の判定と DM に要る)だけで、それ以外の取りこぼしは埋めない(AGENTS.md: ユースケース上ユーザーが必要としない
//! 限り同期・復旧はしない)。
//!
//! #1221 R5-C: 手元に無い現在値の key(profile と自分を指す follow・block、event の key)は、author 本人を含む
//! 有界な provider から key 指定で読む。remote の読取りのために namespace を import・sync しない。他の相手を指す edge の
//! 窓は手元だけを読む。

use super::remote_read_support::{REMOTE_READ_DEADLINE, writer_readers};
use super::*;
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};

/// author replica の follow・block の edge を、購読の開始時に読む key の数の上限。
///
/// author replica は、その author の follow・block の数だけ key を持つ。全件は読まない。上限を超える edge は、
/// その key の docs の event が届いたときに反映する。関係の再計算に要る自分を指す key は、この上限とは別に必ず読む。
pub(crate) const AUTHOR_EDGE_KEYS: usize = 512;
/// 同じ key に docs author ごとの record がありうるので、1 つの key で調べる record の数の上限。
const AUTHOR_RECORDS_PER_KEY: usize = MAX_ENVELOPE_RECORDS_PER_OBJECT;
/// author replica の反映の結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AuthorHydration {
    /// 読めて検証に通った key の数。
    pub(crate) reflected: usize,
    /// そのうち、手元に無かった envelope の数(関係の再計算と、追いつきの間隔に使う)。
    pub(crate) changed: usize,
}

impl AuthorHydration {
    fn reflected(changed: bool) -> Self {
        Self {
            reflected: 1,
            changed: usize::from(changed),
        }
    }

    fn add(&mut self, other: Self) {
        self.reflected += other.reflected;
        self.changed += other.changed;
    }
}

/// 購読の開始時に手元から読む author replica の key。`graph/follows/`・`graph/blocks/` の key の上限つきの一覧
/// (それぞれ `AUTHOR_EDGE_KEYS` 件)で、自分を指す 2 key は除く(別に先に読む)。値は読まない。
/// 同じ key は 1 回だけ返す(一覧は docs author ごとの entry を返す)。
async fn author_state_keys(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    local_author_pubkey: &str,
    docs_author: Option<&str>,
) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut seen = self_edge_keys(local_author_pubkey)
        .into_iter()
        .collect::<BTreeSet<_>>();
    for prefix in ["graph/follows/", "graph/blocks/"] {
        let query = DocKeyQuery {
            prefix: prefix.to_string(),
            order: DocKeyOrder::Ascending,
            limit: AUTHOR_EDGE_KEYS,
        };
        // 著者の docs author が分かれば、その名義の key だけで窓を作る(他の名義の key で窓を埋められない)。
        let page = match docs_author {
            Some(docs_author) => {
                docs_sync
                    .query_replica_keys_by_author(replica, docs_author, query)
                    .await?
            }
            None => docs_sync.query_replica_keys(replica, query).await?,
        };
        for entry in page.entries {
            if seen.insert(entry.key.clone()) {
                keys.push(entry.key);
            }
        }
    }
    Ok(keys)
}

fn self_edge_keys(local_author_pubkey: &str) -> [String; 2] {
    [
        stable_key("graph/follows", local_author_pubkey),
        stable_key("graph/blocks", local_author_pubkey),
    ]
}

/// author replica の現在値の key を、手元の次に author 本人を含む有界な provider から読む(#1221 R5-C)。
///
/// 手元で読めない key だけを remote から読む。provider はその最初の key で一度だけ選び、同じ操作の後続の key で使い回す。
/// `LocalOnly` と自分の replica は手元だけを読む。remote の読取りは namespace を import・sync しない。
struct AuthorKeyReader<'a> {
    services: &'a ServiceHandles,
    local_author_pubkey: &'a str,
    author_pubkey: &'a str,
    replica: ReplicaId,
    docs_author: Option<String>,
    remote: Option<Vec<Arc<dyn DocsSync>>>,
    deadline: Option<tokio::time::Instant>,
}

impl<'a> AuthorKeyReader<'a> {
    async fn new(
        services: &'a ServiceHandles,
        local_author_pubkey: &'a str,
        author_pubkey: &'a str,
        policy: DocFetchPolicy,
    ) -> Result<Self> {
        let remote =
            policy == DocFetchPolicy::LocalThenRemote && author_pubkey != local_author_pubkey;
        Ok(Self {
            services,
            local_author_pubkey,
            author_pubkey,
            replica: author_replica_id(author_pubkey),
            docs_author: known_docs_author(services, local_author_pubkey, author_pubkey).await?,
            remote: (!remote).then(Vec::new),
            deadline: None,
        })
    }

    /// profile の envelope の tag から覚えた docs author を、同じ回の残りの key の読み出しに使う。
    async fn refresh_docs_author(&mut self) -> Result<()> {
        if self.docs_author.is_none() {
            self.docs_author =
                known_docs_author(self.services, self.local_author_pubkey, self.author_pubkey)
                    .await?;
        }
        Ok(())
    }

    async fn read(&mut self, key: &str) -> Result<AuthorHydration> {
        let local = hydrate_author_record(
            self.services,
            self.author_pubkey,
            &self.replica,
            key,
            self.docs_author.as_deref(),
            DocFetchPolicy::LocalOnly,
        )
        .await?;
        if local.reflected > 0 || AuthorKeyKind::of(key).is_none() {
            return Ok(local);
        }
        if self.remote.is_none() {
            self.deadline = Some(tokio::time::Instant::now() + REMOTE_READ_DEADLINE);
            self.remote = Some(
                writer_readers(self.services, &self.replica, &[self.author_pubkey], None).await,
            );
        }
        let deadline = self.deadline.unwrap_or_else(tokio::time::Instant::now);
        for reader in self.remote.iter().flatten() {
            let mut services = self.services.clone();
            services.docs_sync = reader.clone();
            let read = tokio::time::timeout_at(
                deadline,
                hydrate_author_record(
                    &services,
                    self.author_pubkey,
                    &self.replica,
                    key,
                    self.docs_author.as_deref(),
                    DocFetchPolicy::LocalThenRemote,
                ),
            )
            .await;
            reader.finish_remote_object().await;
            match read {
                Ok(Ok(outcome)) if outcome.reflected > 0 => return Ok(outcome),
                Ok(Ok(_)) => {}
                Ok(Err(error)) => warn!(%error, key, "author record read from a provider failed"),
                Err(_) => break,
            }
        }
        Ok(AuthorHydration::default())
    }
}

async fn hydrate_author_keys(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let mut reader =
        AuthorKeyReader::new(services, local_author_pubkey, author_pubkey, policy).await?;
    // profile を先に読む。docs author がまだ分からない著者でも、profile の envelope の tag から覚えれば、
    // 同じ回の follow・block の窓と読み出しから docs author と key の組で読める。
    let mut outcome = reader
        .read(stable_key("profile", "latest").as_str())
        .await?;
    reader.refresh_docs_author().await?;
    for key in self_edge_keys(local_author_pubkey) {
        outcome.add(reader.read(&key).await?);
    }
    // 他の相手を指す edge の窓は手元だけを読む。remote の全 edge の復元はしない(グラフの全件復元を要求しない)。
    for key in author_state_keys(
        services.docs_sync.as_ref(),
        &reader.replica,
        local_author_pubkey,
        reader.docs_author.as_deref(),
    )
    .await?
    {
        outcome.add(
            hydrate_author_record(
                services,
                author_pubkey,
                &reader.replica,
                &key,
                reader.docs_author.as_deref(),
                DocFetchPolicy::LocalOnly,
            )
            .await?,
        );
    }
    Ok(outcome)
}

/// author の docs author の id(ADR 0053 §6)。自分は手元の値。相手は、署名つきの envelope の tag から覚えた値
/// (`hydrate_author_record` が覚える)。分からなければ `None`。
pub(crate) async fn known_docs_author(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
) -> Result<Option<String>> {
    if author_pubkey == local_author_pubkey {
        return services.docs_sync.local_docs_author().await;
    }
    services
        .projection_store
        .get_author_docs_author(author_pubkey)
        .await
}

/// author の状態(profile・follow・block)を、上限つきで反映する。replica は走査しない。
///
/// 購読の開始時(最初の表示)に使う。関係は読むときに edge から求める(#1221 R4-D)ので、ここでは再計算しない。
/// 戻り値は読めた key の数。
pub(crate) async fn hydrate_author_state(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<usize> {
    Ok(
        hydrate_author_keys(services, local_author_pubkey, author_pubkey, policy)
            .await?
            .reflected,
    )
}

/// 同期の区切りと取りこぼしの後の追いつき。読むのは、自分を指す follow・block の key(`graph/follows/<自分>`・
/// `graph/blocks/<自分>`)だけ。相互 follow の判定と DM に要るので、取りこぼしても読み直す。それ以外の key の取りこぼしは
/// 埋めない。
pub(crate) async fn catch_up_author_state(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let mut reader =
        AuthorKeyReader::new(services, local_author_pubkey, author_pubkey, policy).await?;
    let mut outcome = AuthorHydration::default();
    for key in self_edge_keys(local_author_pubkey) {
        outcome.add(reader.read(&key).await?);
    }
    Ok(outcome)
}

/// docs の event が指す author replica の key を 1 つ反映する。
pub(crate) async fn hydrate_author_key(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    key: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    AuthorKeyReader::new(services, local_author_pubkey, author_pubkey, policy)
        .await?
        .read(key)
        .await
}

/// author replica の key の種類。
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthorKeyKind {
    Profile,
    Follow,
    Block,
}

impl AuthorKeyKind {
    fn of(key: &str) -> Option<Self> {
        if key == stable_key("profile", "latest") {
            Some(Self::Profile)
        } else if key.starts_with("graph/follows/") {
            Some(Self::Follow)
        } else if key.starts_with("graph/blocks/") {
            Some(Self::Block)
        } else {
            None
        }
    }
}

/// record 1 件を検証し、通れば、その record が指す envelope を返す。
async fn verified_author_envelope(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    author_pubkey: &str,
    kind: AuthorKeyKind,
    record: &DocRecord,
    docs_author: Option<&str>,
    policy: DocFetchPolicy,
) -> Result<Option<KukuriEnvelope>> {
    let decoded = match kind {
        AuthorKeyKind::Profile => {
            serde_json::from_slice::<AuthorProfileDocV1>(&record.value).map(|doc| {
                (doc.author_pubkey.as_str() == author_pubkey).then_some((doc.envelope_id, None))
            })
        }
        AuthorKeyKind::Follow => {
            serde_json::from_slice::<FollowEdgeDocV1>(&record.value).map(|doc| {
                (doc.subject_pubkey.as_str() == author_pubkey).then(|| {
                    (
                        doc.envelope_id,
                        Some((doc.target_pubkey, doc.status == FollowEdgeStatus::Active)),
                    )
                })
            })
        }
        AuthorKeyKind::Block => {
            serde_json::from_slice::<BlockEdgeDocV1>(&record.value).map(|doc| {
                (doc.subject_pubkey.as_str() == author_pubkey).then(|| {
                    (
                        doc.envelope_id,
                        Some((doc.target_pubkey, doc.status == BlockEdgeStatus::Active)),
                    )
                })
            })
        }
    };
    let (envelope_id, edge) = match decoded {
        Ok(Some(decoded)) => decoded,
        Ok(None) => {
            warn!(author_pubkey = %author_pubkey, key = %record.key, "ignoring author doc with mismatched author");
            return Ok(None);
        }
        Err(error) => {
            warn!(author_pubkey = %author_pubkey, key = %record.key, error = %error, "failed to decode author doc");
            return Ok(None);
        }
    };
    let Some(envelope) =
        fetch_author_envelope(docs_sync, replica, &envelope_id, docs_author, policy).await?
    else {
        return Ok(None);
    };
    if envelope.pubkey.as_str() != author_pubkey {
        return Ok(None);
    }
    let edge_matches = match (kind, edge) {
        (AuthorKeyKind::Profile, _) => matches!(parse_profile(&envelope), Ok(Some(_))),
        (AuthorKeyKind::Follow, Some((target, active))) => matches!(
            parse_follow_edge(&envelope),
            Ok(Some(parsed)) if parsed.target_pubkey == target
                && (parsed.status == FollowEdgeStatus::Active) == active
                && record.key == stable_key("graph/follows", target.as_str())
        ),
        (AuthorKeyKind::Block, Some((target, active))) => matches!(
            parse_block_edge(&envelope),
            Ok(Some(parsed)) if parsed.target_pubkey == target
                && (parsed.status == BlockEdgeStatus::Active) == active
                && record.key == stable_key("graph/blocks", target.as_str())
        ),
        _ => false,
    };
    Ok(edge_matches.then_some(envelope))
}

/// `profile/latest`・`graph/follows/<target>`・`graph/blocks/<target>` の key を 1 つ読んで反映する。
/// それ以外の key は何もしない。
///
/// 著者の docs author が分かっていれば、docs author と key の組で 1 件読む(ADR 0053 §6)。誰でも書ける replica でも、
/// 他の名義の record は何件あっても読まない。組の record が無い・検証に通らないとき(docs author を申告する前の旧 record、
/// 端末ごとの旧名義)と、docs author が分からないときは、上限つき(`AUTHOR_RECORDS_PER_KEY` 件)で調べ、
/// 検証に通ったものから最も新しい envelope を選ぶ(best effort)。
///
/// 反映した envelope が docs author を申告していれば(署名つきの tag)、その著者の docs author として覚える。
async fn hydrate_author_record(
    services: &ServiceHandles,
    author_pubkey: &str,
    replica: &ReplicaId,
    key: &str,
    docs_author: Option<&str>,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let Some(kind) = AuthorKeyKind::of(key) else {
        return Ok(AuthorHydration::default());
    };
    let docs_sync = services.docs_sync.as_ref();
    let mut newest: Option<KukuriEnvelope> = None;
    if let Some(docs_author) = docs_author
        && let Some(record) = docs_sync
            .query_replica_by_author(replica, docs_author, key, policy)
            .await?
    {
        newest = verified_author_envelope(
            docs_sync,
            replica,
            author_pubkey,
            kind,
            &record,
            Some(docs_author),
            policy,
        )
        .await?;
    }
    if newest.is_none() {
        let records = docs_sync
            .query_replica_exact_bounded(replica, key, AUTHOR_RECORDS_PER_KEY, policy)
            .await?;
        for record in &records {
            if let Some(envelope) = verified_author_envelope(
                docs_sync,
                replica,
                author_pubkey,
                kind,
                record,
                None,
                policy,
            )
            .await?
                && newest.as_ref().is_none_or(|current| {
                    (envelope.created_at, envelope.id.as_str())
                        > (current.created_at, current.id.as_str())
                })
            {
                newest = Some(envelope);
            }
        }
    }
    let Some(envelope) = newest else {
        return Ok(AuthorHydration::default());
    };
    if let Some(declared) = envelope.docs_author()
        && Some(declared) != docs_author
    {
        services
            .projection_store
            .put_author_docs_author(author_pubkey, declared)
            .await?;
    }
    let store = services.store.as_ref();
    // envelope が手元にあっても書き直す。`put_envelope` は envelope から follow・block・profile の行も作り直すので、
    // 行だけが失われた状態から戻せる
    // (envelope の有無だけで判断して書かないと、失われた行が二度と作られない。desktop の再起動の test で発生)。`changed` は関係の再計算の要否に使う。
    let changed = store.get_envelope(&envelope.id).await?.is_none();
    store.put_envelope(envelope.clone()).await?;
    if kind == AuthorKeyKind::Profile
        && let Ok(Some(profile)) = parse_profile(&envelope)
    {
        services
            .projection_store
            .upsert_profile_cache(profile)
            .await?;
    }
    Ok(AuthorHydration::reflected(changed))
}
