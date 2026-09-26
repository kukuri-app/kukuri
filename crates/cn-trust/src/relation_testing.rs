//! `RelationStore` 実装への共有 contract スイート（`testing` feature）。
//!
//! in-memory（本 crate）と ArcadeDB（`cn-indexer`）、将来の neo4j が **同一の contract** を
//! 満たすことを保証する。各実装のテストはこのスイートを呼ぶだけにし、契約の drift を防ぐ。
//!
//! `prefix` は pubkey の名前空間。永続 backend（ArcadeDB）に対して実行するとき、
//! 走行間の残留データと衝突しないよう呼び出し側が一意な prefix を渡す。

use anyhow::{Context, Result, ensure};

use crate::relation::{
    ClusterRef, EdgeFeatures, FEATURE_CO_PARTICIPATION_EVENTS, FEATURE_SHARED_TOPICS,
    RelationStore, proximity_from_features,
};

fn pk(prefix: &str, name: &str) -> String {
    format!("{prefix}-{name}")
}

/// `relation_is_pairwise_cluster_proximity`: 同一 cluster で共起の濃い 2 者は近接度が高く、
/// 共起の無い 2 者は低い（根拠つき）。
pub async fn assert_pairwise_cluster_proximity(
    store: &dyn RelationStore,
    prefix: &str,
) -> Result<()> {
    let (a, b, c) = (pk(prefix, "a"), pk(prefix, "b"), pk(prefix, "c"));
    let dense = EdgeFeatures::new()
        .with(FEATURE_SHARED_TOPICS, 4.0)
        .with(FEATURE_CO_PARTICIPATION_EVENTS, 20.0);
    let sparse = EdgeFeatures::new()
        .with(FEATURE_SHARED_TOPICS, 1.0)
        .with(FEATURE_CO_PARTICIPATION_EVENTS, 1.0);
    store.upsert_edge(&a, &b, &dense).await?;
    store.upsert_edge(&a, &c, &sparse).await?;

    let near = store
        .pairwise_proximity(&a, &b)
        .await?
        .context("proximity(a, b) should exist")?;
    let far = store
        .pairwise_proximity(&a, &c)
        .await?
        .context("proximity(a, c) should exist")?;
    ensure!(
        near.score > far.score,
        "dense co-participation must yield higher proximity: near={} far={}",
        near.score,
        far.score
    );
    ensure!(
        (0.0..=1.0).contains(&near.score) && (0.0..=1.0).contains(&far.score),
        "proximity score must stay in [0, 1]"
    );
    // edge の無い 2 者には proximity が無い（勝手に作らない）。
    let none = store
        .pairwise_proximity(&b, &pk(prefix, "stranger"))
        .await?;
    ensure!(none.is_none(), "no edge must mean no proximity");
    Ok(())
}

/// `relation_read_is_explainable`: proximity は feature 内訳（basis）を必ず同伴し、
/// backend 非依存の合成（`proximity_from_features`）と一致する。
pub async fn assert_proximity_is_explainable(
    store: &dyn RelationStore,
    prefix: &str,
) -> Result<()> {
    let (a, b) = (pk(prefix, "xa"), pk(prefix, "xb"));
    let features = EdgeFeatures::new()
        .with(FEATURE_SHARED_TOPICS, 2.0)
        .with(FEATURE_CO_PARTICIPATION_EVENTS, 5.0);
    store.upsert_edge(&a, &b, &features).await?;

    let proximity = store
        .pairwise_proximity(&a, &b)
        .await?
        .context("proximity should exist")?;
    let expected = proximity_from_features(&features);
    ensure!(
        proximity == expected,
        "backend must return the canonical feature composition: got {proximity:?}, want {expected:?}"
    );
    ensure!(
        !proximity.basis.is_empty(),
        "proximity basis must not be empty"
    );
    for entry in &proximity.basis {
        ensure!(
            entry.contribution.is_finite() && entry.weight.is_finite(),
            "basis entries must carry finite weight / contribution"
        );
    }
    Ok(())
}

/// 対称性: foundation の feature（co-participation 系）は対称なので、(from, to) の upsert が
/// (to, from) の read からも同じに見える。
pub async fn assert_symmetric_lookup(store: &dyn RelationStore, prefix: &str) -> Result<()> {
    let (a, b) = (pk(prefix, "sa"), pk(prefix, "sb"));
    let features = EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 3.0);
    store.upsert_edge(&a, &b, &features).await?;
    let forward = store.pairwise_proximity(&a, &b).await?;
    let backward = store.pairwise_proximity(&b, &a).await?;
    ensure!(
        forward == backward && forward.is_some(),
        "pairwise proximity must be readable from both directions"
    );
    Ok(())
}

