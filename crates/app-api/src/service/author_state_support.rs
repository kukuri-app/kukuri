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
pub(crate) struct HexBucketedKeys {
    root_len: usize,
    pending: Vec<String>,
    batch: usize,
    queries_left: usize,
}

impl HexBucketedKeys {
    pub(crate) fn new(prefix: &str, batch: usize, max_queries: usize) -> Self {
        Self {
            root_len: prefix.len(),
            pending: vec![prefix.to_string()],
            batch,
            queries_left: max_queries,
        }
    }

    /// 次の batch。尽きたか、query の数の上限に達したら `None`。
    pub(crate) async fn next_batch(
        &mut self,
        docs_sync: &dyn DocsSync,
        replica: &ReplicaId,
    ) -> Result<Option<Vec<DocKeyEntry>>> {
        while let Some(bucket) = self.pending.pop() {
            if self.queries_left == 0 {
                return Ok(None);
            }
            self.queries_left -= 1;
            let page = docs_sync
                .query_replica_keys(
                    replica,
                    DocKeyQuery {
                        prefix: bucket.clone(),
                        order: DocKeyOrder::Ascending,
                        limit: self.batch,
                    },
                )
                .await?;
            if page.reached_limit && bucket.len() < self.root_len + 64 {
                self.pending.extend(
                    "fedcba9876543210"
                        .chars()
                        .map(|digit| format!("{bucket}{digit}")),
                );
                continue;
            }
            return Ok(Some(page.entries));
        }
        Ok(None)
    }
}

/// 起動時と追いつきで読む author replica の key。`profile/latest`、自分を指す follow・block の key、
/// `graph/follows/`・`graph/blocks/` の key の上限つきの一覧(それぞれ `AUTHOR_EDGE_KEYS` 件)。値は読まない。
/// 同じ key は 1 回だけ返す(一覧は docs author ごとの entry を返す)。
async fn author_state_keys(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    local_author_pubkey: &str,
) -> Result<Vec<String>> {
    let mut keys = vec![
        stable_key("profile", "latest"),
        stable_key("graph/follows", local_author_pubkey),
        stable_key("graph/blocks", local_author_pubkey),
    ];
    let mut seen = keys.iter().cloned().collect::<BTreeSet<_>>();
    for prefix in ["graph/follows/", "graph/blocks/"] {
        let page = docs_sync
            .query_replica_keys(
                replica,
                DocKeyQuery {
                    prefix: prefix.to_string(),
                    order: DocKeyOrder::Ascending,
                    limit: AUTHOR_EDGE_KEYS,
                },
            )
            .await?;
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
    let mut outcome = AuthorHydration::default();
    for key in author_state_keys(services.docs_sync.as_ref(), &replica, local_author_pubkey).await?
    {
        outcome.add(hydrate_author_record(services, author_pubkey, &replica, &key, policy).await?);
    }
    Ok(outcome)
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
    let outcome = hydrate_author_record(services, author_pubkey, &replica, key, policy).await?;
    if outcome.changed > 0 {
        rebuild_relationships(services, local_author_pubkey).await?;
    }
    Ok(outcome)
}

/// 自分の replica の follow・block の edge を、背景で小分けにすべて読む。新しい端末で、自分の follow が
/// `AUTHOR_EDGE_KEYS` 件を超えていても、自分の follow の一覧が欠けないようにする。
///
/// 自分の follow の数に比例するが、自分の author 購読の開始時に 1 回だけ、背景で batch ごとに譲りながら進む。
/// 購読タスクが止まると止まり、次の購読で最初からやり直す(手元にある envelope は書き直さない)。
pub(crate) async fn sweep_own_author_edges(
    services: &ServiceHandles,
    local_author_pubkey: &str,
) -> Result<AuthorHydration> {
    let replica = author_replica_id(local_author_pubkey);
    let mut outcome = AuthorHydration::default();
    for prefix in ["graph/follows/", "graph/blocks/"] {
        let mut keys = HexBucketedKeys::new(prefix, OWN_EDGE_BATCH, OWN_EDGE_MAX_QUERIES);
        while let Some(entries) = keys
            .next_batch(services.docs_sync.as_ref(), &replica)
            .await?
        {
            let mut seen = BTreeSet::new();
            for entry in entries {
                if seen.insert(entry.key.clone()) {
                    outcome.add(
                        hydrate_author_record(
                            services,
                            local_author_pubkey,
                            &replica,
                            &entry.key,
                            DocFetchPolicy::LocalOnly,
                        )
                        .await?,
                    );
                }
            }
            tokio::task::yield_now().await;
        }
    }
    if outcome.changed > 0 {
        rebuild_relationships(services, local_author_pubkey).await?;
    }
    Ok(outcome)
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
        fetch_author_envelope_by_id(docs_sync, replica, &envelope_id, policy).await?
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
/// 同じ key に docs author ごとの record がありうる(誰でも書ける replica。ADR 0053 以前の端末ごとの名義も残る)。
/// 先頭の 1 件だけを見ず、上限つき(`AUTHOR_RECORDS_PER_KEY` 件)で調べ、検証に通ったものから最も新しい
/// envelope を選ぶ。読めない record と、検証に通らない record は飛ばす。
async fn hydrate_author_record(
    services: &ServiceHandles,
    author_pubkey: &str,
    replica: &ReplicaId,
    key: &str,
    policy: DocFetchPolicy,
) -> Result<AuthorHydration> {
    let kind = if key == stable_key("profile", "latest") {
        AuthorKeyKind::Profile
    } else if key.starts_with("graph/follows/") {
        AuthorKeyKind::Follow
    } else if key.starts_with("graph/blocks/") {
        AuthorKeyKind::Block
    } else {
        return Ok(AuthorHydration::default());
    };
    let docs_sync = services.docs_sync.as_ref();
    let records = docs_sync
        .query_replica_exact_bounded(replica, key, AUTHOR_RECORDS_PER_KEY, policy)
        .await?;
    let mut newest: Option<KukuriEnvelope> = None;
    for record in &records {
        if let Some(envelope) =
            verified_author_envelope(docs_sync, replica, author_pubkey, kind, record, policy)
                .await?
            && newest.as_ref().is_none_or(|current| {
                (envelope.created_at, envelope.id.as_str())
                    > (current.created_at, current.id.as_str())
            })
        {
            newest = Some(envelope);
        }
    }
    let Some(envelope) = newest else {
        return Ok(AuthorHydration::default());
    };
    let store = services.store.as_ref();
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
