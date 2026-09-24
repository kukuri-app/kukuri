//! WP-S6 T4【contract】row_mapping round-trip 層(後続 WP-H1 の安全網)。
//!
//! trait メソッド put → get/list(UFCS)で往復させ、models.rs / kukuri_core の
//! struct 丸ごと assert_eq! で「列名対応・JSON serde・Option 化・キャスト」を
//! 全列固定する characterization テスト。テストが落ちたら疑うのは変更であり
//! テストではない。
//!
//! 本ファイル: envelopes / object_index_cache / reaction_cache /
//! bookmarked_custom_reactions / bookmarked_posts。
//! (profiles・follow_edges・muted・author_relationship は
//! row_mapping_roundtrip_social.rs、dm_* 4 テーブルと notifications は
//! row_mapping_roundtrip_dm.rs、live_session / game_room は
//! row_mapping_roundtrip_live_game.rs に分割)
//!
//! 4 ファイル共通の注意: sqlx-sqlite は NULL を i64→0 / TEXT→"" にデコードして
//! try_get::<T> が Ok を返すが、row_mapping の Option 列は `try_get::<Option<T>>`
//! (opt_col ヘルパ)で読むため None で put → None で get の対称が成り立つ
//! (WP-B16 で decode quirk を解消。min fixture がその回帰検出を兼ねる)。

use super::*;
use kukuri_core::{
    AssetRef, AssetRole, CustomReactionAssetSnapshotV1, KukuriEnvelope, RepostSourceSnapshotV1,
};

// ---------------------------------------------------------------------------
// envelopes(row_to_envelope)
// ---------------------------------------------------------------------------

/// max fixture: tags は多要素 + unicode の非自明な JSON、全列に非デフォルト値。
fn envelope_max() -> KukuriEnvelope {
    KukuriEnvelope {
        id: EnvelopeId::from("env-max"),
        pubkey: "f".repeat(64).into(),
        created_at: 1_700_000_000_000,
        kind: "unit-test-max".into(),
        tags: vec![
            vec!["topic".into(), "kukuri:topic:envelope-rt".into()],
            vec![
                "e".into(),
                "env-parent".into(),
                "wss://relay.example".into(),
                "reply".into(),
            ],
            vec!["emoji".into(), "🔥".into()],
        ],
        content: "本文 with 🔥 and \"quotes\"".into(),
        sig: "sig-max".into(),
    }
}

/// min fixture: tags 空 Vec・content / sig 空文字。
fn envelope_min() -> KukuriEnvelope {
    KukuriEnvelope {
        id: EnvelopeId::from("env-min"),
        pubkey: "0".repeat(64).into(),
        created_at: 0,
        kind: "unit-test-min".into(),
        tags: Vec::new(),
        content: String::new(),
        sig: String::new(),
    }
}

#[tokio::test]
async fn envelope_roundtrip_preserves_all_columns() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = envelope_max();
    let min = envelope_min();
    Store::put_envelope(&store, max.clone())
        .await
        .expect("put max envelope");
    Store::put_envelope(&store, min.clone())
        .await
        .expect("put min envelope");

    assert_eq!(
        Store::get_envelope(&store, &max.id)
            .await
            .expect("get max envelope"),
        Some(max)
    );
    assert_eq!(
        Store::get_envelope(&store, &min.id)
            .await
            .expect("get min envelope"),
        Some(min)
    );

    // tags_json の永続形式(JSON 配列の配列、unicode は素通し)を生リテラルで固定。
    let tags_json = sqlx::query_scalar::<_, String>(
        "SELECT tags_json FROM envelopes WHERE envelope_id = 'env-max'",
    )
    .fetch_one(store.pool())
    .await
    .expect("raw tags_json");
    assert_eq!(
        tags_json,
        r#"[["topic","kukuri:topic:envelope-rt"],["e","env-parent","wss://relay.example","reply"],["emoji","🔥"]]"#
    );
}

// ---------------------------------------------------------------------------
// object_index_cache 全 18 列(row_to_object_projection)
// ---------------------------------------------------------------------------

