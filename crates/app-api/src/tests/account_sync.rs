//! 本人の端末間の account 同期の起動と停止（#1218 AC-2、ADR 0061 §6）。

use super::*;

#[tokio::test]
async fn account_sync_starts_privately_and_stops_with_the_runtime() {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(MemoryDocsSync::default());
    let transport = Arc::new(FakeTransport::new("account-sync", FakeNetwork::default()));
    let keys = generate_keys();
    let account = keys.derive_account_sync();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport.clone(),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        keys,
    );
    let hint = kukuri_core::wire::hint_topic_id(account.hint_topic())
        .as_str()
        .to_string();
    let read = || docs.query_replica(account.replica_id(), DocQuery::All);

    assert!(read().await.is_err(), "登録前は公開の導出で開かない");
    app.start_account_sync()
        .await
        .expect("account 同期を始める");
    assert!(read().await.is_ok(), "登録した秘密で開ける");
    let subscribed = transport.subscribed_topics().await.expect("購読の一覧");
    assert!(subscribed.contains(&hint), "hint の topic を購読する");
    assert!(
        normalize_topics(subscribed).is_empty(),
        "診断の topic の一覧に出ない"
    );
    assert!(
        app.leased_topics().await.is_empty(),
        "公開の topic の lease にならない"
    );

    app.shutdown().await;
    assert!(
        !transport
            .subscribed_topics()
            .await
            .expect("購読の一覧")
            .contains(&hint),
        "停止で hint の購読を抜ける"
    );
}

// ---- #1218 AC-3: profile・設定の merge（ADR 0061 §4） ----

use super::sync::CountingDocsSync;
use kukuri_core::{AccountSyncItem, AccountSyncItemKey, KukuriProfileEnvelopeContentV1};
use kukuri_docs_sync::DocFetchPolicy;

const ACCOUNT_DOCS_AUTHOR: &str =
    "acacacacacacacacacacacacacacacacacacacacacacacacacacacacacacacac";

fn device(keys: &KukuriKeys, docs: Arc<dyn DocsSync>) -> AppService {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("account-device", FakeNetwork::default()));
    app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs,
        Arc::new(MemoryBlobService::default()),
        keys.clone(),
    )
}

fn profile_at(keys: &KukuriKeys, name: &str, created_at: i64) -> KukuriEnvelope {
    let content = KukuriProfileEnvelopeContentV1 {
        author_pubkey: keys.public_key(),
        name: Some(name.to_string()),
        ..Default::default()
    };
    let tags = vec![
        vec!["author".into(), keys.public_key_hex()],
        vec!["object".into(), "identity-profile".into()],
    ];
    kukuri_core::sign_envelope_json_at(keys, "identity-profile", tags, &content, created_at)
        .expect("profile envelope")
}

fn always_visible(author: &str, updated_at: i64, op: char, visible: bool) -> AccountSyncItem {
    AccountSyncItem {
        key: AccountSyncItemKey::TrustAlwaysVisible {
            author: Pubkey::from(author),
        },
        op_id: op.to_string().repeat(32),
        updated_at,
        value: visible.then_some(serde_json::Value::Bool(true)),
    }
}

// 2 端末の編集が、逆順・同時刻・再送・古い backup のどの届き方でも同じ勝者になる。
#[tokio::test]
async fn edits_from_two_devices_converge_on_the_same_winner() {
    let keys = generate_keys();
    let author = generate_keys().public_key_hex();
    let older = AccountSyncItem::profile(&profile_at(&keys, "older", 100)).unwrap();
    let tied = [
        AccountSyncItem::profile(&profile_at(&keys, "tied-a", 200)).unwrap(),
        AccountSyncItem::profile(&profile_at(&keys, "tied-b", 200)).unwrap(),
    ];
    let profile_winner = tied
        .iter()
        .max_by(|a, b| a.op_id.cmp(&b.op_id))
        .unwrap()
        .clone();
    let trust = [
        always_visible(&author, 100, 'a', true),
        always_visible(&author, 200, 'b', false),
        always_visible(&author, 200, 'c', true),
    ];
    let edits = [older, tied[0].clone(), tied[1].clone()]
        .into_iter()
        .chain(trust.iter().cloned())
        .collect::<Vec<_>>();
    let mut reversed = edits.clone();
    reversed.reverse();
    // 再送（勝者を 2 度）と、古い backup から戻した端末の版（勝者の後に古い版）。
    let resent_and_restored = [
        profile_winner.clone(),
        profile_winner.clone(),
        trust[2].clone(),
        trust[2].clone(),
        edits[0].clone(),
        trust[0].clone(),
    ];
    for sequence in [edits.as_slice(), reversed.as_slice(), &resent_and_restored] {
        let app = device(&keys, Arc::new(MemoryDocsSync::default()));
        for item in sequence {
            app.merge_account_sync_item(item.clone()).await.unwrap();
        }
        assert_eq!(
            app.get_my_profile().await.unwrap().name,
            profile_winner.value.as_ref().map(|envelope| {
                serde_json::from_value::<KukuriEnvelope>(envelope.clone())
                    .ok()
                    .and_then(|envelope| parse_profile(&envelope).ok().flatten())
                    .and_then(|profile| profile.name)
                    .unwrap()
            })
        );
        assert_eq!(
            app.trust_always_visible(std::slice::from_ref(&author))
                .await
                .unwrap(),
            BTreeSet::from([author.clone()])
        );
    }
    // 同じ操作の再受信と古い版は何もしない。
    let app = device(&keys, Arc::new(MemoryDocsSync::default()));
    assert!(app.merge_account_sync_item(trust[2].clone()).await.unwrap());
    assert!(!app.merge_account_sync_item(trust[2].clone()).await.unwrap());
    assert!(!app.merge_account_sync_item(trust[1].clone()).await.unwrap());
    // 別の account の profile は採らない。
    let other = AccountSyncItem::profile(&profile_at(&generate_keys(), "other", 300)).unwrap();
    assert!(app.merge_account_sync_item(other).await.is_err());
}

