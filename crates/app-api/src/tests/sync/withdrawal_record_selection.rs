//! 取り下げの key(`withdrawals/<object id>/state`)の record の選び方(#1250)の再現と契約。
//!
//! 同じ key には docs author ごとの record がありうる。検証に通らない record が先に並んでいても、上限内にある
//! 著者の正しい取り下げを反映する。Issue #1250 の TR-1〜TR-6・TR-8、AC-1〜AC-3、INVAR-1〜INVAR-3 に対応する。

use super::hydration_integrity::{signed_post, write_object_entries};
use super::hydration_integrity_contract::{ShadowingDocsSync, app_over_docs, honest_header};
use super::*;

/// 本文つきで反映済みの投稿と、その著者の正しい取り下げ。取り下げの key には、`shadows` が先に並ぶ。
struct ShadowedWithdrawal {
    docs_sync: Arc<ShadowingDocsSync>,
    app: AppService,
    store: Arc<MemoryStore>,
    topic: TopicId,
    replica: ReplicaId,
    post: KukuriEnvelope,
    withdrawal: KukuriEnvelope,
    withdrawal_key: String,
}

const WITHDRAWN_WORDS: &str = "words the author took back";

/// 取り下げとして検証に通らない record を 5 種類作る(読めない JSON、取り下げでない envelope、
/// 署名が合わない取り下げ、別の object を対象とする正しい署名の取り下げ、対象の著者でない鍵が正しく署名した
/// その object の取り下げ)。最後の 1 件が、署名の検証だけでは拒否できない最も強い形。
fn invalid_withdrawal_records(topic: &TopicId, target: &KukuriEnvelope) -> Vec<serde_json::Value> {
    let attacker_keys = generate_keys();
    let attacker_post = signed_post(
        &attacker_keys,
        topic,
        "a post of the attacker",
        ObjectVisibility::Public,
        None,
    );
    let other_withdrawal = build_post_withdrawal_envelope(
        &attacker_keys,
        &attacker_post,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal of another object");
    // 対象の id を指すよう content を書き換える(署名は合わなくなる)。
    let mut forged = other_withdrawal.clone();
    forged.content = forged
        .content
        .replace(attacker_post.id.as_str(), target.id.as_str());
    // 攻撃者の鍵で正しく署名し、対象の id と著者を申告する取り下げ(TR-2)。署名は通るが、署名者が対象の著者と違う。
    let claim = |value: &str| {
        value
            .replace(attacker_post.id.as_str(), target.id.as_str())
            .replace(attacker_post.pubkey.as_str(), target.pubkey.as_str())
    };
    let foreign_signer = kukuri_core::sign_envelope_json(
        &attacker_keys,
        "post_withdrawal",
        other_withdrawal
            .tags
            .iter()
            .map(|tag| tag.iter().map(|value| claim(value)).collect())
            .collect(),
        &serde_json::from_str::<serde_json::Value>(claim(&other_withdrawal.content).as_str())
            .expect("withdrawal content"),
    )
    .expect("withdrawal signed by another key");
    foreign_signer
        .verify()
        .expect("the signature itself is valid");
    assert_ne!(foreign_signer.pubkey, target.pubkey);
    // 取り下げとして読め、この object を対象とする。拒否するのは、対象の著者との照合だけ。
    assert_eq!(
        foreign_signer
            .post_withdrawal_content()
            .expect("withdrawal content")
            .expect("post withdrawal")
            .target_object_id,
        target.id
    );
    vec![
        serde_json::json!({ "not": "a withdrawal" }),
        serde_json::to_value(&attacker_post).expect("post json"),
        serde_json::to_value(&forged).expect("forged json"),
        serde_json::to_value(&other_withdrawal).expect("other withdrawal json"),
        serde_json::to_value(&foreign_signer).expect("foreign signer json"),
    ]
}

async fn shadowed_withdrawal(name: &str, shadows: Vec<serde_json::Value>) -> ShadowedWithdrawal {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new(format!("kukuri:topic:withdrawal-{name}").as_str());
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let post = signed_post(
        &author_keys,
        &topic,
        WITHDRAWN_WORDS,
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&post),
        &honest_header(&post),
    )
    .await;
    // 取り下げが届く前に、本文つきの行が反映されている(修正前に保存された行も同じ状態)。
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate before the withdrawal")
    );
    assert_eq!(
        projected_content(&store, &post.id).await.as_deref(),
        Some(WITHDRAWN_WORDS)
    );

    let withdrawal = build_post_withdrawal_envelope(
        &author_keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("withdrawal envelope");
    let withdrawal_key = stable_key("withdrawals", &format!("{}/state", post.id.as_str()));
    for shadow in shadows {
        docs_sync.shadow(withdrawal_key.as_str(), shadow).await;
    }
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: withdrawal_key.clone(),
                value: serde_json::to_value(&withdrawal).expect("withdrawal json"),
            },
        )
        .await
        .expect("write the withdrawal");
    ShadowedWithdrawal {
        docs_sync,
        app,
        store,
        topic,
        replica,
        post,
        withdrawal,
        withdrawal_key,
    }
}

