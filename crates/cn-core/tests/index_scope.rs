//! #413 ingestion scope 管理 state の Postgres integration テスト（ADR 0025 §2.2 / §6.3）。
//!
//! `KUKURI_CN_RUN_INTEGRATION_TESTS=1` のときだけ実 DB に接続して実行する。
//! - supported topic scope ゲート（supported set 内 / 外の判定）。
//! - user indexing request の承認（approve で supported set に入る多段ゲートの接続）。
//! - private channel capability（channel secret）の at-rest 暗号保存・取得・失効。

use anyhow::Result;
use kukuri_cn_core::{
    ChannelSecretCipher, ChannelSecretConflict, IndexScopeKind, IndexingRequestStatus,
    TestDatabase, add_supported_topic, approve_indexing_request, connect_postgres,
    get_channel_secret, initialize_database, insert_indexing_request, is_topic_supported,
    list_channel_secrets, list_indexing_requests, list_indexing_requests_for_requester,
    list_supported_topics, register_channel_secret, register_channel_secret_with_epoch,
    reject_indexing_request, remove_channel_secret, remove_supported_topic,
    revoke_indexing_request, rotate_channel_secret_epoch, upsert_channel_secret,
};

const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const TEST_CIPHER_KEY: &str = "integration-test-channel-secret-encryption-key-0123456789";

fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

fn cipher() -> ChannelSecretCipher {
    ChannelSecretCipher::from_key_material(TEST_CIPHER_KEY).expect("cipher")
}

#[tokio::test]
async fn index_scope_limited_to_operator_supported_topics() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_scope").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    // 追加前は supported ではない（scope ゲートが拒否する）。
    assert!(!is_topic_supported(&pool, IndexScopeKind::PublicTopic, "rust").await?);

    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    assert!(is_topic_supported(&pool, IndexScopeKind::PublicTopic, "rust").await?);

    // supported set 外の topic は拒否される（index_rejects_topic_outside_supported_set）。
    assert!(!is_topic_supported(&pool, IndexScopeKind::PublicTopic, "golang").await?);

    // 除去すると再び supported ではなくなる（de-index の前提）。
    assert!(remove_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?);
    assert!(!is_topic_supported(&pool, IndexScopeKind::PublicTopic, "rust").await?);

    database.cleanup().await
}

#[tokio::test]
async fn index_admits_approved_user_indexing_request() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_request").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    // request は index を保証しない: pending で入り supported にはならない。
    let request =
        insert_indexing_request(&pool, "npub-requester", IndexScopeKind::PublicTopic, "rust")
            .await?;
    assert_eq!(request.status, IndexingRequestStatus::Pending);
    assert!(!is_topic_supported(&pool, IndexScopeKind::PublicTopic, "rust").await?);

    let pending =
        list_indexing_requests(&pool, Some(IndexingRequestStatus::Pending), 50, 0).await?;
    assert_eq!(pending.len(), 1);

    // operator が承認すると supported set に入る（request → 承認 → supported の接続）。
    let approved = approve_indexing_request(&pool, request.id.as_str())
        .await?
        .expect("request exists");
    assert_eq!(approved.status, IndexingRequestStatus::Approved);
    assert!(is_topic_supported(&pool, IndexScopeKind::PublicTopic, "rust").await?);

    database.cleanup().await
}

#[tokio::test]
async fn rejected_request_does_not_become_supported() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_reject").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let request = insert_indexing_request(
        &pool,
        "npub-requester",
        IndexScopeKind::PrivateChannel,
        "secret-room",
    )
    .await?;
    let rejected = reject_indexing_request(&pool, request.id.as_str())
        .await?
        .expect("request exists");
    assert_eq!(rejected.status, IndexingRequestStatus::Rejected);
    assert!(!is_topic_supported(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?);

    database.cleanup().await
}

#[tokio::test]
async fn duplicate_request_is_idempotent() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_dupe").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let first =
        insert_indexing_request(&pool, "npub-a", IndexScopeKind::PublicTopic, "rust").await?;
    let second =
        insert_indexing_request(&pool, "npub-a", IndexScopeKind::PublicTopic, "rust").await?;
    assert_eq!(first.id, second.id);
    let all = list_indexing_requests(&pool, None, 50, 0).await?;
    assert_eq!(all.len(), 1);

    database.cleanup().await
}

