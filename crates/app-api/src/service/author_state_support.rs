//! author replica の状態(profile・follow・block)の反映(#1239)。replica は走査しない。
//!
//! docs の event はその key だけを反映する。起動時と追いつきは、`profile/latest`、自分を指す follow・block の key、
//! follow・block の key の上限つきの一覧から反映する。自分の replica の edge は、背景で小分けにすべて読む。

use super::*;
use kukuri_docs_sync::{DocKeyEntry, DocKeyOrder, DocKeyQuery};

/// author replica の follow・block の edge を、起動時と追いつきで読む key の数の上限。
///
/// author replica は、その author の follow・block の数だけ key を持つ。全件は読まない。上限を超える edge は、
/// その key の docs の event が届いたときに反映する(best effort)。関係の再計算に要る自分を指す key は、
/// この上限とは別に必ず読む。
pub(crate) const AUTHOR_EDGE_KEYS: usize = 512;
/// 同じ key に docs author ごとの record がありうるので、1 つの key で調べる record の数の上限。
const AUTHOR_RECORDS_PER_KEY: usize = MAX_ENVELOPE_RECORDS_PER_OBJECT;
/// 自分の replica の edge を小分けに読むときの、1 回の key の一覧の件数。
const OWN_EDGE_BATCH: usize = 256;
/// 自分の replica の edge を小分けに読むときの、key の一覧の query の数の上限(形の違う key で分割が膨らむのを止める)。
const OWN_EDGE_MAX_QUERIES: usize = 4_096;
/// 読み出しを読み終えた印。
pub(crate) const CHECKPOINT_DONE: &str = "done";
/// 最初から読み直す印。
pub(crate) const CHECKPOINT_RESTART: &str = "restart";
/// 読み終えた最後の桶の前に付ける印。
pub(crate) const CHECKPOINT_AFTER: &str = "after:";

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

/// `prefix` の key を、`batch` 件ずつの key の一覧で読む。一覧が上限で打ち切られたら、次の桁(16 進)で分けて読み直す。
/// key の後ろは 64 桁の 16 進(pubkey・object id)を前提にする。16 進でない key は、分けた後は読まない(best effort)。
///
/// 桶(key の prefix)は昇順に読む。読み終えた最後の桶(`last_bucket`)を保存しておき、次に `done_through` として渡すと、
/// その桶までを読まずに続きから読む(途中で止まっても再開できる)。
pub(crate) struct HexBucketedKeys {
    root_len: usize,
    pending: Vec<String>,
    batch: usize,
    queries_left: usize,
    done_through: Option<String>,
    last_bucket: Option<String>,
    stopped_early: bool,
}

impl HexBucketedKeys {
    pub(crate) fn new(
        prefix: &str,
        batch: usize,
        max_queries: usize,
        done_through: Option<String>,
    ) -> Self {
        Self {
            root_len: prefix.len(),
            pending: vec![prefix.to_string()],
            batch,
            queries_left: max_queries,
            done_through,
            last_bucket: None,
            stopped_early: false,
        }
    }

    fn split(&mut self, bucket: &str) {
        self.pending.extend(
            "fedcba9876543210"
                .chars()
                .map(|digit| format!("{bucket}{digit}")),
        );
    }

    /// query の数の上限で、読み残しを残して止まったか。
    pub(crate) fn stopped_early(&self) -> bool {
        self.stopped_early
    }

    /// 最後に返した batch の桶。
    pub(crate) fn last_bucket(&self) -> Option<&str> {
        self.last_bucket.as_deref()
    }