async fn projected_content(store: &Arc<MemoryStore>, object_id: &EnvelopeId) -> Option<String> {
    ObjectProjectionStore::get_object_projection(store.as_ref(), object_id)
        .await
        .expect("projection")
        .and_then(|row| row.content)
}

async fn stored_withdrawal(
    store: &Arc<MemoryStore>,
    object_id: &EnvelopeId,
) -> Option<kukuri_store::PostWithdrawalRow> {
    kukuri_store::PostWithdrawalStore::get_post_withdrawal(store.as_ref(), object_id)
        .await
        .expect("withdrawal row")
}

async fn assert_withdrawal_applied(fixture: &ShadowedWithdrawal) {
    assert_eq!(
        projected_content(&fixture.store, &fixture.post.id)
            .await
            .as_deref(),
        Some(""),
        "the body of a withdrawn post stayed visible behind invalid withdrawal records"
    );
    let row = stored_withdrawal(&fixture.store, &fixture.post.id)
        .await
        .expect("the author's withdrawal was not applied");
    assert_eq!(row.withdrawal_envelope_id, fixture.withdrawal.id);
    assert_eq!(
        row.target_author_pubkey,
        fixture.post.pubkey.as_str(),
        "the withdrawal row must come from the author's withdrawal"
    );
}

// TR-1 / TR-2 / AC-1(INV-1): key 指定の投稿の反映。修正前の再現(先頭の 1 件だけを読むと失敗する)。
#[tokio::test]
async fn invalid_records_placed_before_the_withdrawal_do_not_cancel_the_withdrawal() {
    let fixture = shadowed_withdrawal("object", Vec::new()).await;
    for shadow in invalid_withdrawal_records(&fixture.topic, &fixture.post) {
        fixture
            .docs_sync
            .shadow(fixture.withdrawal_key.as_str(), shadow)
            .await;
    }
    assert!(
        hydrate_object_in_topic(
            &fixture.app.services,
            fixture.topic.as_str(),
            &fixture.replica,
            &fixture.post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate")
    );
    assert_withdrawal_applied(&fixture).await;
}

// TR-1 / AC-2(INV-2): docs の event(key が `withdrawals/<object id>/state`)。
#[tokio::test]
async fn withdrawal_event_applies_the_withdrawal_behind_invalid_records() {
    let fixture = shadowed_withdrawal("event", Vec::new()).await;
    for shadow in invalid_withdrawal_records(&fixture.topic, &fixture.post) {
        fixture
            .docs_sync
            .shadow(fixture.withdrawal_key.as_str(), shadow)
            .await;
    }
    let applied = hydrate_subscription_event(
        &fixture.app.services,
        fixture.topic.as_str(),
        &fixture.replica,
        fixture.withdrawal_key.as_str(),
    )
    .await
    .expect("event");
    assert_eq!(applied, 1);
    assert_withdrawal_applied(&fixture).await;
}

// TR-1 / AC-2(INV-3): hint(object kind が `post_withdrawal`)。
#[tokio::test]
async fn withdrawal_hint_applies_the_withdrawal_behind_invalid_records() {
    let fixture = shadowed_withdrawal("hint", Vec::new()).await;
    for shadow in invalid_withdrawal_records(&fixture.topic, &fixture.post) {
        fixture
            .docs_sync
            .shadow(fixture.withdrawal_key.as_str(), shadow)
            .await;
    }
    let applied = hydrate_subscription_hint(
        &fixture.app.services,
        fixture.topic.as_str(),
        &fixture.replica,
        &GossipHint::TopicObjectsChanged {
            topic_id: fixture.topic.clone(),
            objects: vec![HintObjectRef {
                object_id: fixture.post.id.as_str().to_string(),
                object_kind: "post_withdrawal".into(),
            }],
        },
    )
    .await
    .expect("hint");
    assert_eq!(applied, 1);
    assert_withdrawal_applied(&fixture).await;
}

// TR-8 / AC-2(INV-4): 本文つきで保存された行は、view の生成からの背景の確認で伏せられる。
#[tokio::test]
async fn background_withdrawal_check_applies_the_withdrawal_behind_invalid_records() {
    let fixture = shadowed_withdrawal("background", Vec::new()).await;
    for shadow in invalid_withdrawal_records(&fixture.topic, &fixture.post) {
        fixture
            .docs_sync
            .shadow(fixture.withdrawal_key.as_str(), shadow)
            .await;
    }
    fixture
        .app
        .schedule_withdrawal_check(fixture.topic.as_str(), &fixture.post.id);
    timeout(Duration::from_secs(5), async {
        while stored_withdrawal(&fixture.store, &fixture.post.id)
            .await
            .is_none()
        {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the background check did not apply the withdrawal");
    assert_withdrawal_applied(&fixture).await;
}

// TR-3 / INVAR-1: 検証に通らない record だけでは、他人の投稿を伏せられない。
#[tokio::test]
async fn invalid_withdrawal_records_alone_do_not_hide_the_post() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:withdrawal-only-invalid");
    let replica = topic_replica_id(topic.as_str());
    let post = signed_post(
        &generate_keys(),
        &topic,
        "still visible",
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&post),
        &honest_header(&post),
    )
    .await;
    let withdrawal_key = stable_key("withdrawals", &format!("{}/state", post.id.as_str()));
    let mut records = invalid_withdrawal_records(&topic, &post);
    // shadow は、その key に record があるときだけ返る。最後の 1 件を key の record として書く。
    let last = records.pop().expect("records");
    for shadow in records {
        docs_sync.shadow(withdrawal_key.as_str(), shadow).await;
    }
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: withdrawal_key.clone(),
                value: last,
            },
        )
        .await
        .expect("write an invalid record");

    assert_eq!(
        hydrate_post_withdrawal_for_object(
            docs_sync.as_ref(),
            store.as_ref(),
            &replica,
            &post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("withdrawal check"),
        Some(PostWithdrawalHydration::Invalid)
    );
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate"),
        "invalid withdrawal records must not stop the post from being projected"
    );
    assert_eq!(
        projected_content(&store, &post.id).await.as_deref(),
        Some("still visible")
    );
    assert!(stored_withdrawal(&store, &post.id).await.is_none());
}

