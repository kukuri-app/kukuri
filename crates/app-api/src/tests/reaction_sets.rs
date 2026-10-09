//! #1232 AC-4: カスタムリアクションのセットの作成・一覧・取り込み。

use super::sync::CountingDocsSync;
use super::*;

fn app_on(
    docs_sync: Arc<dyn DocsSync>,
    blob_service: Arc<MemoryBlobService>,
    keys: KukuriKeys,
) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        keys,
    );
    (app, store)
}

/// 他の人が作ったリアクション（画像の blob は無くてよい）。
fn foreign_asset(owner_pubkey: &str, seed: char, search_key: &str) -> CustomReactionAssetView {
    let blob_hash = seed.to_string().repeat(64);
    CustomReactionAssetView {
        asset_id: kukuri_core::custom_reaction_id(&blob_hash, search_key),
        owner_pubkey: owner_pubkey.to_string(),
        blob_hash,
        search_key: search_key.into(),
        mime: "image/png".into(),
        bytes: 128,
        width: 128,
        height: 128,
    }
}

/// 自分で作ったセット・投稿の自分のリアクションは、同じアカウントの別の端末ではその端末の自作に無いので保存済みへ
/// 置ける。作った端末では、自作にあるものは保存しない。
#[tokio::test]
async fn own_reactions_are_saved_on_another_device_of_the_same_account() {
    let blobs = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let (made_on, _) = app_on(
        Arc::new(MemoryDocsSync::default()),
        blobs.clone(),
        keys.clone(),
    );
    let (other_device, _) = app_on(Arc::new(MemoryDocsSync::default()), blobs, keys);
    let own = made_on
        .create_custom_reaction_asset(CreateCustomReactionAssetInput {
            search_key: "party".into(),
            mime: "image/png".into(),
            bytes: tiny_png_bytes(),
            width: 128,
            height: 128,
        })
        .await
        .expect("own asset");
    let set = made_on
        .create_custom_reaction_set("mine", vec![own.clone()])
        .await
        .expect("create set");

    let imported = other_device
        .import_custom_reaction_set(&set.set_hash)
        .await
        .expect("import on the other device");
    let same_device = made_on
        .import_custom_reaction_set(&set.set_hash)
        .await
        .expect("import on the same device");

    assert_eq!(
        (imported.saved, imported.skipped_own),
        (vec![own.clone()], 0)
    );
    assert_eq!(
        other_device
            .list_bookmarked_custom_reactions()
            .await
            .expect("list bookmarks")
            .len(),
        1
    );
    assert_eq!((same_device.saved.len(), same_device.skipped_own), (0, 1));
    let from_post = reaction_snapshot_from_view(&own);
    assert!(
        other_device
            .bookmark_custom_reaction(from_post.clone())
            .await
            .is_ok()
    );
    assert!(made_on.bookmark_custom_reaction(from_post).await.is_err());
}

/// セットを作った人の自作・保存済みのリアクションを、別のアカウントが同じ ID・最初の作者のまま、セットの並びで
/// 保存済みへ取り込める。取り込む端末の自作にあるリアクションは保存しない。
#[tokio::test]
async fn a_shared_reaction_set_is_saved_with_the_same_reaction_ids() {
    let blobs = Arc::new(MemoryBlobService::default());
    let docs: Arc<dyn DocsSync> = Arc::new(MemoryDocsSync::default());
    let (creator, _) = app_on(docs.clone(), blobs.clone(), generate_keys());
    let (importer, _) = app_on(docs, blobs, generate_keys());
    let own = creator
        .create_custom_reaction_asset(CreateCustomReactionAssetInput {
            search_key: "party".into(),
            mime: "image/png".into(),
            bytes: tiny_png_bytes(),
            width: 128,
            height: 128,
        })
        .await
        .expect("own asset");
    let third = generate_keys().public_key_hex();
    let saved = creator
        .bookmark_custom_reaction(reaction_snapshot_from_view(&foreign_asset(
            &third, 'c', "wave",
        )))
        .await
        .expect("saved asset");
    let importers_own = importer
        .create_custom_reaction_asset(CreateCustomReactionAssetInput {
            search_key: "mine".into(),
            mime: "image/png".into(),
            bytes: tiny_png_bytes(),
            width: 128,
            height: 128,
        })
        .await
        .expect("importer's own asset");

    let set = creator
        .create_custom_reaction_set(" ねこ ", vec![own.clone(), saved.clone(), importers_own])
        .await
        .expect("create set");
    let imported = importer
        .import_custom_reaction_set(&set.set_hash)
        .await
        .expect("import set");
    let listed = importer
        .list_bookmarked_custom_reactions()
        .await
        .expect("list bookmarks");

    assert_eq!((set.name.as_str(), set.item_count), ("ねこ", 3));
    assert_eq!(
        creator
            .list_my_custom_reaction_sets()
            .await
            .expect("list sets"),
        std::slice::from_ref(&set)
    );
    assert_eq!(imported.name, "ねこ");
    assert_eq!(imported.skipped_own, 1);
    let ids = |assets: &[CustomReactionAssetView]| {
        assets
            .iter()
            .map(|asset| (asset.asset_id.clone(), asset.owner_pubkey.clone()))
            .collect::<Vec<_>>()
    };
    let expected = ids(&[own, saved]);
    assert_eq!(ids(&imported.saved), expected);
    assert_eq!(ids(&listed), expected);
}

