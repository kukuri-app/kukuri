//! WP-S6 T4【contract】row_mapping round-trip 層 — social 系(後続 WP-H1 の安全網)。
//!
//! 対象: profiles(sqlite/social.rs:74-97 / 123-150 のインライン写像 —
//! row_mapping.rs の外にあり列追加時に忘れやすい)/ follow_edges
//! (row_to_follow_edge + updated_at latest-wins)/ muted_authors
//! (row_to_muted_author)/ author_relationship_cache
//! (row_to_author_relationship_projection + rebuild の局所置換)。
//! 分割元の全体説明は row_mapping_roundtrip.rs の冒頭を参照。

use super::*;
use kukuri_core::{AssetRef, AssetRole, BlockEdge, BlockEdgeStatus, FollowEdge};

// ---------------------------------------------------------------------------
// profiles(sqlite/social.rs のインライン写像。asset 3 列を含む)
// ---------------------------------------------------------------------------

/// max fixture: 全 Option=Some。picture_asset は picture_blob_hash /
/// picture_mime / picture_bytes の 3 列に分解して永続化される。
fn profile_max() -> Profile {
    Profile {
        pubkey: "a".repeat(64).into(),
        name: Some("name-max".into()),
        display_name: Some("表示名 ✨".into()),
        about: Some("自己紹介 text".into()),
        picture_asset: Some(AssetRef {
            hash: BlobHash::new("9".repeat(64)),
            mime: "image/webp".into(),
            bytes: 2_468,
            role: AssetRole::ProfileAvatar,
        }),
        updated_at: 1_700_000_004_000,
    }
}

/// min fixture: 全 Option=None。
fn profile_min() -> Profile {
    Profile {
        pubkey: "b".repeat(64).into(),
        name: None,
        display_name: None,
        about: None,
        picture_asset: None,
        updated_at: 5,
    }
}

#[tokio::test]
async fn profile_roundtrip_preserves_all_columns_via_get_profile_and_get_profiles() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = profile_max();
    let min = profile_min();
    Store::upsert_profile(&store, max.clone())
        .await
        .expect("upsert max profile");
    Store::upsert_profile(&store, min.clone())
        .await
        .expect("upsert min profile");

    // 現挙動(観測値): sqlx-sqlite は NULL を TEXT→"" / INTEGER→0 に
    // デコードするため、全列 NULL の min fixture は Some("") で読み出され、
    // picture_asset も NULL の picture_blob_hash から
    // Some(AssetRef { hash: "", mime: "", bytes: 0 }) が再構築される。
    let expected_min = Profile {
        pubkey: min.pubkey.clone(),
        name: Some(String::new()),
        display_name: Some(String::new()),
        about: Some(String::new()),
        picture_asset: Some(AssetRef {
            hash: BlobHash::new(""),
            mime: String::new(),
            bytes: 0,
            role: AssetRole::ProfileAvatar,
        }),
        updated_at: 5,
    };

    // get_profile(sqlite/social.rs:74-97 のインライン写像)
    assert_eq!(
        Store::get_profile(&store, max.pubkey.as_str())
            .await
            .expect("get max profile"),
        Some(max.clone())
    );
    assert_eq!(
        Store::get_profile(&store, min.pubkey.as_str())
            .await
            .expect("get min profile"),
        Some(expected_min.clone())
    );

    // get_profiles(sqlite/social.rs:123-150 の重複インライン写像)も同値を返す。
    let profiles = Store::get_profiles(
        &store,
        &[
            max.pubkey.as_str().to_string(),
            min.pubkey.as_str().to_string(),
            "c".repeat(64),
        ],
    )
    .await
    .expect("get profiles");
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles.get(max.pubkey.as_str()), Some(&max));
    assert_eq!(profiles.get(min.pubkey.as_str()), Some(&expected_min));
}