// TR-4 / AC-3: 取り下げの key で調べる record 数は定数。上限を超える数の不正な record の後ろは調べない(best effort)。
#[tokio::test]
async fn withdrawal_records_beyond_the_per_key_limit_are_not_examined() {
    let shadows = (0..MAX_WITHDRAWAL_RECORDS_PER_OBJECT)
        .map(|index| serde_json::json!({ "garbage": index }))
        .collect();
    let fixture = shadowed_withdrawal("per-key-limit", shadows).await;
    fixture.docs_sync.bounded_reads.lock().await.clear();
    assert_eq!(
        hydrate_post_withdrawal_for_object(
            fixture.docs_sync.as_ref(),
            fixture.store.as_ref(),
            &fixture.replica,
            &fixture.post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("withdrawal check"),
        Some(PostWithdrawalHydration::Invalid)
    );
    assert!(
        stored_withdrawal(&fixture.store, &fixture.post.id)
            .await
            .is_none()
    );
    // 読み出しは上限つきで、対象の envelope は候補が無いので読まない。
    assert_eq!(
        *fixture.docs_sync.bounded_reads.lock().await,
        vec![(
            fixture.withdrawal_key.clone(),
            MAX_WITHDRAWAL_RECORDS_PER_OBJECT
        )]
    );
}

// AC-3: 上限ちょうどの位置にある正しい取り下げは反映する。読み出しは key 2 つの上限つきの読み出しだけ。
#[tokio::test]
async fn withdrawal_at_the_last_examined_position_is_applied_with_two_bounded_reads() {
    let shadows = (0..MAX_WITHDRAWAL_RECORDS_PER_OBJECT - 1)
        .map(|index| serde_json::json!({ "garbage": index }))
        .collect();
    let fixture = shadowed_withdrawal("last-position", shadows).await;
    fixture.docs_sync.bounded_reads.lock().await.clear();
    assert_eq!(
        hydrate_post_withdrawal_for_object(
            fixture.docs_sync.as_ref(),
            fixture.store.as_ref(),
            &fixture.replica,
            &fixture.post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("withdrawal check"),
        Some(PostWithdrawalHydration::Applied)
    );
    assert_withdrawal_applied(&fixture).await;
    assert_eq!(
        *fixture.docs_sync.bounded_reads.lock().await,
        vec![
            (
                fixture.withdrawal_key.clone(),
                MAX_WITHDRAWAL_RECORDS_PER_OBJECT
            ),
            (
                stable_key("objects", &format!("{}/envelope", fixture.post.id.as_str())),
                MAX_ENVELOPE_RECORDS_PER_OBJECT
            ),
        ]
    );
}

// TR-5 / INVAR-2: 対象の envelope が未着なら `TargetMissing`。対象が届いた後の反映で伏せた行になる。
#[tokio::test]
async fn withdrawal_behind_invalid_records_waits_for_a_missing_target() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:withdrawal-target-missing");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let post = signed_post(
        &author_keys,
        &topic,
        WITHDRAWN_WORDS,
        ObjectVisibility::Public,
        None,
    );
    let withdrawal = build_post_withdrawal_envelope(
        &author_keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("withdrawal envelope");
    let withdrawal_key = stable_key("withdrawals", &format!("{}/state", post.id.as_str()));
    docs_sync
        .shadow(
            withdrawal_key.as_str(),
            serde_json::json!({ "not": "a withdrawal" }),
        )
        .await;
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: withdrawal_key,
                value: serde_json::to_value(&withdrawal).expect("withdrawal json"),
            },
        )
        .await
        .expect("write the withdrawal");
    assert_eq!(
        hydrate_post_withdrawal_for_object(
            docs_sync.as_ref(),
            store.as_ref(),
            &replica,
            &post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("withdrawal check"),
        Some(PostWithdrawalHydration::TargetMissing)
    );
    assert!(stored_withdrawal(&store, &post.id).await.is_none());

    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&post),
        &honest_header(&post),
    )
    .await;
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate")
    );
    assert_eq!(
        projected_content(&store, &post.id).await.as_deref(),
        Some("")
    );
    assert!(stored_withdrawal(&store, &post.id).await.is_some());
}