// 一方の端末の編集を、もう一方の端末が account の replica から点読して反映する。
#[tokio::test]
async fn one_device_reads_and_merges_the_other_devices_edits() {
    let keys = generate_keys();
    let author = generate_keys().public_key_hex();
    let docs: Arc<dyn DocsSync> = Arc::new(MemoryDocsSync::with_docs_author(ACCOUNT_DOCS_AUTHOR));
    let (first, second) = (device(&keys, docs.clone()), device(&keys, docs));
    for app in [&first, &second] {
        app.start_account_sync().await.unwrap();
    }
    first.set_trust_always_visible(&author, true).await.unwrap();
    first
        .set_my_profile(ProfileInput {
            name: Some("from the first device".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    for key in [
        AccountSyncItemKey::TrustAlwaysVisible {
            author: Pubkey::from(author.as_str()),
        },
        AccountSyncItemKey::Profile,
    ] {
        let item = second
            .read_account_sync_item(&key, DocFetchPolicy::LocalOnly)
            .await
            .unwrap()
            .expect("the other device's item");
        assert!(second.merge_account_sync_item(item).await.unwrap());
    }
    assert_eq!(
        second.list_trust_always_visible().await.unwrap(),
        vec![author]
    );
    assert_eq!(
        second.get_my_profile().await.unwrap().name.as_deref(),
        Some("from the first device")
    );
}

// 1 item の反映の読み出しは、関係の無い item・同じ key の履歴が 10 倍になっても変わらない。
#[tokio::test]
async fn one_item_read_does_not_grow_with_other_items_or_history() {
    let keys = generate_keys();
    let target = generate_keys().public_key_hex();
    let mut reads = Vec::new();
    for scale in [1, 10] {
        let docs = Arc::new(CountingDocsSync::with_docs_author(ACCOUNT_DOCS_AUTHOR));
        let app = device(&keys, docs.clone());
        app.start_account_sync().await.unwrap();
        for _ in 0..scale {
            app.set_trust_always_visible(&target, true).await.unwrap();
            app.set_trust_always_visible(&target, false).await.unwrap();
            app.set_trust_always_visible(&generate_keys().public_key_hex(), true)
                .await
                .unwrap();
        }
        docs.reset_records_returned();
        docs.clear_queries().await;
        app.read_account_sync_item(
            &AccountSyncItemKey::TrustAlwaysVisible {
                author: Pubkey::from(target.as_str()),
            },
            DocFetchPolicy::LocalOnly,
        )
        .await
        .unwrap()
        .expect("the latest edit");
        reads.push((docs.records_returned(), docs.queries().await.len()));
    }
    assert_eq!(reads[0], reads[1], "{reads:?}");
    assert_eq!(reads[0].0, 1, "{reads:?}");
}

// 旧版の端末内の設定の取り込みは、この端末だけで採用し、他の端末の新しい編集を account の replica で上書きしない。
#[tokio::test]
async fn importing_a_legacy_setting_does_not_overwrite_a_newer_edit_on_the_replica() {
    let keys = generate_keys();
    let author = generate_keys().public_key_hex();
    let docs: Arc<dyn DocsSync> = Arc::new(MemoryDocsSync::with_docs_author(ACCOUNT_DOCS_AUTHOR));
    let (edited, upgraded, third) = (
        device(&keys, docs.clone()),
        device(&keys, docs.clone()),
        device(&keys, docs),
    );
    for app in [&edited, &upgraded, &third] {
        app.start_account_sync().await.unwrap();
    }
    edited
        .set_trust_always_visible(&author, true)
        .await
        .unwrap();
    edited
        .set_trust_always_visible(&author, false)
        .await
        .unwrap();
    upgraded.import_trust_always_visible(&author).await.unwrap();
    assert_eq!(
        upgraded.list_trust_always_visible().await.unwrap(),
        vec![author.clone()]
    );
    let item = third
        .read_account_sync_item(
            &AccountSyncItemKey::TrustAlwaysVisible {
                author: Pubkey::from(author.as_str()),
            },
            DocFetchPolicy::LocalOnly,
        )
        .await
        .unwrap()
        .expect("the newer edit");
    assert_eq!(item.value, None, "the replica keeps the newer tombstone");
}
