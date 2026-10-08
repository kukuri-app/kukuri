//! 公開参照の索引（#1632、ADR 0063）。公開記録の書込み・取り下げ・更新で、保持端末を探せる hash（`is_public_blob`）が
//! 決まること。期待値は SQLite の実装の結果で、store の試験が SQLite、Web の browser 試験が IndexedDB の実装で同じ操作列を
//! 確かめる。

use kukuri_core::{
    AssetRef, AssetRole, BlobHash, CustomReactionAssetSnapshotV1, EnvelopeId, ObjectStatus,
    PayloadRef, PostWithdrawalReason, Profile, Pubkey, ReactionKeyKind, ReplicaId,
    RepostSourceSnapshotV1, TopicId, WithdrawalReasonVisibility,
};

use crate::{
    ContentCacheStore, ObjectProjectionRow, PostWithdrawalRow, ProjectionStore,
    ReactionProjectionRow, Store,
};

const REPLICA: &str = "bucket::v1::topic::746f706963::1";

pub fn hash(seed: u32) -> String {
    format!("{seed:064x}")
}

pub fn asset(hash: &str) -> AssetRef {
    AssetRef {
        hash: BlobHash::new(hash),
        mime: "image/png".into(),
        bytes: 1,
        role: AssetRole::ImageOriginal,
    }
}

pub fn post(
    object_id: &str,
    channel: &str,
    body: &str,
    attachments: &[&str],
) -> ObjectProjectionRow {
    ObjectProjectionRow {
        object_id: EnvelopeId::from(object_id),
        topic_id: "topic".into(),
        channel_id: channel.into(),
        author_pubkey: "b".repeat(64),
        created_at: 1,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::BlobText {
            hash: BlobHash::new(body),
            mime: "text/plain".into(),
            bytes: 1,
        },
        content: Some("body".into()),
        attachments: attachments.iter().map(|hash| asset(hash)).collect(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new(REPLICA),
        source_key: format!("objects/{object_id}/envelope"),
        source_envelope_id: EnvelopeId::from(object_id),
        source_blob_hash: Some(BlobHash::new(body)),
        source_docs_author: None,
        derived_at: 1,
        projection_version: crate::VERIFIED_OBJECT_PROJECTION_VERSION,
    }
}

pub fn reaction(
    reaction_id: &str,
    target: &str,
    asset_hash: &str,
    status: ObjectStatus,
) -> ReactionProjectionRow {
    ReactionProjectionRow {
        source_replica_id: ReplicaId::new(REPLICA),
        target_object_id: EnvelopeId::from(target),
        reaction_id: EnvelopeId::from(reaction_id),
        author_pubkey: "c".repeat(64),
        created_at: 1,
        updated_at: 1,
        reaction_key_kind: ReactionKeyKind::CustomAsset,
        normalized_reaction_key: format!("custom:{reaction_id}"),
        emoji: None,
        custom_asset_id: Some(format!("asset-{reaction_id}")),
        custom_asset_snapshot: Some(CustomReactionAssetSnapshotV1 {
            asset_id: format!("asset-{reaction_id}"),
            owner_pubkey: Pubkey::from("c".repeat(64)),
            blob_hash: BlobHash::new(asset_hash),
            search_key: String::new(),
            mime: "image/png".into(),
            bytes: 1,
            width: 1,
            height: 1,
        }),
        status,
        source_key: format!("reactions/{reaction_id}"),
        source_envelope_id: EnvelopeId::from(reaction_id),
        derived_at: 1,
        projection_version: 1,
    }
}

pub fn withdrawal(object_id: &str) -> PostWithdrawalRow {
    PostWithdrawalRow {
        target_object_id: EnvelopeId::from(object_id),
        target_author_pubkey: "b".repeat(64),
        source_replica_id: ReplicaId::new(REPLICA),
        withdrawal_envelope_id: EnvelopeId::from(format!("withdraw-{object_id}")),
        withdrawn_at: 2,
        generation: 1,
        replacement_object_id: None,
        reason_visibility: WithdrawalReasonVisibility::Private,
        reason: Some(PostWithdrawalReason::Other),
    }
}

pub fn profile(picture: Option<&str>, updated_at: i64) -> Profile {
    Profile {
        pubkey: Pubkey::from("a".repeat(64)),
        name: None,
        display_name: None,
        about: None,
        picture_asset: picture.map(asset),
        updated_at,
        nip05: None,
    }
}

/// `seeds` の hash（`hash(seed)`）ごとに、公開参照があるか。
pub async fn public(store: &dyn ContentCacheStore, seeds: &[u32]) -> Vec<bool> {
    let mut found = Vec::new();
    for seed in seeds {
        found.push(store.is_public_blob(&hash(*seed)).await.expect("public"));
    }
    found
}

/// 公開 topic の投稿の本文・添付・repost の添付・リンクプレビューの画像、profile の画像、公開 topic の有効な custom
/// reaction の asset だけが公開参照になる。取り下げ・reaction の削除・profile の更新で外れ、同じ hash を別の公開記録が
/// 参照している間は公開のまま。
pub async fn check_public_blob_refs<S: Store + ProjectionStore + ContentCacheStore>(store: &S) {
    let mut reposting = post("p1", "public", &hash(1), &[&hash(2)]);
    reposting.repost_of = Some(RepostSourceSnapshotV1 {
        source_object_id: EnvelopeId::from("source"),
        source_topic_id: TopicId::new("topic"),
        source_author_pubkey: Pubkey::from("d".repeat(64)),
        source_object_kind: "post".into(),
        content: String::new(),
        attachments: vec![asset(&hash(3))],
        reply_to_object_id: None,
        root_id: None,
        content_labels: Vec::new(),
    });
    for row in [
        reposting,
        post("p2", "channel-x", &hash(4), &[&hash(5)]),
        post("p3", "public", &hash(6), &[&hash(2)]),
    ] {
        store.put_object_projection(row).await.expect("post");
    }
    store
        .note_link_preview_image("p1", &hash(7))
        .await
        .expect("link preview");
    store
        .upsert_profile(profile(Some(&hash(8)), 1))
        .await
        .expect("profile");
    for row in [
        reaction("r1", "p1", &hash(9), ObjectStatus::Active),
        reaction("r2", "p2", &hash(10), ObjectStatus::Active),
    ] {
        store.upsert_reaction_cache(row).await.expect("reaction");
    }
    assert_eq!(
        public(store, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]).await,
        [
            true, true, true, false, false, true, true, true, true, false
        ]
    );

    store
        .put_post_withdrawal(withdrawal("p1"))
        .await
        .expect("withdrawal");
    store
        .upsert_reaction_cache(reaction("r1", "p1", &hash(9), ObjectStatus::Deleted))
        .await
        .expect("deleted reaction");
    store
        .upsert_profile(profile(None, 2))
        .await
        .expect("profile without a picture");
    assert_eq!(
        public(store, &[1, 2, 3, 7, 8, 9]).await,
        [false, true, false, false, false, false]
    );
}