    /// 次の batch。尽きたか、query の数の上限に達したら `None`。
    pub(crate) async fn next_batch(
        &mut self,
        docs_sync: &dyn DocsSync,
        replica: &ReplicaId,
    ) -> Result<Option<Vec<DocKeyEntry>>> {
        while let Some(bucket) = self.pending.pop() {
            if let Some(done) = self.done_through.clone() {
                if done.len() > bucket.len() && done.starts_with(bucket.as_str()) {
                    // 前回、読み終えた桶の祖先。前回は分けて読んだので、読まずに分ける。
                    self.split(&bucket);
                    continue;
                }
                if bucket.as_str() <= done.as_str() {
                    // 前回までに読み終えた範囲(互いに接頭辞でない桶は、文字列の順が key の範囲の順になる)。
                    continue;
                }
            }
            if self.queries_left == 0 {
                self.pending.push(bucket);
                self.stopped_early = true;
                return Ok(None);
            }
            self.queries_left -= 1;
            let query = DocKeyQuery {
                prefix: bucket.clone(),
                order: DocKeyOrder::Ascending,
                limit: self.batch,
            };
            let page = docs_sync.query_replica_keys(replica, query).await?;
            if page.reached_limit && bucket.len() < self.root_len + 64 {
                self.split(&bucket);
                continue;
            }
            self.last_bucket = Some(bucket);
            return Ok(Some(page.entries));
        }
        Ok(None)
    }
}