// TR-6 / INVAR-3: 取り下げの key の読み出しの失敗はエラー。「取り下げなし」として本文つきの行を作らない。
#[tokio::test]
async fn withdrawal_read_failure_is_an_error_and_does_not_project_the_body() {
    let docs_sync = Arc::new(ShadowingDocsSync::default());
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:withdrawal-io-failure");
    let replica = topic_replica_id(topic.as_str());
    let post = signed_post(
        &generate_keys(),
        &topic,
        "not projected while the withdrawal key cannot be read",
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&post),
        &honest_header(&post),
    )
    .await;
    *docs_sync.failing_key.lock().await = Some(stable_key(
        "withdrawals",
        &format!("{}/state", post.id.as_str()),
    ));
    assert!(
        hydrate_object_in_topic(
            &app.services,
            topic.as_str(),
            &replica,
            &post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .is_err(),
        "a docs read failure must not be treated as the absence of a withdrawal"
    );
    assert!(projected_content(&store, &post.id).await.is_none());
}

// INV-6(#1239 T5a の入口): ページの範囲の照合は、projection に既にある行の取り下げを key 指定で確認する。
// 検証に通らない record が先に並んでいても、上限内にある著者の正しい取り下げを反映する。
#[tokio::test]
async fn range_reconcile_applies_the_withdrawal_behind_invalid_records() {
    let fixture = shadowed_withdrawal("range-reconcile", Vec::new()).await;
    for shadow in invalid_withdrawal_records(&fixture.topic, &fixture.post) {
        fixture
            .docs_sync
            .shadow(fixture.withdrawal_key.as_str(), shadow)
            .await;
    }
    // 照合は時系列の索引から範囲を読む。`write_object_entries` は索引を書かないので、ここで足す。
    fixture
        .docs_sync
        .apply_doc_op(
            &fixture.replica,
            DocOp::SetJson {
                key: stable_key(
                    "indexes/timeline",
                    &format!(
                        "{}/{}",
                        kukuri_core::timeline_sort_key(fixture.post.created_at, &fixture.post.id),
                        fixture.post.id.as_str()
                    ),
                ),
                value: serde_json::json!({}),
            },
        )
        .await
        .expect("write the timeline index entry");

    let hydrated = fixture
        .app
        .reconcile_timeline_range(fixture.topic.as_str(), &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile");
    assert_eq!(
        hydrated, 1,
        "the withdrawal of the projected row is applied"
    );
    assert_withdrawal_applied(&fixture).await;
}