/// `neighbors`: proximity 降順で最大 k 件。
pub async fn assert_neighbors_ranked(store: &dyn RelationStore, prefix: &str) -> Result<()> {
    let v = pk(prefix, "viewer");
    let (n1, n2, n3) = (pk(prefix, "n1"), pk(prefix, "n2"), pk(prefix, "n3"));
    store
        .upsert_edge(
            &v,
            &n1,
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 9.0),
        )
        .await?;
    store
        .upsert_edge(
            &v,
            &n2,
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 1.0),
        )
        .await?;
    store
        .upsert_edge(
            &v,
            &n3,
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 4.0),
        )
        .await?;

    let top2 = store.neighbors(&v, 2).await?;
    ensure!(
        top2 == vec![n1.clone(), n3.clone()],
        "neighbors must be proximity-descending and k-limited: got {top2:?}"
    );
    Ok(())
}

/// `cluster_of` / `set_cluster` の往復。未割り当ては None。
pub async fn assert_cluster_roundtrip(store: &dyn RelationStore, prefix: &str) -> Result<()> {
    let member = pk(prefix, "member");
    ensure!(
        store.cluster_of(&member).await?.is_none(),
        "unassigned pubkey must have no cluster"
    );
    let cluster = ClusterRef(format!("{prefix}-topic:rust"));
    store.set_cluster(&member, &cluster).await?;
    let read = store.cluster_of(&member).await?;
    ensure!(
        read == Some(cluster),
        "cluster assignment must round-trip, got {read:?}"
    );
    Ok(())
}

/// 成立しなくなったペアの edge と、参加 0 の author の cluster を消せる（#1221 R5-E）。どちらの向きで消しても同じ。
pub async fn assert_edge_and_cluster_removal(
    store: &dyn RelationStore,
    prefix: &str,
) -> Result<()> {
    let (a, b) = (pk(prefix, "ra"), pk(prefix, "rb"));
    store
        .upsert_edge(
            &a,
            &b,
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 1.0),
        )
        .await?;
    store.remove_edge(&b, &a).await?;
    ensure!(
        store.pairwise_proximity(&a, &b).await?.is_none()
            && store.neighbors(&a, 8).await?.is_empty(),
        "a removed edge must not be readable"
    );
    store
        .set_cluster(&a, &ClusterRef(format!("{prefix}-topic:gone")))
        .await?;
    store.clear_cluster(&a).await?;
    ensure!(
        store.cluster_of(&a).await?.is_none(),
        "a cleared cluster must not be readable"
    );
    Ok(())
}

/// `proximity_scores` は `pairwise_proximity` と同じ score を返し、edge の無い candidate を含めない
/// （relation 値 R の重み。ADR 0026 §8.2）。
pub async fn assert_proximity_scores(store: &dyn RelationStore, prefix: &str) -> Result<()> {
    let viewer = pk(prefix, "score-viewer");
    let near = pk(prefix, "score-near");
    let far = pk(prefix, "score-far");
    let unrelated = pk(prefix, "score-unrelated");
    store
        .upsert_edge(
            &viewer,
            &near,
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 9.0),
        )
        .await?;
    store
        .upsert_edge(
            &far,
            &viewer,
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 0.25),
        )
        .await?;
    let scores = store
        .proximity_scores(&viewer, &[near.clone(), far.clone(), unrelated.clone()])
        .await?;
    ensure!(
        !scores.contains_key(&unrelated),
        "candidate without an edge must be omitted: {scores:?}"
    );
    for candidate in [&near, &far] {
        let expected = store
            .pairwise_proximity(&viewer, candidate)
            .await?
            .map(|proximity| proximity.score);
        ensure!(
            scores.get(candidate).copied() == expected,
            "proximity_scores must match pairwise_proximity for {candidate}: {scores:?}"
        );
    }
    ensure!(
        scores[&near] > scores[&far],
        "closer candidate must have a higher score: {scores:?}"
    );
    ensure!(
        store.proximity_scores(&viewer, &[]).await?.is_empty(),
        "empty candidates must return an empty map"
    );
    Ok(())
}

/// 全 contract を一括実行する（各実装のテストはこれを呼ぶ）。
pub async fn assert_relation_store_contracts(
    store: &dyn RelationStore,
    prefix: &str,
) -> Result<()> {
    assert_pairwise_cluster_proximity(store, prefix).await?;
    assert_proximity_is_explainable(store, prefix).await?;
    assert_symmetric_lookup(store, prefix).await?;
    assert_neighbors_ranked(store, prefix).await?;
    assert_cluster_roundtrip(store, prefix).await?;
    assert_proximity_scores(store, prefix).await?;
    assert_edge_and_cluster_removal(store, prefix).await?;
    Ok(())
}