/// 取れない・セットでない blob は失敗にし、何も保存しない。
#[tokio::test]
async fn an_unreadable_reaction_set_saves_nothing() {
    let (app, _, _, blob_service) = local_app_with_memory_services();
    let image = blob_service
        .put_blob(tiny_png_bytes(), "image/png")
        .await
        .expect("image blob");

    assert!(
        app.import_custom_reaction_set(&"e".repeat(64))
            .await
            .is_err()
    );
    assert!(
        app.import_custom_reaction_set(image.hash.as_str())
            .await
            .is_err()
    );
    assert!(
        app.list_bookmarked_custom_reactions()
            .await
            .expect("list bookmarks")
            .is_empty()
    );
}

/// 1 つのセットは 100 件まで。作ったセットの一覧は、新しい順の 100 件だけを読む。
#[tokio::test]
async fn reaction_sets_hold_100_reactions_and_list_the_newest_100() {
    let (app, _, _, _) = local_app_with_memory_services();
    let owner = generate_keys().public_key_hex();
    let assets = (0..101)
        .map(|index| foreign_asset(&owner, 'f', &format!("key-{index}")))
        .collect::<Vec<_>>();
    assert!(
        app.create_custom_reaction_set("too many", assets.clone())
            .await
            .is_err()
    );

    let mut created = Vec::new();
    for (index, asset) in assets.into_iter().enumerate() {
        created.push(
            app.create_custom_reaction_set(&format!("set-{index:03}"), vec![asset])
                .await
                .expect("create set"),
        );
    }
    let listed = app.list_my_custom_reaction_sets().await.expect("list sets");
    assert_eq!(listed.len(), 100);
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0].created_at >= pair[1].created_at)
    );
    let left_out = created
        .iter()
        .find(|set| !listed.contains(set))
        .expect("one set is left out");
    assert!(
        listed
            .iter()
            .all(|set| set.created_at >= left_out.created_at)
    );
}

/// 取り込みが読むのはセットの blob だけで、無関係な保存済み・投稿が増えても docs を読まない。
#[tokio::test]
async fn importing_a_set_does_not_read_the_rest_of_the_library() {
    let blobs = Arc::new(MemoryBlobService::default());
    let (creator, _) = app_on(
        Arc::new(MemoryDocsSync::default()),
        blobs.clone(),
        generate_keys(),
    );
    let owner = generate_keys().public_key_hex();
    let set = creator
        .create_custom_reaction_set(
            "set",
            (0..3)
                .map(|index| foreign_asset(&owner, 'a', &format!("set-{index}")))
                .collect(),
        )
        .await
        .expect("create set");

    let mut reads = Vec::new();
    for (bookmarks, posts) in [(5, 1), (50, 10)] {
        let docs = Arc::new(CountingDocsSync::default());
        let (importer, _) = app_on(docs.clone(), blobs.clone(), generate_keys());
        for index in 0..bookmarks {
            importer
                .bookmark_custom_reaction(reaction_snapshot_from_view(&foreign_asset(
                    &owner,
                    'b',
                    &format!("unrelated-{index}"),
                )))
                .await
                .expect("unrelated bookmark");
        }
        for _ in 0..posts {
            importer
                .create_post("kukuri:topic:reaction-set-scale", "unrelated", None)
                .await
                .expect("unrelated post");
        }
        docs.clear_queries().await;
        docs.reset_records_returned();
        let key_queries = docs.key_queries();
        let imported = importer
            .import_custom_reaction_set(&set.set_hash)
            .await
            .expect("import set");
        assert_eq!(imported.saved.len(), 3);
        reads.push((
            docs.queries().await.len(),
            docs.key_queries() - key_queries,
            docs.records_returned(),
        ));
    }
    assert_eq!(reads, [(0, 0, 0), (0, 0, 0)]);
}
