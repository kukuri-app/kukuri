//! アカウントの署名鍵から導出した docs author で、著者の record を読む(ADR 0053、Issue #1258)。
//!
//! 同じ key に検証に通らない record を上限(8 件)を超えて積まれても、著者の docs author と key の組の読み出しで、
//! 著者の投稿と取り下げを反映する。Issue #1258 の TR-3〜TR-6・TR-8・TR-10・TR-11、AC-3〜AC-5、INVAR-1〜4 に対応する。
//! 旧 record(tag も手がかりも無い)の契約(TR-7)は、`hydration_integrity_contract.rs` と
//! `withdrawal_record_selection.rs` の上限つきの読み出しの test が固定している。

use super::hydration_integrity::write_object_entries;
use super::shadowing_docs::{ShadowingDocsSync, app_over_docs, honest_header, shadow_docs_author};
use super::*;

/// 著者の docs author の id。shadow の名義(`shadow_docs_author(n)`)より後ろに並ぶ。
fn author_docs_author() -> String {
    "f0".repeat(32)
}

const WORDS: &str = "words of a post that declares its docs author";

struct Fixture {
    docs_sync: Arc<ShadowingDocsSync>,
    app: AppService,
    store: Arc<MemoryStore>,
    topic: TopicId,
    replica: ReplicaId,
    author_keys: KukuriKeys,
    post: KukuriEnvelope,
    envelope_key: String,
    withdrawal_key: String,
}

/// 著者の docs author を tag で申告した投稿を、その docs author の名義で docs に書く(まだ反映しない)。
async fn fixture(name: &str) -> Fixture {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author(
        author_docs_author(),
    ));
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new(format!("kukuri:topic:docs-author-{name}").as_str());
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let post = build_post_envelope_with_docs_author(
        &author_keys,
        &topic,
        PayloadRef::InlineText { text: WORDS.into() },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        Vec::new(),
        Some(author_docs_author().as_str()),
    )
    .expect("post envelope");
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&post),
        &honest_header(&post),
    )
    .await;
    let envelope_key = stable_key("objects", &format!("{}/envelope", post.id.as_str()));
    let withdrawal_key = stable_key("withdrawals", &format!("{}/state", post.id.as_str()));
    Fixture {
        docs_sync,
        app,
        store,
        topic,
        replica,
        author_keys,
        post,
        envelope_key,
        withdrawal_key,
    }
}

impl Fixture {
    /// key に、検証に通らない record を `count` 件、著者の record より前に積む。
    async fn flood(&self, key: &str, count: usize) {
        for index in 0..count {
            self.docs_sync
                .shadow(key, serde_json::json!({ "garbage": index }))
                .await;
        }
    }

