use super::*;
use anyhow::Result;
use sha2::{Digest, Sha384};
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;

const PRE_METAVERSE_FIXTURE: &str =
    include_str!("../../fixtures/sqlite/pre-metaverse-game-room-columns.fixture");

// 埋め込み migration(LF)を CRLF に変えた場合の checksum。
// かつて存在した CRLF 自己修復(#211、WP-C6 で撤去)が対象にしていた形の
// 不一致で、これを接続時にサイレント修復しないことを固定する。
fn crlf_variant_checksum(sql: &str) -> Vec<u8> {
    let lf_sql = sql.replace("\r\n", "\n").replace('\r', "\n");
    let crlf_sql = lf_sql.replace('\n', "\r\n");
    Vec::from(Sha384::digest(crlf_sql.as_bytes()).as_slice())
}

#[tokio::test]
async fn connect_file_fails_loudly_on_line_ending_variant_migration_checksums() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("store.db");
    let store = SqliteStore::connect_file(&db_path)
        .await
        .expect("initialize sqlite store");
    store.close().await;

    let database_url = format!("sqlite://{}", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reopen sqlite db");
    for version in [20260319000000_i64, 20260319010000_i64] {
        let migration = STORE_MIGRATOR
            .iter()
            .find(|migration| {
                migration.version == version && !migration.migration_type.is_down_migration()
            })
            .expect("embedded store migration");
        let variant_checksum = crlf_variant_checksum(migration.sql.as_ref());
        assert_ne!(
            variant_checksum.as_slice(),
            migration.checksum.as_ref(),
            "CRLF variant checksum must differ from the embedded checksum"
        );
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ?1 WHERE version = ?2")
            .bind(variant_checksum)
            .bind(version)
            .execute(&pool)
            .await
            .expect("rewrite migration checksum to CRLF variant");
    }
    pool.close().await;

    let error = match SqliteStore::connect_file(&db_path).await {
        Ok(_) => panic!("checksum mismatch must fail loudly (no silent repair)"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("was previously applied but has been modified"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn connect_file_migration_failure_is_typed_as_migration_error() {
    // WP-Q2: 起動 Failed 画面のエラー分類を文字列 contains から typed downcast に
    // 移すため、migration 失敗が StoreStartupError::Migration として downcast できることを固定する。
    use crate::StoreStartupError;

    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("store.db");
    let store = SqliteStore::connect_file(&db_path)
        .await
        .expect("initialize sqlite store");
    store.close().await;

    // 適用済み migration の checksum を書き換えて version mismatch を誘発する。
    let database_url = format!("sqlite://{}", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("reopen sqlite db");
    let version = 20260319000000_i64;
    let migration = STORE_MIGRATOR
        .iter()
        .find(|migration| {
            migration.version == version && !migration.migration_type.is_down_migration()
        })
        .expect("embedded store migration");
    sqlx::query("UPDATE _sqlx_migrations SET checksum = ?1 WHERE version = ?2")
        .bind(crlf_variant_checksum(migration.sql.as_ref()))
        .bind(version)
        .execute(&pool)
        .await
        .expect("rewrite migration checksum");
    pool.close().await;

    let error = match SqliteStore::connect_file(&db_path).await {
        Ok(_) => panic!("checksum mismatch must fail"),
        Err(error) => error,
    };
    let typed = error
        .downcast_ref::<StoreStartupError>()
        .expect("startup error must be typed as StoreStartupError");
    assert!(
        matches!(typed, StoreStartupError::Migration(_)),
        "expected Migration variant, got {typed:?}"
    );
    // message は従来と同一(表示・ログ・既存文字列アサーションは不変)。
    assert!(
        format!("{error:#}").contains("was previously applied but has been modified"),
        "migration source message must survive typing: {error:#}"
    );
}

#[tokio::test]
async fn notification_content_labels_migration_backfills_known_objects_and_leaves_unknown_unresolved()
 {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("pre-notification-labels.db");
    materialize_sqlite_fixture(&db_path, 20260901000000)
        .await
        .expect("materialize pre-notification-label schema");

    let database_url = format!("sqlite://{}", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("open pre-notification-label database");
    sqlx::query(
        r#"
        INSERT INTO object_index_cache (
          object_id, topic_id, author_pubkey, created_at, payload_ref_json,
          source_replica_id, source_key, source_envelope_id, derived_at,
          projection_version, content_labels_json
        ) VALUES (
          'adult-object', 'kukuri:topic:migration', 'author', 1, '{}',
          'topic::migration', 'objects/adult-object/state', 'adult-object', 1,
          1, '["adult"]'
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert labeled projection");
    for (notification_id, object_id) in [
        ("known-notification", "adult-object"),
        ("unknown-notification", "missing-object"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO notifications (
              notification_id, recipient_pubkey, kind, actor_pubkey, object_id,
              preview_text, created_at, received_at
            ) VALUES (?1, 'recipient', 'mention', 'actor', ?2, 'preview', 1, 1)
            "#,
        )
        .bind(notification_id)
        .bind(object_id)
        .execute(&pool)
        .await
        .expect("insert legacy notification");
    }
    pool.close().await;

    let migrated = SqliteStore::connect_file(&db_path)
        .await
        .expect("apply notification labels migration");
    let known = sqlx::query_scalar::<_, Option<String>>(
        "SELECT content_labels_json FROM notifications WHERE notification_id = 'known-notification'",
    )
    .fetch_one(migrated.pool())
    .await
    .expect("read known labels");
    let unknown = sqlx::query_scalar::<_, Option<String>>(
        "SELECT content_labels_json FROM notifications WHERE notification_id = 'unknown-notification'",
    )
    .fetch_one(migrated.pool())
    .await
    .expect("read unresolved labels");
    assert_eq!(known.as_deref(), Some("[\"adult\"]"));
    assert_eq!(unknown, None);
}

// #1248: 署名つき envelope と replica を確かめる前に保存された投稿の行(`projection_version` が検証済みの版より小さい行)は、
// migration で消える。bookmark・通知・reaction の表と、検証済みの版の行は残る。
#[tokio::test]
async fn unverified_object_projections_are_dropped_and_other_tables_are_kept() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("pre-verified-projection.db");
    materialize_sqlite_fixture(&db_path, 20260920000000)
        .await
        .expect("materialize the schema before the verified projection version");

    let database_url = format!("sqlite://{}", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("open the database before the migration");
    let verified = crate::VERIFIED_OBJECT_PROJECTION_VERSION;
    for (object_id, version) in [
        ("legacy-v1", 1),
        ("unverified-v2", verified - 1),
        ("verified", verified),
    ] {
        sqlx::query(
            r#"
            INSERT INTO object_index_cache (
              object_id, topic_id, author_pubkey, created_at, payload_ref_json,
              source_replica_id, source_key, source_envelope_id, derived_at,
              projection_version
            ) VALUES (
              ?1, 'kukuri:topic:migration', 'author', 1, '{}',
              'topic::kukuri:topic:migration', 'objects/x/state', ?1, 1, ?2
            )
            "#,
        )
        .bind(object_id)
        .bind(version)
        .execute(&pool)
        .await
        .expect("insert a projection row");
    }
    sqlx::query(
        r#"
        INSERT INTO notifications (
          notification_id, recipient_pubkey, kind, actor_pubkey, object_id,
          preview_text, created_at, received_at
        ) VALUES ('kept-notification', 'recipient', 'mention', 'actor', 'unverified-v2', 'preview', 1, 1)
        "#,
    )
    .execute(&pool)
    .await
    .expect("insert a notification");
    pool.close().await;

    let migrated = SqliteStore::connect_file(&db_path)
        .await
        .expect("apply the migration");
    let remaining = sqlx::query_scalar::<_, String>(
        "SELECT object_id FROM object_index_cache ORDER BY object_id",
    )
    .fetch_all(migrated.pool())
    .await
    .expect("list projection rows");
    assert_eq!(remaining, vec!["verified".to_string()]);
    let notifications = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notifications")
        .fetch_one(migrated.pool())
        .await
        .expect("count notifications");
    assert_eq!(notifications, 1, "other tables must be kept");
}

#[tokio::test]
async fn connect_file_open_failure_is_typed_as_open_error() {
    // 存在しない親ディレクトリ配下のパスは create_if_missing でも作成できず接続失敗する。
    use crate::StoreStartupError;

    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("missing-parent").join("store.db");
    let error = match SqliteStore::connect_file(&db_path).await {
        Ok(_) => panic!("connecting under a missing parent dir must fail"),
        Err(error) => error,
    };
    let typed = error
        .downcast_ref::<StoreStartupError>()
        .expect("startup error must be typed as StoreStartupError");
    assert!(
        matches!(typed, StoreStartupError::Open { .. }),
        "expected Open variant, got {typed:?}"
    );
    assert!(
        format!("{error:#}").contains("failed to connect sqlite database"),
        "open error message must survive typing: {error:#}"
    );
}

#[tokio::test]
async fn connect_file_applies_unfiltered_projection_indexes() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("store.db");
    let expected_indexes = [
        "idx_game_room_cache_topic_updated_all",
        "idx_live_session_cache_topic_started_all",
        "idx_object_index_cache_topic_created_all",
        "idx_object_thread_cache_topic_root_created_all",
    ];

    let store = SqliteStore::connect_file(&db_path)
        .await
        .expect("initialize sqlite store");
    let mut actual_indexes = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name
        FROM sqlite_master
        WHERE type = 'index'
          AND name IN (
            'idx_game_room_cache_topic_updated_all',
            'idx_live_session_cache_topic_started_all',
            'idx_object_index_cache_topic_created_all',
            'idx_object_thread_cache_topic_root_created_all'
          )
        ORDER BY name
        "#,
    )
    .fetch_all(store.pool())
    .await
    .expect("load unfiltered projection indexes");
    actual_indexes.sort();
    assert_eq!(actual_indexes, expected_indexes);
    store.close().await;

    let reopened = SqliteStore::connect_file(&db_path)
        .await
        .expect("reopen sqlite store");
    let mut reopened_indexes = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name
        FROM sqlite_master
        WHERE type = 'index'
          AND name IN (
            'idx_game_room_cache_topic_updated_all',
            'idx_live_session_cache_topic_started_all',
            'idx_object_index_cache_topic_created_all',
            'idx_object_thread_cache_topic_root_created_all'
          )
        ORDER BY name
        "#,
    )
    .fetch_all(reopened.pool())
    .await
    .expect("load unfiltered projection indexes after reopen");
    reopened_indexes.sort();
    assert_eq!(reopened_indexes, expected_indexes);
}

#[tokio::test]
async fn connect_file_migrates_pre_metaverse_game_room_fixture() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("pre-metaverse.db");
    let applied_through = fixture_applied_through(PRE_METAVERSE_FIXTURE);
    materialize_sqlite_fixture(&db_path, applied_through)
        .await
        .expect("materialize old sqlite fixture");

    let database_url = format!("sqlite://{}", db_path.display());
    let pre_migration_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("open old sqlite fixture");
    assert!(
        !sqlite_column_exists(&pre_migration_pool, "game_room_cache", "room_kind")
            .await
            .expect("check old fixture room_kind column"),
        "fixture should represent the pre-metaverse game_room_cache schema"
    );
    pre_migration_pool.close().await;

    let migrated = SqliteStore::connect_file(&db_path)
        .await
        .expect("migrate old sqlite fixture");

    assert!(
        sqlite_column_exists(migrated.pool(), "game_room_cache", "room_kind")
            .await
            .expect("check migrated room_kind column")
    );
    assert!(
        sqlite_column_exists(migrated.pool(), "game_room_cache", "metaverse_json")
            .await
            .expect("check migrated metaverse_json column")
    );
    let index_exists = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT 1
        FROM sqlite_master
        WHERE type = 'index'
          AND name = 'idx_game_room_cache_topic_kind_updated'
        LIMIT 1
        "#,
    )
    .fetch_optional(migrated.pool())
    .await
    .expect("check migrated game room kind index")
    .is_some();
    assert!(index_exists);

    let latest_migration = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM _sqlx_migrations WHERE version = ?1 AND success = true",
    )
    .bind(20260825000000_i64)
    .fetch_optional(migrated.pool())
    .await
    .expect("check latest migration record")
    .is_some();
    assert!(latest_migration);
}

// ---------------------------------------------------------------------------
// WP-S6 T1: migration down の可逆性を固定する characterization テスト。
// 後続 WP-H1(ProjectionStore 分割)の安全網として、
// (1) 全世代に up/down が embed で揃うこと、
// (2) 全適用 → undo(0) → 再適用でスキーマと migration 記録が完全復元されること
// を固定する。期待値は観測した現挙動の生リテラル(世代数 20 など)。
// ---------------------------------------------------------------------------

/// 全世代に ReversibleUp / ReversibleDown が揃っていることを固定する(DB 不要)。
/// down が embed されていない世代は `Migrator::undo` に黙ってスキップされ、
/// round-trip を静かに破壊するため、ここで欠落を即検出する。
#[tokio::test]
async fn all_generations_have_paired_down() {
    use sqlx::migrate::MigrationType;
    use std::collections::BTreeMap;

    // version ごとに (up あり, down あり) を集計する。
    let mut generations: BTreeMap<i64, (bool, bool)> = BTreeMap::new();
    for migration in STORE_MIGRATOR.iter() {
        let entry = generations
            .entry(migration.version)
            .or_insert((false, false));
        match migration.migration_type {
            MigrationType::ReversibleUp => entry.0 = true,
            MigrationType::ReversibleDown => entry.1 = true,
            MigrationType::Simple => panic!(
                "store migration {} must be a reversible up/down pair, found a simple migration",
                migration.version
            ),
        }
    }

    assert_eq!(
        generations.len(),
        25,
        "store migrations must cover exactly 25 generations, found versions: {:?}",
        generations.keys().collect::<Vec<_>>()
    );

    let missing_up: Vec<i64> = generations
        .iter()
        .filter(|(_, (has_up, _))| !has_up)
        .map(|(version, _)| *version)
        .collect();
    assert!(
        missing_up.is_empty(),
        "store migration generations missing an up migration: {missing_up:?}"
    );

    let missing_down: Vec<i64> = generations
        .iter()
        .filter(|(_, (_, has_down))| !has_down)
        .map(|(version, _)| *version)
        .collect();
    assert!(
        missing_down.is_empty(),
        "store migration generations missing a paired down migration: {missing_down:?}"
    );
}

/// 全適用 → undo(0) → run() の full round-trip でスキーマ(全テーブルの
/// pragma_table_info 正規化文字列)と _sqlx_migrations の version 集合が
/// 全適用時と一致することを固定する。down の残置列・欠落 down による
/// up スキップをまとめて検出する。
#[tokio::test]
async fn full_migration_round_trip() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("round-trip.db");
    let store = SqliteStore::connect_file(&db_path)
        .await
        .expect("initialize sqlite store");

    let fully_migrated_snapshot = schema_table_snapshot(store.pool())
        .await
        .expect("snapshot fully migrated schema");

    STORE_MIGRATOR
        .undo(store.pool(), 0)
        .await
        .expect("undo all store migrations");

    let remaining_tables = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name
        FROM sqlite_master
        WHERE type = 'table'
          AND name NOT LIKE 'sqlite_%'
          AND name != '_sqlx_migrations'
        ORDER BY name
        "#,
    )
    .fetch_all(store.pool())
    .await
    .expect("list tables after undo(0)");
    assert_eq!(
        remaining_tables,
        Vec::<String>::new(),
        "undo(0) must drop every store table"
    );

    STORE_MIGRATOR
        .run(store.pool())
        .await
        .expect("re-run all store migrations after undo(0)");

    let replayed_snapshot = schema_table_snapshot(store.pool())
        .await
        .expect("snapshot schema after round trip");
    assert_eq!(
        replayed_snapshot, fully_migrated_snapshot,
        "schema after undo(0) + run() must match the fully migrated schema"
    );

    let applied_versions =
        sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(store.pool())
            .await
            .expect("list applied migration versions");
    let mut expected_versions: Vec<i64> = STORE_MIGRATOR
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .collect();
    expected_versions.sort_unstable();
    expected_versions.dedup();
    assert_eq!(
        applied_versions.len(),
        25,
        "round trip must restore all 25 migration generations"
    );
    assert_eq!(applied_versions, expected_versions);
}