/// 起動時と追いつきで読む author replica の key(`profile/latest` は別に先に読む)。自分を指す follow・block の key、
/// `graph/follows/`・`graph/blocks/` の key の上限つきの一覧(それぞれ `AUTHOR_EDGE_KEYS` 件)。値は読まない。
/// 同じ key は 1 回だけ返す(一覧は docs author ごとの entry を返す)。
async fn author_state_keys(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    local_author_pubkey: &str,
    docs_author: Option<&str>,
) -> Result<Vec<String>> {
    let mut keys = vec![
        stable_key("graph/follows", local_author_pubkey),
        stable_key("graph/blocks", local_author_pubkey),
    ];
    let mut seen = keys.iter().cloned().collect::<BTreeSet<_>>();
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

async fn hydrate_author_keys(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let replica = author_replica_id(author_pubkey);
    let mut docs_author = known_docs_author(services, local_author_pubkey, author_pubkey).await?;
    // profile を先に読む。docs author がまだ分からない著者でも、profile の envelope の tag から覚えれば、
    // 同じ回の follow・block の窓と読み出しから docs author と key の組で読める。
    let mut outcome = hydrate_author_record(
        services,
        author_pubkey,
        &replica,
        stable_key("profile", "latest").as_str(),
        docs_author.as_deref(),
        policy,
    )
    .await?;
    if docs_author.is_none() {
        docs_author = known_docs_author(services, local_author_pubkey, author_pubkey).await?;
    }
    for key in author_state_keys(
        services.docs_sync.as_ref(),
        &replica,
        local_author_pubkey,
        docs_author.as_deref(),
    )
    .await?
    {
        outcome.add(
            hydrate_author_record(
                services,
                author_pubkey,
                &replica,
                &key,
                docs_author.as_deref(),
                policy,
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

async fn rebuild_relationships(services: &ServiceHandles, local_author_pubkey: &str) -> Result<()> {
    rebuild_author_relationships(
        services.store.as_ref(),
        services.projection_store.as_ref(),
        local_author_pubkey,
    )
    .await
}

/// author の状態(profile・follow・block)を、上限つきで反映する。replica は走査しない。
///
/// 起動時と復旧で使う。関係は、反映の有無にかかわらず再計算する(起動時に、手元の edge から関係を作り直す)。
/// 戻り値は読めた key の数。
pub(crate) async fn hydrate_author_state(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<usize> {
    let outcome = hydrate_author_keys(services, local_author_pubkey, author_pubkey, policy).await?;
    rebuild_relationships(services, local_author_pubkey).await?;
    Ok(outcome.reflected)
}

/// 同期の区切りと取りこぼしの後の追いつき。読む範囲は `hydrate_author_state` と同じ。関係の再計算は、
/// 手元に無かった envelope が入ったときだけ行う。
pub(crate) async fn catch_up_author_state(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let outcome = hydrate_author_keys(services, local_author_pubkey, author_pubkey, policy).await?;
    if outcome.changed > 0 {
        rebuild_relationships(services, local_author_pubkey).await?;
    }
    Ok(outcome)
}

/// docs の event が指す author replica の key を 1 つ反映する。関係の再計算は、手元に無かった envelope が
/// 入ったときだけ行う。
pub(crate) async fn hydrate_author_key(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    author_pubkey: &str,
    key: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let replica = author_replica_id(author_pubkey);
    let docs_author = known_docs_author(services, local_author_pubkey, author_pubkey).await?;
    let outcome = hydrate_author_record(
        services,
        author_pubkey,
        &replica,
        key,
        docs_author.as_deref(),
        policy,
    )
    .await?;
    if outcome.changed > 0 {
        rebuild_relationships(services, local_author_pubkey).await?;
    }
    Ok(outcome)
}

/// 自分の replica の follow・block の edge を、背景で小分けにすべて読む。新しい端末で、自分の follow が
/// `AUTHOR_EDGE_KEYS` 件を超えていても、自分の follow の一覧が欠けないようにする。
///
/// 読み終えた桶の位置を store に残し(`sync_checkpoints`)、止まっても続きから読む。1 回の実行の query 数には上限があり、
/// 上限に達したら位置を残して終わる(次の購読で続ける)。読み終えたら印を残し、以後は行わない。読み終えた後の edge は、
/// docs の event(key 単位)で入る。event を取りこぼしたとき(`Lagged`)だけ、`restart_own_author_edge_sweep` で最初から読み直す。
pub(crate) async fn sweep_own_author_edges(
    services: &ServiceHandles,
    local_author_pubkey: &str,
) -> Result<AuthorHydration> {
    sweep_own_author_edges_with(
        services,
        local_author_pubkey,
        OWN_EDGE_BATCH,
        OWN_EDGE_MAX_QUERIES,
    )
    .await
}

pub(crate) async fn sweep_own_author_edges_with(
    services: &ServiceHandles,
    local_author_pubkey: &str,
    batch: usize,
    max_queries: usize,
) -> Result<AuthorHydration> {
    let replica = author_replica_id(local_author_pubkey);
    let projection_store = services.projection_store.as_ref();
    // 自分の docs author。record は組で先に読み、旧名義でしか読めなかった edge を書き直すのに使う。
    let docs_author = services.docs_sync.local_docs_author().await?;
    let mut outcome = AuthorHydration::default();
    for prefix in ["graph/follows/", "graph/blocks/"] {
        let checkpoint_key = own_edge_checkpoint_key(local_author_pubkey, prefix);
        let checkpoint = projection_store
            .get_sync_checkpoint(&checkpoint_key)
            .await?;
        if checkpoint.as_deref() == Some(CHECKPOINT_DONE) {
            continue;
        }
        let done_through = checkpoint
            .as_deref()
            .and_then(|value| value.strip_prefix(CHECKPOINT_AFTER))
            .map(str::to_string);
        // 名義を問わない一覧で読む。自分の edge は、ADR 0053 以前の端末ごとの名義でも書かれている。docs author を指定すると、
        // それらを 1 件も読まない(独立監査 B-5)。他の名義のごみの key は、読み進めを遅らせるだけで、位置は残る。
        let mut keys = HexBucketedKeys::new(prefix, batch, max_queries, done_through);
        while let Some(entries) = keys
            .next_batch(services.docs_sync.as_ref(), &replica)
            .await?
        {
            let mut seen = BTreeSet::new();
            for entry in entries {
                if !seen.insert(entry.key.clone()) {
                    continue;
                }
                // 本体がまだ手元に無い key は、相手から取る。1 件の失敗で同じ位置に留まらないよう、飛ばして進む。
                match read_author_record(
                    services,
                    local_author_pubkey,
                    &replica,
                    &entry.key,
                    docs_author.as_deref(),
                    DocFetchPolicy::LocalThenRemote,
                )
                .await
                {
                    Ok(read) => {
                        outcome.add(read.outcome);
                        // 旧名義でしか読めなかった自分の edge は、自分の docs author で書き直す。
                        if docs_author.is_some()
                            && !read.read_by_docs_author
                            && let Err(error) =
                                rewrite_own_legacy_edge(services.docs_sync.as_ref(), &read).await
                        {
                            warn!(
                                author_pubkey = %local_author_pubkey,
                                key = %entry.key,
                                error = %error,
                                "failed to rewrite an own edge under the account docs author"
                            );
                        }
                    }
                    Err(error) => {
                        warn!(
                            author_pubkey = %local_author_pubkey,
                            key = %entry.key,
                            error = %error,
                            "skipping an own edge that could not be read"
                        );
                    }
                }
            }
            if let Some(bucket) = keys.last_bucket() {
                projection_store
                    .put_sync_checkpoint(&checkpoint_key, &format!("{CHECKPOINT_AFTER}{bucket}"))
                    .await?;
            }
            tokio::task::yield_now().await;
        }
        if keys.stopped_early() {
            break;
        }
        projection_store
            .put_sync_checkpoint(&checkpoint_key, CHECKPOINT_DONE)
            .await?;
    }
    if outcome.changed > 0 {
        rebuild_relationships(services, local_author_pubkey).await?;
    }
    Ok(outcome)
}

/// 自分の edge の読み出しを、最初からやり直す(#1239)。自分の replica の event を取りこぼしたとき(`Lagged`)に使う。
/// どの key を取りこぼしたかは分からないので、読み終えた印があっても消す。
pub(crate) async fn restart_own_author_edge_sweep(
    projection_store: &dyn ProjectionStore,
    local_author_pubkey: &str,
) -> Result<()> {
    for prefix in ["graph/follows/", "graph/blocks/"] {
        projection_store
            .put_sync_checkpoint(
                &own_edge_checkpoint_key(local_author_pubkey, prefix),
                CHECKPOINT_RESTART,
            )
            .await?;
    }
    Ok(())
}

fn own_edge_checkpoint_key(local_author_pubkey: &str, prefix: &str) -> String {
    format!("own-author-edges/{local_author_pubkey}/{prefix}")
}

/// author replica の key の種類。
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthorKeyKind {
    Profile,
    Follow,
    Block,
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
    Ok(
        read_author_record(services, author_pubkey, replica, key, docs_author, policy)
            .await?
            .outcome,
    )
}

/// `hydrate_author_record` の結果に、反映した envelope と、それを docs author と key の組で読めたかを添えたもの。
struct AuthorRecordRead {
    outcome: AuthorHydration,
    envelope: Option<KukuriEnvelope>,
    kind: Option<AuthorKeyKind>,
    read_by_docs_author: bool,
}

async fn read_author_record(
    services: &ServiceHandles,
    author_pubkey: &str,
    replica: &ReplicaId,
    key: &str,
    docs_author: Option<&str>,
    policy: DocFetchPolicy,
) -> Result<AuthorRecordRead> {
    let empty = |kind| AuthorRecordRead {
        outcome: AuthorHydration::default(),
        envelope: None,
        kind,
        read_by_docs_author: false,
    };
    let kind = if key == stable_key("profile", "latest") {
        AuthorKeyKind::Profile
    } else if key.starts_with("graph/follows/") {
        AuthorKeyKind::Follow
    } else if key.starts_with("graph/blocks/") {
        AuthorKeyKind::Block
    } else {
        return Ok(empty(None));
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
    let read_by_docs_author = newest.is_some();
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
        return Ok(empty(Some(kind)));
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
    Ok(AuthorRecordRead {
        outcome: AuthorHydration::reflected(changed),
        envelope: Some(envelope),
        kind: Some(kind),
        read_by_docs_author,
    })
}

/// 自分の replica で、自分の docs author 以外の名義(ADR 0053 以前の端末ごとの名義)で読めた edge を、自分の docs author で
/// 書き直す(ADR 0053 §6)。書き直すと、同じ key の旧名義の entry は消え、以後はどの端末でも docs author と key の組で読める。
async fn rewrite_own_legacy_edge(docs_sync: &dyn DocsSync, read: &AuthorRecordRead) -> Result<()> {
    let Some(envelope) = read.envelope.as_ref() else {
        return Ok(());
    };
    match read.kind {
        Some(AuthorKeyKind::Follow) => {
            if let Ok(Some(edge)) = parse_follow_edge(envelope) {
                persist_follow_edge_doc(docs_sync, &edge, envelope).await?;
            }
        }
        Some(AuthorKeyKind::Block) => {
            if let Ok(Some(edge)) = parse_block_edge(envelope) {
                persist_block_edge_doc(docs_sync, &edge, envelope).await?;
            }
        }
        _ => {}
    }
    Ok(())
}