#[tokio::test]
async fn profile_picture_asset_role_reads_back_as_profile_avatar() {
    // role 列は存在せず永続化されない。読み出し側は常に ProfileAvatar を再構築する。
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let mut profile = profile_max();
    profile.pubkey = "d".repeat(64).into();
    let mut asset = profile.picture_asset.clone().expect("fixture asset");
    asset.role = AssetRole::ImageOriginal;
    profile.picture_asset = Some(asset.clone());
    Store::upsert_profile(&store, profile.clone())
        .await
        .expect("upsert profile");

    let fetched = Store::get_profile(&store, profile.pubkey.as_str())
        .await
        .expect("get profile")
        .expect("profile exists");
    let fetched_asset = fetched.picture_asset.expect("picture asset");
    assert_eq!(fetched_asset.role, AssetRole::ProfileAvatar);
    assert_eq!(fetched_asset.hash, asset.hash);
    assert_eq!(fetched_asset.mime, asset.mime);
    assert_eq!(fetched_asset.bytes, asset.bytes);
}

#[tokio::test]
async fn profile_picture_mime_and_bytes_null_read_back_as_empty_and_zero() {
    // 既存 DB 互換: picture_blob_hash ありで picture_mime / picture_bytes が
    // NULL の行の読み出しを固定する。put 経由では作れないため生 SQL で NULL にする。
    //
    // 現挙動(観測値): sqlx-sqlite は NULL を TEXT→"" / INTEGER→0 に
    // デコードして try_get が Ok を返すため、sqlite/social.rs:74-97 の
    // "application/octet-stream" / unwrap_or_default フォールバックは
    // NULL 列では発火せず、mime は "" になる(フォールバックは実質デッドコード)。
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = profile_max();
    Store::upsert_profile(&store, max.clone())
        .await
        .expect("upsert profile");
    sqlx::query("UPDATE profiles SET picture_mime = NULL, picture_bytes = NULL WHERE pubkey = ?1")
        .bind(max.pubkey.as_str())
        .execute(store.pool())
        .await
        .expect("null out picture_mime/picture_bytes");

    let fetched = Store::get_profile(&store, max.pubkey.as_str())
        .await
        .expect("get profile")
        .expect("profile exists");
    assert_eq!(
        fetched.picture_asset,
        Some(AssetRef {
            hash: BlobHash::new("9".repeat(64)),
            mime: String::new(),
            bytes: 0,
            role: AssetRole::ProfileAvatar,
        })
    );
}

// ---------------------------------------------------------------------------
// follow_edges(row_to_follow_edge + updated_at latest-wins)
// 既存 sqlite_store.rs のテストは envelope 経由で status のみ検証 — ここでは
// 直接 upsert で全列 round-trip と latest-wins の境界(tie)を固定する。
// ---------------------------------------------------------------------------

fn follow_edge(
    subject: &str,
    target: &str,
    status: FollowEdgeStatus,
    updated_at: i64,
    envelope_id: &str,
) -> FollowEdge {
    FollowEdge {
        subject_pubkey: subject.into(),
        target_pubkey: target.into(),
        status,
        updated_at,
        envelope_id: EnvelopeId::from(envelope_id),
    }
}

#[tokio::test]
async fn follow_edge_roundtrip_preserves_all_columns_and_ordering() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let subject = "a".repeat(64);
    let target_active = "b".repeat(64);
    let target_revoked = "c".repeat(64);
    let active = follow_edge(
        &subject,
        &target_active,
        FollowEdgeStatus::Active,
        300,
        "env-follow-1",
    );
    let revoked = follow_edge(
        &subject,
        &target_revoked,
        FollowEdgeStatus::Revoked,
        200,
        "env-follow-2",
    );
    Store::upsert_follow_edge(&store, active.clone())
        .await
        .expect("upsert active edge");
    Store::upsert_follow_edge(&store, revoked.clone())
        .await
        .expect("upsert revoked edge");

    // 主キー updated_at DESC の順序ごと全列固定(副キー target_pubkey ASC の
    // tie-break はここでは未行使 — T6 の pagination テストと T8 の backend_parity が担保)。
    assert_eq!(
        Store::list_follow_edges_by_subject(&store, subject.as_str())
            .await
            .expect("list by subject"),
        vec![active.clone(), revoked]
    );
    assert_eq!(
        Store::list_follow_edges_by_target(&store, target_active.as_str())
            .await
            .expect("list by target"),
        vec![active]
    );
}

