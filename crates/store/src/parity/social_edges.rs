//! #1211 AC-6: 自分の follow・block の edge を相手の順に小分けに読む口と、block の edge の 1 件の読取り（移行で自分の
//! edge を 64 件ずつ送る）。#1650: 自分への follow の edge（フォロワー）を相手の順に小分けに読む口。

use super::*;
use kukuri_core::{BlockEdge, BlockEdgeStatus, FollowEdge, Pubkey};

#[derive(Debug, PartialEq)]
struct OwnEdgesScenarioResult {
    /// 先頭から 2 件と、a より後。
    first_page: Vec<FollowEdge>,
    after_a: Vec<FollowEdge>,
    block_c: Option<BlockEdge>,
    blocks_after_a: Vec<BlockEdge>,
    /// a へ follow する edge の先頭から 1 件と、subject より後・other より後（先頭の行より後の位置へ飛ぶ）。
    followers_of_a: Vec<FollowEdge>,
    followers_of_a_after_subject: Vec<FollowEdge>,
    followers_of_a_after_other: Vec<FollowEdge>,
}

async fn own_edges_scenario<S: Store + ProjectionStore>(store: &S) -> OwnEdgesScenarioResult {
    let (subject, other, third) = ("1".repeat(64), "2".repeat(64), "3".repeat(64));
    let [a, b, c] = ["a", "b", "c"].map(|target| target.repeat(64));
    for (from, to) in [
        (&subject, &b),
        (&subject, &a),
        (&subject, &c),
        (&other, &a),
        (&third, &a),
    ] {
        Store::upsert_follow_edge(
            store,
            parity_follow_edge(
                from,
                to,
                FollowEdgeStatus::Active,
                100,
                &format!("follow-{}-{}", &from[..1], &to[..1]),
            ),
        )
        .await
        .expect("Store::upsert_follow_edge");
    }
    for to in [&a, &c] {
        Store::upsert_block_edge(
            store,
            BlockEdge {
                subject_pubkey: Pubkey::from(subject.as_str()),
                target_pubkey: Pubkey::from(to.as_str()),
                status: BlockEdgeStatus::Active,
                updated_at: 100,
                envelope_id: EnvelopeId::from(format!("block-{}", &to[..1])),
            },
        )
        .await
        .expect("Store::upsert_block_edge");
    }
    OwnEdgesScenarioResult {
        first_page: Store::list_follow_edges_by_subject_after(store, &subject, None, 2)
            .await
            .expect("Store::list_follow_edges_by_subject_after"),
        after_a: Store::list_follow_edges_by_subject_after(store, &subject, Some(&a), 10)
            .await
            .expect("Store::list_follow_edges_by_subject_after"),
        block_c: Store::get_block_edge(store, &subject, &c)
            .await
            .expect("Store::get_block_edge"),
        blocks_after_a: Store::list_block_edges_by_subject_after(store, &subject, Some(&a), 10)
            .await
            .expect("Store::list_block_edges_by_subject_after"),
        followers_of_a: Store::list_follow_edges_by_target_after(store, &a, None, 1)
            .await
            .expect("Store::list_follow_edges_by_target_after"),
        followers_of_a_after_subject: Store::list_follow_edges_by_target_after(
            store,
            &a,
            Some(&subject),
            10,
        )
        .await
        .expect("Store::list_follow_edges_by_target_after"),
        followers_of_a_after_other: Store::list_follow_edges_by_target_after(
            store,
            &a,
            Some(&other),
            10,
        )
        .await
        .expect("Store::list_follow_edges_by_target_after"),
    }
}

pub(super) async fn own_edges_match_between_backends<S: Store + ProjectionStore>(
    make: &impl AsyncFn() -> S,
) {
    let backend = make().await;
    let from_backend = own_edges_scenario(&backend).await;
    assert_eq!(
        from_backend,
        own_edges_scenario(&MemoryStore::default()).await
    );

    // sanity: 相手の昇順で、`after` を含まず、別の subject の edge を含まない。
    let targets = |edges: &[FollowEdge]| {
        edges
            .iter()
            .map(|edge| edge.target_pubkey.as_str()[..1].to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(targets(&from_backend.first_page), ["a", "b"]);
    assert_eq!(targets(&from_backend.after_a), ["b", "c"]);
    assert!(from_backend.block_c.is_some());
    assert_eq!(
        from_backend
            .blocks_after_a
            .iter()
            .map(|edge| edge.target_pubkey.as_str()[..1].to_string())
            .collect::<Vec<_>>(),
        ["c"]
    );
    let subjects = |edges: &[FollowEdge]| {
        edges
            .iter()
            .map(|edge| edge.subject_pubkey.as_str()[..1].to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(subjects(&from_backend.followers_of_a), ["1"]);
    assert_eq!(
        subjects(&from_backend.followers_of_a_after_subject),
        ["2", "3"]
    );
    assert_eq!(subjects(&from_backend.followers_of_a_after_other), ["3"]);
}