/// max fixture: 全 Option=Some・attachments 2 要素・repost_of の入れ子も全 Some。
fn object_projection_max() -> ObjectProjectionRow {
    ObjectProjectionRow {
        object_id: EnvelopeId::from("obj-max"),
        topic_id: "kukuri:topic:object-rt".into(),
        channel_id: "private:friends".into(),
        author_pubkey: "a".repeat(64),
        created_at: 1_700_000_001_000,
        object_kind: "post".into(),
        root_object_id: Some(EnvelopeId::from("obj-root")),
        reply_to_object_id: Some(EnvelopeId::from("obj-parent")),
        payload_ref: PayloadRef::BlobText {
            hash: BlobHash::new("1".repeat(64)),
            mime: "text/markdown".into(),
            bytes: 321,
        },
        content: Some("投稿本文 📝".into()),
        attachments: vec![
            AssetRef {
                hash: BlobHash::new("2".repeat(64)),
                mime: "image/png".into(),
                bytes: 2_048,
                role: AssetRole::ImageOriginal,
            },
            AssetRef {
                hash: BlobHash::new("3".repeat(64)),
                mime: "video/mp4".into(),
                bytes: 1_048_576,
                role: AssetRole::VideoPoster,
            },
        ],
        repost_of: Some(RepostSourceSnapshotV1 {
            source_object_id: EnvelopeId::from("src-object"),
            source_topic_id: TopicId::new("kukuri:topic:source"),
            source_author_pubkey: "b".repeat(64).into(),
            source_object_kind: "comment".into(),
            content: "引用元 💬".into(),
            attachments: vec![AssetRef {
                hash: BlobHash::new("4".repeat(64)),
                mime: "image/webp".into(),
                bytes: 512,
                role: AssetRole::ImagePreview,
            }],
            reply_to_object_id: Some(EnvelopeId::from("src-parent")),
            root_id: Some(EnvelopeId::from("src-root")),
            content_labels: Vec::new(),
        }),
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new("topic::kukuri:topic:object-rt"),
        source_key: "objects/obj-max/header".into(),
        source_envelope_id: EnvelopeId::from("env-obj-max"),
        source_blob_hash: Some(BlobHash::new("5".repeat(64))),
        source_docs_author: None,
        derived_at: 1_700_000_001_500,
        projection_version: 7,
    }
}

/// min fixture: 全 Option=None・attachments 空 Vec。
fn object_projection_min() -> ObjectProjectionRow {
    ObjectProjectionRow {
        object_id: EnvelopeId::from("obj-min"),
        topic_id: "kukuri:topic:object-rt".into(),
        channel_id: "public".into(),
        author_pubkey: "c".repeat(64),
        created_at: 0,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::InlineText {
            text: String::new(),
        },
        content: None,
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new("topic::kukuri:topic:object-rt"),
        source_key: "objects/obj-min/header".into(),
        source_envelope_id: EnvelopeId::from("env-obj-min"),
        source_blob_hash: None,
        source_docs_author: None,
        derived_at: 0,
        projection_version: 0,
    }
}

#[tokio::test]
async fn object_projection_roundtrip_preserves_all_18_columns() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = object_projection_max();
    let min = object_projection_min();
    ObjectProjectionStore::put_object_projection(&store, max.clone())
        .await
        .expect("put max projection");
    ObjectProjectionStore::put_object_projection(&store, min.clone())
        .await
        .expect("put min projection");

    assert_eq!(
        ObjectProjectionStore::get_object_projection(&store, &max.object_id)
            .await
            .expect("get max projection"),
        Some(max)
    );

    // None で put した Option 列(content / source_blob_hash / root / reply /
    // repost_of)は None のまま読み出される(WP-B16 で NULL decode quirk を解消)。
    assert_eq!(
        ObjectProjectionStore::get_object_projection(&store, &min.object_id)
            .await
            .expect("get min projection"),
        Some(min)
    );
}

// ---------------------------------------------------------------------------
// reaction_cache 全 16 列(row_to_reaction_projection)
// ---------------------------------------------------------------------------

/// max fixture: CustomAsset + snapshot / emoji / custom_asset_id 全 Some、status 非デフォルト。
fn reaction_max() -> ReactionProjectionRow {
    ReactionProjectionRow {
        source_replica_id: ReplicaId::new("topic::kukuri:topic:reaction-rt"),
        target_object_id: EnvelopeId::from("target-max"),
        reaction_id: EnvelopeId::from("reaction-max"),
        author_pubkey: "a".repeat(64),
        created_at: 1_700_000_002_000,
        updated_at: 1_700_000_002_100,
        reaction_key_kind: ReactionKeyKind::CustomAsset,
        normalized_reaction_key: "custom_asset:asset-max".into(),
        emoji: Some("🎉".into()),
        custom_asset_id: Some("asset-max".into()),
        custom_asset_snapshot: Some(CustomReactionAssetSnapshotV1 {
            asset_id: "asset-max".into(),
            owner_pubkey: "b".repeat(64).into(),
            blob_hash: BlobHash::new("6".repeat(64)),
            search_key: "うれしい".into(),
            mime: "image/gif".into(),
            bytes: 12_345,
            width: 128,
            height: 96,
        }),
        status: ObjectStatus::Edited,
        source_key: "reactions/reaction-max".into(),
        source_envelope_id: EnvelopeId::from("env-reaction-max"),
        derived_at: 1_700_000_002_200,
        projection_version: 4,
    }
}