/// 全テーブル(_sqlx_migrations / sqlite 内部テーブル除外)の pragma_table_info を
/// 正規化した文字列 snapshot にする。index は T2(migrations_roundtrip)の範囲。
async fn schema_table_snapshot(pool: &sqlx::Pool<sqlx::Sqlite>) -> Result<String> {
    let table_names = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name
        FROM sqlite_master
        WHERE type = 'table'
          AND name NOT LIKE 'sqlite_%'
          AND name != '_sqlx_migrations'
        ORDER BY name
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut snapshot = String::new();
    for table_name in &table_names {
        snapshot.push_str("table ");
        snapshot.push_str(table_name);
        snapshot.push('\n');
        let columns = sqlx::query("SELECT * FROM pragma_table_info(?1) ORDER BY cid")
            .bind(table_name)
            .fetch_all(pool)
            .await?;
        for column in &columns {
            let cid: i64 = column.get("cid");
            let name: String = column.get("name");
            let column_type: String = column.get("type");
            let notnull: i64 = column.get("notnull");
            let default_value: Option<String> = column.get("dflt_value");
            let pk: i64 = column.get("pk");
            snapshot.push_str(&format!(
                "  column cid={cid} name={name} type={column_type} notnull={notnull} default={default_value:?} pk={pk}\n"
            ));
        }
    }
    Ok(snapshot)
}