#[tokio::test]
async fn follow_edge_upsert_latest_wins_and_tie_overwrites() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let subject = "d".repeat(64);
    let target = "e".repeat(64);
    let current = follow_edge(
        &subject,
        &target,
        FollowEdgeStatus::Active,
        300,
        "env-current",
    );
    Store::upsert_follow_edge(&store, current.clone())
        .await
        .expect("upsert current edge");

    // 古い updated_at(299 < 300)は黙って無視される。
    let stale = follow_edge(
        &subject,
        &target,
        FollowEdgeStatus::Revoked,
        299,
        "env-stale",
    );
    Store::upsert_follow_edge(&store, stale)
        .await
        .expect("upsert stale edge");
    assert_eq!(
        Store::list_follow_edges_by_subject(&store, subject.as_str())
            .await
            .expect("list after stale"),
        vec![current]
    );

    // 同値 updated_at(300)は上書きされる(既存 > 新規のときだけ無視 = tie は新規勝ち)。
    let tie = follow_edge(&subject, &target, FollowEdgeStatus::Revoked, 300, "env-tie");
    Store::upsert_follow_edge(&store, tie.clone())
        .await
        .expect("upsert tie edge");
    assert_eq!(
        Store::list_follow_edges_by_subject(&store, subject.as_str())
            .await
            .expect("list after tie"),
        vec![tie]
    );

    // 新しい updated_at(301)は当然上書き。
    let newer = follow_edge(
        &subject,
        &target,
        FollowEdgeStatus::Active,
        301,
        "env-newer",
    );
    Store::upsert_follow_edge(&store, newer.clone())
        .await
        .expect("upsert newer edge");
    assert_eq!(
        Store::list_follow_edges_by_subject(&store, subject.as_str())
            .await
            .expect("list after newer"),
        vec![newer]
    );
}

#[tokio::test]
async fn block_edge_roundtrip_and_latest_wins() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let subject = "1".repeat(64);
    let target = "2".repeat(64);
    let active = BlockEdge {
        subject_pubkey: subject.as_str().into(),
        target_pubkey: target.as_str().into(),
        status: BlockEdgeStatus::Active,
        updated_at: 300,
        envelope_id: EnvelopeId::from("env-block-active"),
    };
    Store::upsert_block_edge(&store, active.clone())
        .await
        .expect("upsert active block");
    Store::upsert_block_edge(
        &store,
        BlockEdge {
            status: BlockEdgeStatus::Revoked,
            updated_at: 299,
            envelope_id: EnvelopeId::from("env-block-stale"),
            ..active.clone()
        },
    )
    .await
    .expect("ignore stale unblock");

    assert_eq!(
        Store::list_block_edges_by_subject(&store, subject.as_str())
            .await
            .expect("list by subject"),
        vec![active.clone()]
    );
    assert_eq!(
        Store::list_block_edges_by_target(&store, target.as_str())
            .await
            .expect("list by target"),
        vec![active]
    );
}

// ---------------------------------------------------------------------------
// muted_authors(row_to_muted_author)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn muted_author_roundtrip_preserves_all_columns_and_ordering() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let newer = MutedAuthorRow {
        author_pubkey: "a".repeat(64),
        muted_at: 200,
    };
    let older = MutedAuthorRow {
        author_pubkey: "b".repeat(64),
        muted_at: 100,
    };
    SocialProjectionStore::put_muted_author(&store, older.clone())
        .await
        .expect("put older muted");
    SocialProjectionStore::put_muted_author(&store, newer.clone())
        .await
        .expect("put newer muted");

    assert_eq!(
        SocialProjectionStore::get_muted_author(&store, newer.author_pubkey.as_str())
            .await
            .expect("get muted"),
        Some(newer.clone())
    );
    // 主キー muted_at DESC の順序ごと固定(副キー author_pubkey ASC の
    // tie-break はここでは未行使 — T6 の pagination テストと T8 の backend_parity が担保)。
    assert_eq!(
        SocialProjectionStore::list_muted_authors(&store)
            .await
            .expect("list muted"),
        vec![newer, older]
    );
}