/// min fixture: Emoji kind で emoji / custom_asset_id / snapshot すべて None。
fn reaction_min() -> ReactionProjectionRow {
    ReactionProjectionRow {
        source_replica_id: ReplicaId::new("topic::kukuri:topic:reaction-rt"),
        target_object_id: EnvelopeId::from("target-min"),
        reaction_id: EnvelopeId::from("reaction-min"),
        author_pubkey: "c".repeat(64),
        created_at: 0,
        updated_at: 0,
        reaction_key_kind: ReactionKeyKind::Emoji,
        normalized_reaction_key: "emoji:👍".into(),
        emoji: None,
        custom_asset_id: None,
        custom_asset_snapshot: None,
        status: ObjectStatus::Active,
        source_key: "reactions/reaction-min".into(),
        source_envelope_id: EnvelopeId::from("env-reaction-min"),
        derived_at: 0,
        projection_version: 0,
    }
}

#[tokio::test]
async fn reaction_projection_roundtrip_preserves_all_16_columns() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = reaction_max();
    let min = reaction_min();
    ReactionBookmarkStore::upsert_reaction_cache(&store, max.clone())
        .await
        .expect("upsert max reaction");
    ReactionBookmarkStore::upsert_reaction_cache(&store, min.clone())
        .await
        .expect("upsert min reaction");

    assert_eq!(
        ReactionBookmarkStore::get_reaction_cache(
            &store,
            &max.source_replica_id,
            &max.target_object_id,
            &max.reaction_id,
        )
        .await
        .expect("get max reaction"),
        Some(max)
    );
    // None で put した emoji / custom_asset_id / snapshot は None のまま
    // 読み出される(WP-B16 で NULL decode quirk を解消)。
    assert_eq!(
        ReactionBookmarkStore::get_reaction_cache(
            &store,
            &min.source_replica_id,
            &min.target_object_id,
            &min.reaction_id,
        )
        .await
        .expect("get min reaction"),
        Some(min)
    );
}

// ---------------------------------------------------------------------------
// bookmarked_custom_reactions(row_to_bookmarked_custom_reaction)
// ---------------------------------------------------------------------------

/// max fixture: bytes は u32 を超える値で i64 経由の u64 キャストを踏む。
fn bookmarked_custom_reaction_max() -> BookmarkedCustomReactionRow {
    BookmarkedCustomReactionRow {
        asset_id: "asset-bookmark-max".into(),
        owner_pubkey: "a".repeat(64),
        blob_hash: BlobHash::new("7".repeat(64)),
        search_key: "パーティー 🎊".into(),
        mime: "image/apng".into(),
        bytes: 5_000_000_000,
        width: 1_920,
        height: 1_080,
        bookmarked_at: 1_700_000_003_000,
    }
}

/// min fixture: 数値列すべて 0。search_key の空文字は asset_id フォールバック
/// (T5 の edge 層で固定)になるため、ここでは非空にして素通しを固定する。
fn bookmarked_custom_reaction_min() -> BookmarkedCustomReactionRow {
    BookmarkedCustomReactionRow {
        asset_id: "asset-bookmark-min".into(),
        owner_pubkey: "b".repeat(64),
        blob_hash: BlobHash::new("8".repeat(64)),
        search_key: "min".into(),
        mime: "image/png".into(),
        bytes: 0,
        width: 0,
        height: 0,
        bookmarked_at: 1_000,
    }
}

/// tie fixture: bookmarked_at を max と同値にし、副キー asset_id DESC を実際に行使する
/// (asset-bookmark-aaa < asset-bookmark-max なので DESC では max の後ろに並ぶ)。
fn bookmarked_custom_reaction_tie() -> BookmarkedCustomReactionRow {
    BookmarkedCustomReactionRow {
        asset_id: "asset-bookmark-aaa".into(),
        owner_pubkey: "c".repeat(64),
        blob_hash: BlobHash::new("9".repeat(64)),
        search_key: "tie".into(),
        mime: "image/gif".into(),
        bytes: 1,
        width: 1,
        height: 1,
        bookmarked_at: 1_700_000_003_000,
    }
}

