//! peer の接続候補（`PeerCandidateStore`。native の `sqlite/peer_candidates.rs`）。reload の後も同じ候補を読む。
//! 学習した候補だけを期限（30 日）と容量（64 MiB）で古いものから消し、明示の ticket と設定の seed は消さない。

use anyhow::{Result, ensure};
use async_trait::async_trait;
use kukuri_store::{
    LEARNED_BUDGET_BYTES, LEARNED_PRUNE_STEP, LEARNED_RETENTION_MS, LEARNED_SOURCE, MAX_ADDR_BYTES,
    PeerCandidateStore, peer_candidate_bytes,
};
use wasm_bindgen::JsValue;
use web_sys::{IdbKeyRange, IdbTransaction};

use crate::IndexedDbCache;
use crate::content_cache::{META, PEER_CANDIDATES};
use crate::idb::{self, Mode, js_error};
use crate::rows::{self, Txn, between, key, num, prefix, text, top};

const LEARNED_BYTES: &str = "peer_learned_bytes";
const IMPORTED: &str = "imported";
const SEED: &str = "seed";

#[derive(serde::Serialize, serde::Deserialize)]
struct Candidate {
    scope: String,
    source: String,
    endpoint_id: String,
    addr: Vec<u8>,
    accounted_bytes: i64,
    seen_ms: i64,
}

fn candidate_key(scope: &str, source: &str, endpoint_id: &str) -> JsValue {
    key(&[text(scope), text(source), text(endpoint_id)])
}

/// 学習した候補だけが載る、回収の順（見た時刻, id）の索引の値。
fn learned_extra(row: &Candidate) -> [(&'static str, JsValue); 1] {
    let learned = if row.source == LEARNED_SOURCE {
        key(&[num(row.seen_ms), text(&row.endpoint_id)])
    } else {
        JsValue::UNDEFINED
    };
    [("learned", learned)]
}

async fn meta_number(tx: &IdbTransaction, name: &str) -> Result<i64> {
    let request = rows::store(tx, META)?.get(&text(name)).map_err(js_error)?;
    Ok(idb::done(&request).await?.as_f64().unwrap_or(0.0) as i64)
}

fn set_meta(tx: &IdbTransaction, name: &str, value: &JsValue) -> Result<()> {
    rows::store(tx, META)?
        .put_with_key(value, &text(name))
        .map_err(js_error)?;
    Ok(())
}

/// 学習した候補を古い順に、`keep` が偽を返すまで（最大 `limit` 件）消し、消した bytes を返す。
async fn evict_learned(
    tx: &IdbTransaction,
    range: &IdbKeyRange,
    limit: usize,
    mut keep: impl FnMut(i64) -> bool,
) -> Result<i64> {
    let oldest: Vec<Candidate> =
        rows::scan(tx, PEER_CANDIDATES, Some("learned"), range, false, limit).await?;
    let mut freed = 0;
    for row in oldest {
        if keep(freed) {
            break;
        }
        rows::delete(
            tx,
            PEER_CANDIDATES,
            &candidate_key(&row.scope, &row.source, &row.endpoint_id),
        )?;
        freed += row.accounted_bytes;
    }
    Ok(freed)
}

/// 期限の過ぎた学習した候補を 1 回分（`LEARNED_PRUNE_STEP` 件まで）消す。
async fn prune_learned(tx: &IdbTransaction, now_ms: i64) -> Result<()> {
    let expired = IdbKeyRange::upper_bound_with_open(
        &key(&[num(now_ms.saturating_sub(LEARNED_RETENTION_MS))]),
        true,
    )
    .map_err(js_error)?;
    let freed = evict_learned(tx, &expired, LEARNED_PRUNE_STEP, |_| false).await?;
    if freed > 0 {
        set_meta(
            tx,
            LEARNED_BYTES,
            &num(meta_number(tx, LEARNED_BYTES).await? - freed),
        )?;
    }
    Ok(())
}

fn cutoff(source: &str, now_ms: i64) -> i64 {
    if source == LEARNED_SOURCE {
        now_ms.saturating_sub(LEARNED_RETENTION_MS)
    } else {
        i64::MIN
    }
}

impl IndexedDbCache {
    /// 学習した候補の容量の上限を指定して置く（試験で小さい上限を使う）。
    pub(crate) async fn put_peer_candidate_bounded(
        &self,
        id: (&str, &str, &str),
        addr: &[u8],
        now_ms: i64,
        budget_bytes: i64,
    ) -> Result<bool> {
        ensure!(
            addr.len() <= MAX_ADDR_BYTES,
            "peer address exceeds candidate budget"
        );
        let (scope, source, endpoint_id) = id;
        let row = Candidate {
            scope: scope.to_owned(),
            source: source.to_owned(),
            endpoint_id: endpoint_id.to_owned(),
            addr: addr.to_vec(),
            accounted_bytes: peer_candidate_bytes(scope, source, endpoint_id, addr),
            seen_ms: now_ms,
        };
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PEER_CANDIDATES, META], Mode::Write)?;
            let previous = rows::get::<Candidate>(
                &tx,
                PEER_CANDIDATES,
                &candidate_key(&row.scope, &row.source, &row.endpoint_id),
            )
            .await?;
            let changed = previous
                .as_ref()
                .is_none_or(|previous| previous.addr != row.addr);
            let old_bytes = previous.map_or(0, |previous| previous.accounted_bytes);
            rows::put(&tx, PEER_CANDIDATES, &row, &learned_extra(&row))?;
            if row.source == LEARNED_SOURCE {
                let total =
                    meta_number(&tx, LEARNED_BYTES).await? + row.accounted_bytes - old_bytes;
                set_meta(&tx, LEARNED_BYTES, &num(total))?;
                prune_learned(&tx, now_ms).await?;
                let total = meta_number(&tx, LEARNED_BYTES).await?;
                if total > budget_bytes {
                    let all = IdbKeyRange::lower_bound(&key(&[num(i64::MIN)])).map_err(js_error)?;
                    // 1 件ずつ古い順に、合計が上限に収まるまで消す（消す数は超えた bytes の分だけ）。
                    let mut freed = 0;
                    while total - freed > budget_bytes {
                        let step = evict_learned(&tx, &all, 1, |_| false).await?;
                        if step == 0 {
                            break;
                        }
                        freed += step;
                    }
                    set_meta(&tx, LEARNED_BYTES, &num(total - freed))?;
                }
            }
            tx.commit().await?;
            Ok(changed)
        })
        .await
    }
}