#[tokio::test]
async fn index_private_channel_requires_submitted_channel_secret() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_channel_secret").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    let cipher = cipher();

    // secret 未登録の channel は capability が取得できない（secret 無しでは sync/index できない）。
    assert!(
        get_channel_secret(&pool, &cipher, "secret-room")
            .await?
            .is_none()
    );

    // indexing リクエスト＝secret 送信で capability を登録する。
    let secret_hex = hex::encode([42u8; 32]);
    upsert_channel_secret(&pool, &cipher, "secret-room", secret_hex.as_str()).await?;

    let loaded = get_channel_secret(&pool, &cipher, "secret-room")
        .await?
        .expect("secret registered");
    assert_eq!(loaded.namespace_secret_hex, secret_hex);
    assert_eq!(loaded.channel_id, "secret-room");

    // 起動時復元のため列挙できる。
    let all = list_channel_secrets(&pool, &cipher).await?;
    assert_eq!(all.len(), 1);

    // 失効させると capability が消える（sync 停止 + de-index の前提）。
    assert!(remove_channel_secret(&pool, "secret-room").await?);
    assert!(
        get_channel_secret(&pool, &cipher, "secret-room")
            .await?
            .is_none()
    );

    database.cleanup().await
}

#[tokio::test]
async fn register_channel_secret_is_first_writer_wins() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_first_writer").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    let cipher = cipher();

    let first_secret = hex::encode([1u8; 32]);
    let attacker_secret = hex::encode([2u8; 32]);

    // 最初の登録は成功する。
    register_channel_secret(&pool, &cipher, "secret-room", first_secret.as_str()).await?;
    // 同一 secret の再提示は冪等（no-op、成功）。
    register_channel_secret(&pool, &cipher, "secret-room", first_secret.as_str()).await?;

    // 別 secret での上書き（乗っ取り）は拒否される。
    let err = register_channel_secret(&pool, &cipher, "secret-room", attacker_secret.as_str())
        .await
        .expect_err("second writer with different secret must be rejected");
    assert!(err.downcast_ref::<ChannelSecretConflict>().is_some());

    // 既存の capability は上書きされず維持される。
    let stored = get_channel_secret(&pool, &cipher, "secret-room")
        .await?
        .expect("secret registered");
    assert_eq!(stored.namespace_secret_hex, first_secret);

    register_channel_secret_with_epoch(
        &pool,
        &cipher,
        "secret-room",
        "epoch-1",
        first_secret.as_str(),
    )
    .await?;
    let epoch: Option<String> = sqlx::query_scalar(
        "SELECT epoch_id FROM cn_index.channel_secrets WHERE channel_id = 'secret-room'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(epoch.as_deref(), Some("epoch-1"));
    let err = register_channel_secret_with_epoch(
        &pool,
        &cipher,
        "secret-room",
        "epoch-2",
        first_secret.as_str(),
    )
    .await
    .expect_err("another epoch must not replace the registered one");
    assert!(err.downcast_ref::<ChannelSecretConflict>().is_some());
    upsert_channel_secret(&pool, &cipher, "secret-room", attacker_secret.as_str()).await?;
    let epoch: Option<String> = sqlx::query_scalar(
        "SELECT epoch_id FROM cn_index.channel_secrets WHERE channel_id = 'secret-room'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        epoch, None,
        "operator replacement invalidates the old epoch reader"
    );

    database.cleanup().await
}