// ---------------------------------------------------------------------------
// #1221 R4-D: 関係は対象の author の follow edge から読むときに求める(cache と全件の再計算を持たない)。
// sqlite と memory で同じ結果になることを固定する。
// ---------------------------------------------------------------------------

async fn relationships_derived_from_edges<S: Store + SocialProjectionStore>(store: &S) {
    let local = "a".repeat(64);
    let (friend_one, friend_two, target, mutual) = (
        "2".repeat(64),
        "1".repeat(64),
        "3".repeat(64),
        "4".repeat(64),
    );
    let edge =
        |subject: &str, target: &str, status: FollowEdgeStatus, updated_at: i64| FollowEdge {
            subject_pubkey: kukuri_core::Pubkey::from(subject),
            target_pubkey: kukuri_core::Pubkey::from(target),
            status,
            updated_at,
            envelope_id: EnvelopeId::from(format!("follow-{subject}-{target}-{updated_at}")),
        };
    for (subject, to) in [
        (&local, &friend_one),
        (&local, &friend_two),
        (&friend_one, &target),
        (&friend_two, &target),
        (&target, &local),
        (&local, &mutual),
        (&mutual, &local),
    ] {
        store
            .upsert_follow_edge(edge(subject, to, FollowEdgeStatus::Active, 1))
            .await
            .expect("edge");
    }
    let get = |author: String| {
        let local = local.clone();
        async move {
            SocialProjectionStore::get_author_relationship(store, &local, &author)
                .await
                .expect("relationship")
        }
    };
    let followed = get(target.clone()).await.expect("followed by target");
    assert!(followed.followed_by && !followed.following && !followed.mutual);
    assert!(followed.friend_of_friend);
    assert_eq!(
        followed.friend_of_friend_via_pubkeys,
        vec![friend_two.clone(), friend_one.clone()]
    );
    assert!(get(mutual.clone()).await.expect("mutual").mutual);
    assert_eq!(get("9".repeat(64)).await, None, "no edge, no relationship");
    assert_eq!(
        get(local.clone()).await,
        None,
        "no relationship with oneself"
    );

    store
        .upsert_follow_edge(edge(&local, &target, FollowEdgeStatus::Active, 2))
        .await
        .expect("follow back");
    let now_mutual = get(target.clone()).await.expect("mutual after follow back");
    assert!(now_mutual.mutual && !now_mutual.friend_of_friend);
    assert!(now_mutual.friend_of_friend_via_pubkeys.is_empty());

    store
        .upsert_follow_edge(edge(&mutual, &local, FollowEdgeStatus::Revoked, 2))
        .await
        .expect("unfollow");
    let revoked = get(mutual.clone()).await.expect("still following");
    assert!(revoked.following && !revoked.followed_by && !revoked.mutual);

    let listed = SocialProjectionStore::list_author_relationships(
        store,
        &local,
        &[target.clone(), "9".repeat(64)],
    )
    .await
    .expect("list");
    assert_eq!(listed.len(), 1);
    assert!(listed[&target].mutual);
}

#[tokio::test]
async fn author_relationships_are_derived_from_follow_edges() {
    relationships_derived_from_edges(&SqliteStore::connect_memory().await.expect("sqlite store"))
        .await;
    relationships_derived_from_edges(&crate::MemoryStore::default()).await;
}

// 関係を読む join は、自分の follow を主 key の前方一致で読み、対象への edge を主 key で引く(対象の follower の総数を走査しない)。
#[tokio::test]
async fn author_relationship_join_reads_by_primary_keys() {
    use sqlx::Row;
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT mine.target_pubkey FROM follow_edges AS mine \
         JOIN follow_edges AS theirs ON theirs.subject_pubkey = mine.target_pubkey \
         AND theirs.target_pubkey = ?2 AND theirs.status = 'active' \
         WHERE mine.subject_pubkey = ?1 AND mine.status = 'active' ORDER BY mine.target_pubkey",
    )
    .bind("a".repeat(64))
    .bind("b".repeat(64))
    .fetch_all(store.pool())
    .await
    .expect("plan");
    let details = plan
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>();
    assert!(
        details.iter().all(|detail| !detail.starts_with("SCAN")),
        "relationship join must not scan follow_edges: {details:?}"
    );
}