#[async_trait]
impl PeerCandidateStore for IndexedDbCache {
    async fn put_peer_candidate(
        &self,
        scope: &str,
        source: &str,
        endpoint_id: &str,
        addr: &[u8],
        now_ms: i64,
    ) -> Result<bool> {
        self.put_peer_candidate_bounded(
            (scope, source, endpoint_id),
            addr,
            now_ms,
            LEARNED_BUDGET_BYTES,
        )
        .await
    }

    async fn peer_candidate_window(
        &self,
        scope: &str,
        source: &str,
        after: Option<(i64, String)>,
        limit: usize,
        now_ms: i64,
    ) -> Result<Vec<(String, Vec<u8>, i64)>> {
        let limit = limit.min(64);
        let (scope, source) = (scope.to_owned(), source.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PEER_CANDIDATES, META], Mode::Write)?;
            if source == LEARNED_SOURCE {
                prune_learned(&tx, now_ms).await?;
            }
            let cutoff = cutoff(&source, now_ms);
            let head = [text(&scope), text(&source)];
            let (after_ms, after_id) = after
                .clone()
                .filter(|(seen_ms, _)| *seen_ms >= cutoff)
                .unwrap_or((cutoff, String::new()));
            let lower = [
                head[0].clone(),
                head[1].clone(),
                num(after_ms),
                text(&after_id),
            ];
            let mut found: Vec<Candidate> = rows::scan(
                &tx,
                PEER_CANDIDATES,
                Some("window"),
                &between(&lower, &top(&head), true, false)?,
                false,
                limit,
            )
            .await?;
            // 末尾に届いたら、期限の内の先頭から `after` まで折り返す。
            if found.len() < limit
                && let Some((after_ms, after_id)) = after
            {
                let start = [head[0].clone(), head[1].clone(), num(cutoff)];
                let end = [
                    head[0].clone(),
                    head[1].clone(),
                    num(after_ms),
                    text(&after_id),
                ];
                found.extend(
                    rows::scan::<Candidate>(
                        &tx,
                        PEER_CANDIDATES,
                        Some("window"),
                        &between(&start, &end, false, false)?,
                        false,
                        limit - found.len(),
                    )
                    .await?,
                );
            }
            tx.commit().await?;
            Ok(found
                .into_iter()
                .map(|row| (row.endpoint_id, row.addr, row.seen_ms))
                .collect())
        })
        .await
    }

    async fn peer_candidate_by_id(
        &self,
        scope: &str,
        source: &str,
        endpoint_id: &str,
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>> {
        let (scope, source, endpoint) =
            (scope.to_owned(), source.to_owned(), endpoint_id.to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PEER_CANDIDATES], Mode::Read)?;
            Ok(rows::get::<Candidate>(
                &tx,
                PEER_CANDIDATES,
                &candidate_key(&scope, &source, &endpoint),
            )
            .await?
            .filter(|row| row.seen_ms >= cutoff(&source, now_ms))
            .map(|row| row.addr))
        })
        .await
    }

    async fn imported_peer_candidate_ids(
        &self,
        scope: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let (scope, after) = (scope.to_owned(), after.unwrap_or("").to_owned());
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PEER_CANDIDATES], Mode::Read)?;
            let head = [text(&scope), text(IMPORTED)];
            let lower = [head[0].clone(), head[1].clone(), text(&after)];
            let found: Vec<Candidate> = rows::scan(
                &tx,
                PEER_CANDIDATES,
                None,
                &between(&lower, &top(&head), true, false)?,
                false,
                limit.min(65),
            )
            .await?;
            Ok(found.into_iter().map(|row| row.endpoint_id).collect())
        })
        .await
    }

    async fn imported_peer_candidate_window(
        &self,
        scope: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        let limit = limit.min(4);
        let (scope, after) = (scope.to_owned(), after.map(str::to_owned));
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PEER_CANDIDATES], Mode::Read)?;
            let head = [text(&scope), text(IMPORTED)];
            let lower = [
                head[0].clone(),
                head[1].clone(),
                text(after.as_deref().unwrap_or("")),
            ];
            let mut found: Vec<Candidate> = rows::scan(
                &tx,
                PEER_CANDIDATES,
                None,
                &between(&lower, &top(&head), true, false)?,
                false,
                limit,
            )
            .await?;
            if found.len() < limit
                && let Some(after) = &after
            {
                let end = [head[0].clone(), head[1].clone(), text(after)];
                found.extend(
                    rows::scan::<Candidate>(
                        &tx,
                        PEER_CANDIDATES,
                        None,
                        &between(&head, &end, false, false)?,
                        false,
                        limit - found.len(),
                    )
                    .await?,
                );
            }
            Ok(found
                .into_iter()
                .map(|row| (row.endpoint_id, row.addr))
                .collect())
        })
        .await
    }

    async fn replace_seed_candidates(
        &self,
        scope: &str,
        seeds: Vec<(String, Vec<u8>)>,
        now_ms: i64,
    ) -> Result<()> {
        for (_, addr) in &seeds {
            ensure!(
                addr.len() <= MAX_ADDR_BYTES,
                "peer address exceeds candidate budget"
            );
        }
        let digest = blake3::hash(&serde_json::to_vec(&seeds)?)
            .to_hex()
            .to_string();
        let scope = scope.to_owned();
        self.run(move |db| async move {
            let tx = Txn::begin(&db.idb, &[PEER_CANDIDATES, META], Mode::Write)?;
            let state = format!("peer_seed:{scope}");
            let request = rows::store(&tx, META)?
                .get(&text(&state))
                .map_err(js_error)?;
            if idb::done(&request).await?.as_string().as_deref() == Some(digest.as_str()) {
                return Ok(());
            }
            rows::delete(
                &tx,
                PEER_CANDIDATES,
                &prefix(&[text(&scope), text(SEED)])?.into(),
            )?;
            for (endpoint_id, addr) in seeds {
                let row = Candidate {
                    scope: scope.clone(),
                    source: SEED.into(),
                    endpoint_id,
                    addr,
                    accounted_bytes: 0,
                    seen_ms: now_ms,
                };
                rows::put(&tx, PEER_CANDIDATES, &row, &learned_extra(&row))?;
            }
            set_meta(&tx, &state, &text(&digest))?;
            tx.commit().await
        })
        .await
    }
}