fn fixture_applied_through(fixture: &str) -> i64 {
    fixture
        .lines()
        .find_map(|line| line.strip_prefix("applied_through="))
        .expect("fixture applied_through")
        .parse()
        .expect("fixture applied_through version")
}

/// `applied_through` 世代までの up migration だけを適用した DB ファイルを合成する
/// 共通ヘルパ(WP-S6 T2 の stepwise round-trip でも中間世代 DB の合成に使う)。
/// `applied_through = 0` は「migration 未適用(_sqlx_migrations のみ)」を意味する。
pub(crate) async fn materialize_sqlite_fixture(
    db_path: &std::path::Path,
    applied_through: i64,
) -> Result<()> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await?;

    for migration in STORE_MIGRATOR
        .iter()
        .filter(|migration| {
            migration.version <= applied_through && !migration.migration_type.is_down_migration()
        })
        .collect::<Vec<_>>()
    {
        sqlx::raw_sql(migration.sql.as_ref()).execute(&pool).await?;
        sqlx::query(
            r#"
            INSERT INTO _sqlx_migrations (
                version,
                description,
                success,
                checksum,
                execution_time
            )
            VALUES (?1, ?2, true, ?3, 0)
            "#,
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&pool)
        .await?;
    }

    pool.close().await;
    Ok(())
}

async fn sqlite_column_exists(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    table_name: &str,
    column_name: &str,
) -> Result<bool> {
    let rows = sqlx::query("SELECT name FROM pragma_table_info(?1)")
        .bind(table_name)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column_name))
}