#[tokio::test]
async fn bookmarked_custom_reaction_roundtrip_preserves_all_columns() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = bookmarked_custom_reaction_max();
    let tie = bookmarked_custom_reaction_tie();
    let min = bookmarked_custom_reaction_min();
    ReactionBookmarkStore::put_bookmarked_custom_reaction(&store, min.clone())
        .await
        .expect("put min bookmark");
    ReactionBookmarkStore::put_bookmarked_custom_reaction(&store, tie.clone())
        .await
        .expect("put tie bookmark");
    ReactionBookmarkStore::put_bookmarked_custom_reaction(&store, max.clone())
        .await
        .expect("put max bookmark");

    // 読み出しは list のみ。主キー bookmarked_at DESC に加え、max と tie が
    // bookmarked_at 同値のため副キー asset_id DESC(tie-break)もここで行使して固定。
    assert_eq!(
        ReactionBookmarkStore::list_bookmarked_custom_reactions(&store)
            .await
            .expect("list bookmarks"),
        vec![max, tie, min]
    );
}

// ---------------------------------------------------------------------------
// bookmarked_posts 全 15 列(row_to_bookmarked_post)
// ---------------------------------------------------------------------------

/// max fixture: 全 Option=Some・attachments 非空・repost_of 非自明。
fn bookmarked_post_max() -> BookmarkedPostRow {
    BookmarkedPostRow {
        source_object_id: EnvelopeId::from("bp-obj-max"),
        source_envelope_id: EnvelopeId::from("bp-env-max"),
        source_replica_id: ReplicaId::new("topic::kukuri:topic:bookmark-rt"),
        topic_id: "kukuri:topic:bookmark-rt".into(),
        channel_id: "private:club".into(),
        author_pubkey: "a".repeat(64),
        created_at: 1_700_000_004_000,
        object_kind: "comment".into(),
        payload_ref: PayloadRef::BlobText {
            hash: BlobHash::new("9".repeat(64)),
            mime: "text/plain".into(),
            bytes: 77,
        },
        content: Some("ブックマーク本文 🔖".into()),
        attachments: vec![AssetRef {
            hash: BlobHash::new("a".repeat(64)),
            mime: "image/jpeg".into(),
            bytes: 65_536,
            role: AssetRole::Attachment,
        }],
        reply_to_object_id: Some(EnvelopeId::from("bp-parent")),
        root_object_id: Some(EnvelopeId::from("bp-root")),
        repost_of: Some(RepostSourceSnapshotV1 {
            source_object_id: EnvelopeId::from("bp-src-object"),
            source_topic_id: TopicId::new("kukuri:topic:bookmark-src"),
            source_author_pubkey: "b".repeat(64).into(),
            source_object_kind: "post".into(),
            content: "元投稿".into(),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        }),
        bookmarked_at: 2_000,
    }
}

/// min fixture: 全 Option=None・attachments 空 Vec。
fn bookmarked_post_min() -> BookmarkedPostRow {
    BookmarkedPostRow {
        source_object_id: EnvelopeId::from("bp-obj-min"),
        source_envelope_id: EnvelopeId::from("bp-env-min"),
        source_replica_id: ReplicaId::new("topic::kukuri:topic:bookmark-rt"),
        topic_id: "kukuri:topic:bookmark-rt".into(),
        channel_id: "public".into(),
        author_pubkey: "c".repeat(64),
        created_at: 0,
        object_kind: "post".into(),
        payload_ref: PayloadRef::InlineText {
            text: String::new(),
        },
        content: None,
        attachments: Vec::new(),
        reply_to_object_id: None,
        root_object_id: None,
        repost_of: None,
        bookmarked_at: 1_000,
    }
}

#[tokio::test]
async fn bookmarked_post_roundtrip_preserves_all_15_columns() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = bookmarked_post_max();
    let min = bookmarked_post_min();
    ReactionBookmarkStore::put_bookmarked_post(&store, min.clone())
        .await
        .expect("put min bookmarked post");
    ReactionBookmarkStore::put_bookmarked_post(&store, max.clone())
        .await
        .expect("put max bookmarked post");

    // None で put した content / reply_to / root / repost_of は None のまま
    // 読み出される(WP-B16 で object_index_cache 側と同じ trim + filter に統一し、
    // NULL decode quirk と非対称を解消。'' ケースは T5 の raw INSERT テストで固定)。
    // 読み出しは list のみ。順序は主キー bookmarked_at DESC のみ行使して固定
    // (副キー source_object_id DESC の tie-break はここでは未行使 —
    // T6 の pagination テストと T8 の backend_parity が担保)。
    assert_eq!(
        ReactionBookmarkStore::list_bookmarked_posts_page(&store, None, false)
            .await
            .expect("list bookmarked posts"),
        vec![max, min]
    );
}