#[tokio::test]
async fn epoch_rollover_requires_the_existing_request_and_capability() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_epoch_rollover").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    let cipher = cipher();
    let old = hex::encode([1_u8; 32]);
    let next = hex::encode([2_u8; 32]);
    register_channel_secret_with_epoch(&pool, &cipher, "room", "e1", &old).await?;
    assert!(
        rotate_channel_secret_epoch(&pool, &cipher, "owner", "room", ("e1", &old), ("e2", &next))
            .await
            .is_err()
    );
    insert_indexing_request(&pool, "owner", IndexScopeKind::PrivateChannel, "room").await?;
    assert!(
        rotate_channel_secret_epoch(
            &pool,
            &cipher,
            "owner",
            "room",
            ("e1", &next),
            ("e2", &next)
        )
        .await
        .is_err()
    );
    rotate_channel_secret_epoch(&pool, &cipher, "owner", "room", ("e1", &old), ("e2", &next))
        .await?;
    rotate_channel_secret_epoch(&pool, &cipher, "owner", "room", ("e1", &old), ("e2", &next))
        .await?;
    let stored = get_channel_secret(&pool, &cipher, "room").await?.unwrap();
    assert_eq!(stored.epoch_id.as_deref(), Some("e2"));
    assert_eq!(stored.namespace_secret_hex, next);
    assert!(
        rotate_channel_secret_epoch(&pool, &cipher, "owner", "room", ("e1", &old), ("e3", &old))
            .await
            .is_err()
    );
    assert!(revoke_indexing_request(&pool, "owner", IndexScopeKind::PrivateChannel, "room").await?);
    assert!(get_channel_secret(&pool, &cipher, "room").await?.is_none());
    database.cleanup().await
}

#[tokio::test]
async fn channel_secret_is_encrypted_at_rest() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_atrest").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;
    let cipher = cipher();

    let secret_hex = hex::encode([7u8; 32]);
    upsert_channel_secret(&pool, &cipher, "secret-room", secret_hex.as_str()).await?;

    // DB 列には平文 secret が残らない（ciphertext のみ）。
    let ciphertext: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM cn_index.channel_secrets WHERE channel_id = $1")
            .bind("secret-room")
            .fetch_one(&pool)
            .await?;
    assert_ne!(ciphertext, secret_hex.as_bytes());

    // 別鍵では復号できない（鍵は runtime 供給で DB には無い）。
    let other =
        ChannelSecretCipher::from_key_material("a-completely-different-channel-secret-key-abcdef")
            .unwrap();
    assert!(
        get_channel_secret(&pool, &other, "secret-room")
            .await
            .is_err()
    );

    database.cleanup().await
}

#[tokio::test]
async fn supported_topics_listed_for_startup_restore() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_restore").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;
    add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?;

    let topics = list_supported_topics(&pool).await?;
    assert_eq!(topics.len(), 2);
    assert!(
        topics
            .iter()
            .any(|t| t.kind == IndexScopeKind::PublicTopic && t.id == "rust")
    );
    assert!(
        topics
            .iter()
            .any(|t| t.kind == IndexScopeKind::PrivateChannel && t.id == "secret-room")
    );

    database.cleanup().await
}

#[tokio::test]
async fn requester_listing_returns_own_requests_only_including_rejected() -> Result<()> {
    // #975: 自分の申請一覧は requester で絞り、他利用者の行を含まず、却下行を含む。
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-core index scope test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_index_own_list").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    initialize_database(&pool).await?;

    let mine_rust =
        insert_indexing_request(&pool, "npub-me", IndexScopeKind::PublicTopic, "rust").await?;
    let mine_room =
        insert_indexing_request(&pool, "npub-me", IndexScopeKind::PrivateChannel, "room").await?;
    insert_indexing_request(&pool, "npub-other", IndexScopeKind::PublicTopic, "golang").await?;
    reject_indexing_request(&pool, mine_room.id.as_str()).await?;

    let mine = list_indexing_requests_for_requester(&pool, "npub-me").await?;
    let mut ids: Vec<&str> = mine.iter().map(|request| request.id.as_str()).collect();
    ids.sort_unstable();
    let mut expected = vec![mine_rust.id.as_str(), mine_room.id.as_str()];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert!(
        mine.iter()
            .all(|request| request.requester_pubkey == "npub-me")
    );
    let rejected = mine
        .iter()
        .find(|request| request.id == mine_room.id)
        .expect("rejected row remains listed");
    assert_eq!(rejected.status, IndexingRequestStatus::Rejected);
    assert!(rejected.decided_at.is_some());

    assert!(
        list_indexing_requests_for_requester(&pool, "npub-nobody")
            .await?
            .is_empty()
    );
    assert!(
        list_indexing_requests_for_requester(&pool, " ")
            .await
            .is_err()
    );

    database.cleanup().await
}