    async fn project_post(&self) {
        assert!(
            hydrate_object_in_topic_with_hint(
                &self.app.services,
                self.topic.as_str(),
                &self.replica,
                &self.post.id,
                Some(author_docs_author().as_str()),
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("project the post")
        );
    }

    async fn write_withdrawal(&self) -> KukuriEnvelope {
        let withdrawal = build_post_withdrawal_envelope(
            &self.author_keys,
            &self.post,
            1,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )
        .expect("withdrawal envelope");
        self.docs_sync
            .apply_doc_op(
                &self.replica,
                DocOp::SetJson {
                    key: self.withdrawal_key.clone(),
                    value: serde_json::to_value(&withdrawal).expect("withdrawal json"),
                },
            )
            .await
            .expect("write the withdrawal");
        withdrawal
    }

    async fn row(&self) -> Option<kukuri_store::ObjectProjectionRow> {
        ObjectProjectionStore::get_object_projection(self.store.as_ref(), &self.post.id)
            .await
            .expect("projection")
    }

    async fn withdrawal_row(&self) -> Option<kukuri_store::PostWithdrawalRow> {
        kukuri_store::PostWithdrawalStore::get_post_withdrawal(self.store.as_ref(), &self.post.id)
            .await
            .expect("withdrawal row")
    }

    async fn assert_withdrawn(&self, withdrawal: &KukuriEnvelope) {
        assert_eq!(
            self.row().await.and_then(|row| row.content).as_deref(),
            Some(""),
            "the body of a withdrawn post stayed visible behind a flooded withdrawal key"
        );
        let row = self
            .withdrawal_row()
            .await
            .expect("the author's withdrawal was not applied");
        assert_eq!(row.withdrawal_envelope_id, withdrawal.id);
    }
}

/// 上限を超える数(この数の不正な record が先に並ぶと、key だけの上限つきの読み出しでは著者の record に届かない)。
const FLOOD: usize = MAX_WITHDRAWAL_RECORDS_PER_OBJECT + 1;

// TR-5 / AC-4: envelope の key を上限を超えて埋められても、docs の event の docs author を手がかりに投稿を反映する。
// 修正前(key だけの上限つきの読み出し)は、この投稿は反映されない。
#[tokio::test]
async fn doc_event_projects_a_post_behind_a_flooded_envelope_key() {
    let fixture = fixture("event-post").await;
    fixture.flood(fixture.envelope_key.as_str(), FLOOD).await;

    // 手がかりが無い読み出し(旧 record と同じ扱い)では、上限つきの読み出しが著者の record に届かない。
    assert!(
        !hydrate_object_in_topic(
            &fixture.app.services,
            fixture.topic.as_str(),
            &fixture.replica,
            &fixture.post.id,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate without a hint")
    );

    let applied = hydrate_subscription_doc_event(
        &fixture.app.services,
        fixture.topic.as_str(),
        &fixture.replica,
        fixture.envelope_key.as_str(),
        Some(author_docs_author().as_str()),
    )
    .await
    .expect("doc event");
    assert_eq!(applied, 1);
    let row = fixture.row().await.expect("row");
    assert_eq!(row.content.as_deref(), Some(WORDS));
    assert_eq!(row.author_pubkey, fixture.author_keys.public_key_hex());
    assert_eq!(
        row.source_docs_author,
        Some(author_docs_author()),
        "the row keeps the docs author that the author declared in the signed tag"
    );
}

// TR-5 / AC-4: hint の docs author でも同じ。
#[tokio::test]
async fn hint_projects_a_post_behind_a_flooded_envelope_key() {
    let fixture = fixture("hint-post").await;
    fixture.flood(fixture.envelope_key.as_str(), FLOOD).await;
    let applied = hydrate_subscription_hint(
        &fixture.app.services,
        fixture.topic.as_str(),
        &fixture.replica,
        &GossipHint::TopicObjectsChanged {
            topic_id: fixture.topic.clone(),
            objects: vec![HintObjectRef {
                object_id: fixture.post.id.as_str().to_string(),
                object_kind: "post".into(),
                docs_author: Some(author_docs_author()),
            }],
        },
    )
    .await
    .expect("hint");
    assert_eq!(applied, 1);
    assert_eq!(
        fixture.row().await.and_then(|row| row.content).as_deref(),
        Some(WORDS)
    );
}

// TR-6 / INVAR-3: 手がかりが偽でも、検証に通らない値は反映しない。上限つきの読み出しへ落ちて、著者の投稿を反映する。
#[tokio::test]
async fn false_docs_author_hint_never_projects_an_unverified_value() {
    let fixture = fixture("false-hint").await;
    // 別の投稿の正しい署名の envelope を、この key に、別の名義(shadow の 0 番)で置く。
    let other = super::hydration_integrity::signed_post(
        &generate_keys(),
        &fixture.topic,
        "an envelope of another post under this key",
        ObjectVisibility::Public,
        None,
    );
    fixture
        .docs_sync
        .shadow(
            fixture.envelope_key.as_str(),
            serde_json::to_value(&other).expect("other envelope json"),
        )
        .await;
    for hint in [
        shadow_docs_author(0),
        "ab".repeat(32),
        "not a docs author id".to_string(),
    ] {
        ObjectProjectionStore::rebuild_object_projections(fixture.store.as_ref(), Vec::new())
            .await
            .expect("clear projection");
        assert!(
            hydrate_object_in_topic_with_hint(
                &fixture.app.services,
                fixture.topic.as_str(),
                &fixture.replica,
                &fixture.post.id,
                Some(hint.as_str()),
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("hydrate with a false hint"),
            "{hint}"
        );
        let row = fixture.row().await.expect("row");
        assert_eq!(row.content.as_deref(), Some(WORDS));
        assert_eq!(row.author_pubkey, fixture.author_keys.public_key_hex());
    }

    // 偽の手がかりに加えて key も上限を超えて埋められていれば、反映しない(何も出さない側に倒れる)。
    ObjectProjectionStore::rebuild_object_projections(fixture.store.as_ref(), Vec::new())
        .await
        .expect("clear projection");
    fixture.flood(fixture.envelope_key.as_str(), FLOOD).await;
    assert!(
        !hydrate_object_in_topic_with_hint(
            &fixture.app.services,
            fixture.topic.as_str(),
            &fixture.replica,
            &fixture.post.id,
            Some(shadow_docs_author(0).as_str()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("hydrate with a false hint and a flooded key")
    );
    assert!(fixture.row().await.is_none());
}

// TR-3 / AC-3(取り下げの key を読む全入口): 取り下げの key を上限を超えて埋められても、著者の取り下げを反映する。
// 修正前(key だけの上限つきの読み出し)は、どの入口でも本文が残る。
#[tokio::test]
async fn every_entry_applies_a_withdrawal_behind_a_flooded_withdrawal_key() {
    for entry in ["object", "event", "hint", "background"] {
        let fixture = fixture(format!("withdrawal-{entry}").as_str()).await;
        fixture.project_post().await;
        let withdrawal = fixture.write_withdrawal().await;
        fixture.flood(fixture.withdrawal_key.as_str(), FLOOD).await;
        match entry {
            // 投稿の個別反映。検証済みの envelope の tag から docs author を得る。
            "object" => {
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
            }
            // 取り下げの event。手がかりが無くても、反映済みの行の列から docs author を得る。
            "event" => {
                let applied = hydrate_subscription_event(
                    &fixture.app.services,
                    fixture.topic.as_str(),
                    &fixture.replica,
                    fixture.withdrawal_key.as_str(),
                )
                .await
                .expect("event");
                assert_eq!(applied, 1);
            }
            "hint" => {
                let applied = hydrate_subscription_hint(
                    &fixture.app.services,
                    fixture.topic.as_str(),
                    &fixture.replica,
                    &GossipHint::TopicObjectsChanged {
                        topic_id: fixture.topic.clone(),
                        objects: vec![HintObjectRef {
                            object_id: fixture.post.id.as_str().to_string(),
                            object_kind: "post_withdrawal".into(),
                            docs_author: None,
                        }],
                    },
                )
                .await
                .expect("hint");
                assert_eq!(applied, 1);
            }
            // TR-10: 保存済みの行(再起動後と同じ状態)を、view の生成からの背景の確認で伏せる。
            _ => {
                fixture
                    .app
                    .schedule_withdrawal_check(fixture.topic.as_str(), &fixture.post.id);
                timeout(Duration::from_secs(5), async {
                    while fixture.withdrawal_row().await.is_none() {
                        sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("the background check did not apply the withdrawal");
            }
        }
        fixture.assert_withdrawn(&withdrawal).await;
    }
}

// 旧 record の投稿(tag なし)の取り下げでも、取り下げを書いた docs author の手がかり(event・hint)があれば、
// 上限を超えて埋められた key から読める。手がかりは署名が無いが、読んだ取り下げは対象の著者との一致まで検証する。
#[tokio::test]
async fn writer_hint_applies_a_withdrawal_of_a_post_without_the_tag() {
    let docs_sync = Arc::new(ShadowingDocsSync::with_account_docs_author(
        author_docs_author(),
    ));
    let (app, store) = app_over_docs(docs_sync.clone());
    let topic = TopicId::new("kukuri:topic:docs-author-writer-hint");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let post = super::hydration_integrity::signed_post(
        &author_keys,
        &topic,
        WORDS,
        ObjectVisibility::Public,
        None,
    );
    assert_eq!(post.docs_author(), None);
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
            DocFetchPolicy::LocalOnly
        )
        .await
        .expect("project")
    );
    let withdrawal = build_post_withdrawal_envelope(
        &author_keys,
        &post,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("withdrawal");
    let withdrawal_key = stable_key("withdrawals", &format!("{}/state", post.id.as_str()));
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
    for index in 0..FLOOD {
        docs_sync
            .shadow(
                withdrawal_key.as_str(),
                serde_json::json!({ "garbage": index }),
            )
            .await;
    }
    let applied = hydrate_subscription_doc_event(
        &app.services,
        topic.as_str(),
        &replica,
        withdrawal_key.as_str(),
        Some(author_docs_author().as_str()),
    )
    .await
    .expect("event");
    assert_eq!(applied, 1);
    assert_eq!(
        ObjectProjectionStore::get_object_projection(store.as_ref(), &post.id)
            .await
            .expect("projection")
            .and_then(|row| row.content)
            .as_deref(),
        Some("")
    );
}

// TR-4 / INVAR-1・2: docs author が合うことは検証の代わりにしない。第三者が自分の名義で置いた「正しく署名された」
// 取り下げは、手がかりがそれを指していても反映しない。不正な record をいくつ積んでも、他人の投稿は伏せられない。
#[tokio::test]
async fn withdrawal_written_by_another_docs_author_does_not_hide_the_post() {
    let fixture = fixture("foreign-withdrawal").await;
    fixture.project_post().await;
    // 攻撃者の鍵で正しく署名し、対象の id と著者を申告する取り下げ。
    let attacker_keys = generate_keys();
    let attacker_post = super::hydration_integrity::signed_post(
        &attacker_keys,
        &fixture.topic,
        "a post of the attacker",
        ObjectVisibility::Public,
        None,
    );
    let attacker_withdrawal = build_post_withdrawal_envelope(
        &attacker_keys,
        &attacker_post,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        None,
    )
    .expect("withdrawal of the attacker's post");
    let claim = |value: &str| {
        value
            .replace(attacker_post.id.as_str(), fixture.post.id.as_str())
            .replace(attacker_post.pubkey.as_str(), fixture.post.pubkey.as_str())
    };
    let forged = kukuri_core::sign_envelope_json(
        &attacker_keys,
        "post_withdrawal",
        attacker_withdrawal
            .tags
            .iter()
            .map(|tag| tag.iter().map(|value| claim(value)).collect())
            .collect(),
        &serde_json::from_str::<serde_json::Value>(claim(&attacker_withdrawal.content).as_str())
            .expect("withdrawal content"),
    )
    .expect("withdrawal signed by another key");
    forged.verify().expect("the signature itself is valid");
    // 著者の名義の record は無い。攻撃者の名義(shadow の 0 番)の record と、不正な record だけがある。
    fixture
        .docs_sync
        .shadow(
            fixture.withdrawal_key.as_str(),
            serde_json::to_value(&forged).expect("forged json"),
        )
        .await;
    fixture
        .flood(fixture.withdrawal_key.as_str(), FLOOD * 4)
        .await;

    let attacker_author = shadow_docs_author(0);
    for writer in [None, Some(attacker_author.as_str())] {
        let outcome = hydrate_post_withdrawal_for_object_with_hints(
            fixture.docs_sync.as_ref(),
            fixture.store.as_ref(),
            &fixture.replica,
            &fixture.post.id,
            WithdrawalReadHints {
                target_docs_author: None,
                writer_docs_author: writer,
            },
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("withdrawal check");
        assert_eq!(outcome, Some(PostWithdrawalHydration::Invalid));
    }
    assert!(fixture.withdrawal_row().await.is_none());
    assert_eq!(
        fixture.row().await.and_then(|row| row.content).as_deref(),
        Some(WORDS),
        "records that do not verify must not hide a post of another author"
    );
}

// TR-8 / AC-5: 読む docs の record 数と読み出しの回数は、その key に積まれた record 数に依存しない。
#[tokio::test]
async fn reads_do_not_grow_with_the_records_piled_on_the_keys() {
    let mut reads = Vec::new();
    for flood in [FLOOD, FLOOD * 10] {
        let fixture = fixture(format!("reads-{flood}").as_str()).await;
        fixture.flood(fixture.envelope_key.as_str(), flood).await;
        let withdrawal = fixture.write_withdrawal().await;
        fixture.flood(fixture.withdrawal_key.as_str(), flood).await;
        fixture.docs_sync.author_reads.lock().await.clear();
        fixture.docs_sync.bounded_reads.lock().await.clear();

        fixture.project_post().await;
        fixture.assert_withdrawn(&withdrawal).await;
        reads.push((
            fixture.docs_sync.author_reads.lock().await.len(),
            fixture.docs_sync.bounded_reads.lock().await.clone(),
        ));
    }
    assert_eq!(reads[0], reads[1], "reads must not depend on the flood");
    // 投稿の envelope、取り下げ、取り下げの検証に使う対象の envelope を、docs author と key の組で 1 件ずつ。
    // key だけの上限つきの読み出しへは落ちない。
    assert_eq!(reads[0], (3, Vec::new()));
}

// TR-11 / INVAR-4: docs author と key の組の読み出しの失敗はエラー。「取り下げなし」として本文つきの行を作らない。
#[tokio::test]
async fn read_failure_by_docs_author_is_an_error() {
    let fixture = fixture("io-failure").await;
    fixture.write_withdrawal().await;
    *fixture.docs_sync.failing_key.lock().await = Some(fixture.withdrawal_key.clone());
    assert!(
        hydrate_object_in_topic_with_hint(
            &fixture.app.services,
            fixture.topic.as_str(),
            &fixture.replica,
            &fixture.post.id,
            Some(author_docs_author().as_str()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .is_err(),
        "a docs read failure must not be treated as the absence of a withdrawal"
    );
    assert!(fixture.row().await.is_none());
}

// AC-3・AC-4(書く側): docs author を持つ docs へ投稿すると、envelope の tag・hint・行に docs author が入る。
// docs author を持たない docs では、tag も手がかりも付けない(旧 record と同じ形)。
#[tokio::test]
async fn created_post_declares_the_local_docs_author() {
    for docs_author in [Some(author_docs_author()), None] {
        let store = Arc::new(MemoryStore::default());
        let docs_sync: Arc<MemoryDocsSync> = Arc::new(match &docs_author {
            Some(id) => MemoryDocsSync::with_docs_author(id.clone()),
            None => MemoryDocsSync::default(),
        });
        let hints = Arc::new(TrackingHintTransport::default());
        let app = app_service_from_dependencies(
            store.clone(),
            store.clone(),
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            hints.clone(),
            docs_sync.clone(),
            Arc::new(MemoryBlobService::default()),
            generate_keys(),
        );
        let topic = TopicId::new("kukuri:topic:docs-author-created");
        let mut published = hints.hint_sender(&topic).await.subscribe();
        let object_id = app
            .create_post(topic.as_str(), "hello", None)
            .await
            .expect("create post");

        let replica = topic_replica_id(topic.as_str());
        let record = docs_sync
            .query_replica_with_policy(
                &replica,
                DocQuery::Exact(stable_key("objects", &format!("{object_id}/envelope"))),
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("envelope record")
            .into_iter()
            .next()
            .expect("envelope");
        let envelope: KukuriEnvelope = serde_json::from_slice(&record.value).expect("envelope");
        envelope.verify().expect("signature verification");
        assert_eq!(envelope.docs_author(), docs_author.as_deref());

        let row = ObjectProjectionStore::get_object_projection(
            store.as_ref(),
            &EnvelopeId::from(object_id.as_str()),
        )
        .await
        .expect("projection")
        .expect("row");
        assert_eq!(row.source_docs_author, docs_author);

        let hint = timeout(Duration::from_secs(5), published.recv())
            .await
            .expect("hint timeout")
            .expect("hint");
        let GossipHint::TopicObjectsChanged { objects, .. } = hint.hint else {
            panic!("unexpected hint");
        };
        assert_eq!(objects[0].docs_author, docs_author);
    }
}
