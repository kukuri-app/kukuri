//! WP-S6 T2: migration の per-generation stepwise round-trip と全適用後スキーマ golden。
//!
//! 後続 WP-H1(ProjectionStore 分割)の安全網。永続スキーマ(凍結境界)への
//! 退行・意図しない変更を CI と PR diff の両方で可視化する。
//!
//! - stepwise round-trip: 各世代 k について
//!   「全適用 → undo(V[k-1]) → V[k-1] まで適用した別 DB とスキーマ一致
//!   → run() 再適用 → 全適用スキーマと一致」を全世代で固定する。
//! - schema golden: 全適用後スキーマの正規化 dump を
//!   `crates/store/fixtures/schema/store_schema_full.txt` と比較して固定する。
//!
//! golden の再生成手順(スキーマ変更が本当に意図的な場合のみ):
//!   `UPDATE_SCHEMA_GOLDEN=1 cargo nextest run -p kukuri-store fully_migrated_schema_matches_golden`
//! を実行し、golden の diff をレビューのうえコミットする
//! (app-api の views_wire_snapshot.rs の UPDATE_FIXTURES=1 と同じ流儀)。
//!
//! snapshot の正規化方針:
//! - 生 sqlite_master の CREATE TABLE テキスト全体比較はしない
//!   (ALTER TABLE ADD/DROP COLUMN の履歴でテキストが揺れるため)。
//!   テーブルは pragma_table_info(name/type/notnull/dflt_value/pk、cid 順)、
//!   index は pragma index_list / index_info の構造で比較する。
//! - 外部キーは pragma_foreign_key_list(id/seq/参照先テーブル/from/to/
//!   on_update/on_delete/match)を id・seq 順(決定的)で各テーブル直下に含める
//!   (現行スキーマの FK は 20260310 up.sql の topic_objects / object_threads →
//!   envelopes の 2 本のみ)。
//! - CHECK 制約・VIEW・TRIGGER は現行スキーマに 1 つも存在しないため snapshot の
//!   対象外(導入する場合はこの正規化への追加が必要)。
//! - partial index の WHERE 句(notifications に 3 本)は pragma で取れないため、
//!   index に限り sqlite_master.sql の空白正規化テキストを併用する。
//! - テーブル・index は名前順ソート。_sqlx_migrations はスキーマ比較から除外し
//!   (undo 由来 DB と materialize 由来 DB で installed_on が異なるため)、
//!   version 集合を別 assert で固定する。

use super::*;
use anyhow::Result;
use sqlx::Row;
use std::path::PathBuf;

use super::migrations::materialize_sqlite_fixture;

/// 全世代の up migration version(migrations/ ディレクトリのファイル名から
/// 観測した生リテラル、昇順)。世代の追加・削除はここと golden の両方に現れる。
const EXPECTED_VERSIONS: [i64; 29] = [
    20260310000000,
    20260312000000,
    20260315000000,
    20260319000000,
    20260319010000,
    20260328000000,
    20260328010000,
    20260329000000,
    20260330000000,
    20260330010000,
    20260330020000,
    20260331000000,
    20260405000000,
    20260411000000,
    20260413000000,
    20260527000000,
    20260814000000,
    20260825000000,
    20260827000000,
    20260828000000,
    20260829000000,
    20260901000000,
    20260903000000,
    20260920000000,
    20260921000000,
    20260921010000,
    20260921020000,
    20260921030000,
    20260922000000,
];

/// 各世代 k について「全適用 → undo(V[k-1]) → 中間世代スキーマと一致 →
/// run() 再適用 → 全適用スキーマと一致」を固定する(k=0 は undo(0) = 全 revert)。
/// 期待側の中間世代 DB は materialize_sqlite_fixture で up のみ順次適用して合成する。
#[tokio::test]
async fn per_generation_stepwise_round_trip() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("stepwise-round-trip.db");
    let store = SqliteStore::connect_file(&db_path)
        .await
        .expect("initialize sqlite store");

    let versions = migrator_up_versions();
    assert_eq!(
        versions, EXPECTED_VERSIONS,
        "embedded store migration generations drifted from the observed 29 versions"
    );

    let full_snapshot = schema_snapshot(store.pool())
        .await
        .expect("snapshot fully migrated schema");
    assert_eq!(
        applied_migration_versions(store.pool())
            .await
            .expect("list applied versions after connect_file"),
        EXPECTED_VERSIONS
    );

    for (k, version) in versions.iter().enumerate() {
        let undo_target = if k == 0 { 0 } else { versions[k - 1] };
        STORE_MIGRATOR
            .undo(store.pool(), undo_target)
            .await
            .unwrap_or_else(|error| {
                panic!("undo generation {version} down to {undo_target} (k={k}): {error}")
            });

        // 期待スキーマ: V[k-1] までの up だけを適用した別 DB(materialize で合成)。
        let expected_db_path = tempdir
            .path()
            .join(format!("expected-through-{undo_target}.db"));
        materialize_sqlite_fixture(&expected_db_path, undo_target)
            .await
            .expect("materialize expected intermediate db");
        let expected_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite://{}", expected_db_path.display()))
            .await
            .expect("open expected intermediate db");
        let expected_snapshot = schema_snapshot(&expected_pool)
            .await
            .expect("snapshot expected intermediate schema");
        expected_pool.close().await;

        let undone_snapshot = schema_snapshot(store.pool())
            .await
            .expect("snapshot schema after undo");
        assert_eq!(
            undone_snapshot, expected_snapshot,
            "schema after undo(generation {version}, k={k}) must match a db migrated \
             through {undo_target} only"
        );
        assert_eq!(
            applied_migration_versions(store.pool())
                .await
                .expect("list applied versions after undo"),
            &versions[..k],
            "undo(generation {version}, k={k}) must leave exactly the first {k} versions applied"
        );

        STORE_MIGRATOR
            .run(store.pool())
            .await
            .unwrap_or_else(|error| {
                panic!("re-run migrations after undoing generation {version} (k={k}): {error}")
            });

        let replayed_snapshot = schema_snapshot(store.pool())
            .await
            .expect("snapshot schema after re-run");
        assert_eq!(
            replayed_snapshot, full_snapshot,
            "schema after undo(generation {version}, k={k}) + run() must match the fully \
             migrated schema"
        );
        assert_eq!(
            applied_migration_versions(store.pool())
                .await
                .expect("list applied versions after re-run"),
            EXPECTED_VERSIONS
        );
    }

    store.close().await;
}

/// 全適用後スキーマの正規化 dump を golden ファイルと比較して固定する。
/// スキーマが変わる PR は必ず golden の diff として現れる(H1 の受入
/// 「storage schema 不変」の検出装置)。再生成手順はファイル冒頭の doc コメント参照。
#[tokio::test]
async fn fully_migrated_schema_matches_golden() {
    let tempdir = tempdir().expect("tempdir");
    let db_path = tempdir.path().join("schema-golden.db");
    let store = SqliteStore::connect_file(&db_path)
        .await
        .expect("initialize sqlite store");

    // _sqlx_migrations は snapshot から除外しているため、version 集合を別に固定する。
    assert_eq!(
        migrator_up_versions(),
        EXPECTED_VERSIONS,
        "embedded store migration generations drifted from the observed 29 versions"
    );
    assert_eq!(
        applied_migration_versions(store.pool())
            .await
            .expect("list applied versions"),
        EXPECTED_VERSIONS
    );

    let snapshot = schema_snapshot(store.pool())
        .await
        .expect("snapshot fully migrated schema");
    store.close().await;

    let golden_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/schema/store_schema_full.txt");
    if std::env::var_os("UPDATE_SCHEMA_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_path.parent().expect("golden dir"))
            .expect("create golden dir");
        std::fs::write(&golden_path, &snapshot).expect("write schema golden");
        return;
    }

    let committed = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read {} ({error}); run `UPDATE_SCHEMA_GOLDEN=1 cargo nextest run \
                 -p kukuri-store fully_migrated_schema_matches_golden` to (re)generate the golden",
                golden_path.display()
            )
        })
        .replace("\r\n", "\n");
    assert_eq!(
        snapshot, committed,
        "fully migrated store schema drifted from fixtures/schema/store_schema_full.txt; \
         the persistent schema is a frozen boundary — if the change is truly intentional, \
         regenerate the golden (see module doc) and review the diff"
    );
}

/// STORE_MIGRATOR の up migration version 一覧(昇順・重複除去)。
/// migration メタデータの列挙は契約そのものなので migrator から取得してよい。
fn migrator_up_versions() -> Vec<i64> {
    let mut versions: Vec<i64> = STORE_MIGRATOR
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .collect();
    versions.sort_unstable();
    versions.dedup();
    versions
}

/// _sqlx_migrations に記録された version 一覧(昇順)。
async fn applied_migration_versions(pool: &sqlx::Pool<sqlx::Sqlite>) -> Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await?,
    )
}

/// スキーマの正規化 snapshot(決定的・LF・末尾改行あり)。
/// - テーブル(名前順)ごとに pragma_table_info を cid 順で 1 行ずつ、
///   続けて pragma_foreign_key_list を id・seq 順で 1 行ずつ。
/// - 続けて全テーブルの index を index 名順に、pragma index_list のメタ
///   (unique/origin/partial)+ pragma index_info のキー列(seqno 順)+
///   sqlite_master.sql の空白正規化テキスト(自動 index は None)を 1 行ずつ。
/// - _sqlx_migrations と sqlite 内部オブジェクトは除外。
async fn schema_snapshot(pool: &sqlx::Pool<sqlx::Sqlite>) -> Result<String> {
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
        // 外部キー(id・seq 順で決定的)。参照先カラム省略形は to が NULL → None。
        let foreign_keys =
            sqlx::query("SELECT * FROM pragma_foreign_key_list(?1) ORDER BY id, seq")
                .bind(table_name)
                .fetch_all(pool)
                .await?;
        for foreign_key in &foreign_keys {
            let id: i64 = foreign_key.get("id");
            let seq: i64 = foreign_key.get("seq");
            let referenced_table: String = foreign_key.get("table");
            let from: String = foreign_key.get("from");
            let to: Option<String> = foreign_key.get("to");
            let on_update: String = foreign_key.get("on_update");
            let on_delete: String = foreign_key.get("on_delete");
            let fk_match: String = foreign_key.get("match");
            snapshot.push_str(&format!(
                "  fk id={id} seq={seq} table={referenced_table} from={from} to={to:?} on_update={on_update} on_delete={on_delete} match={fk_match}\n"
            ));
        }
    }

    // (index 名, テーブル名, unique, origin, partial) を集めて index 名順にソート。
    let mut indexes: Vec<(String, String, i64, String, i64)> = Vec::new();
    for table_name in &table_names {
        let index_rows = sqlx::query("SELECT * FROM pragma_index_list(?1)")
            .bind(table_name)
            .fetch_all(pool)
            .await?;
        for index_row in &index_rows {
            indexes.push((
                index_row.get("name"),
                table_name.clone(),
                index_row.get("unique"),
                index_row.get("origin"),
                index_row.get("partial"),
            ));
        }
    }
    indexes.sort();

    for (index_name, table_name, unique, origin, partial) in &indexes {
        snapshot.push_str(&format!(
            "index {index_name} table={table_name} unique={unique} origin={origin} partial={partial}\n"
        ));
        let key_columns = sqlx::query("SELECT * FROM pragma_index_info(?1) ORDER BY seqno")
            .bind(index_name)
            .fetch_all(pool)
            .await?;
        for key_column in &key_columns {
            let seqno: i64 = key_column.get("seqno");
            let cid: i64 = key_column.get("cid");
            let name: Option<String> = key_column.get("name");
            snapshot.push_str(&format!("  key seqno={seqno} cid={cid} name={name:?}\n"));
        }
        // partial index の WHERE 句は pragma で取れないため sqlite_master.sql を
        // 空白正規化して併用する(自動 index は sql が NULL → None)。
        let raw_sql = sqlx::query_scalar::<_, Option<String>>(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
        )
        .bind(index_name)
        .fetch_optional(pool)
        .await?
        .flatten();
        let normalized_sql =
            raw_sql.map(|sql| sql.split_whitespace().collect::<Vec<_>>().join(" "));
        snapshot.push_str(&format!("  sql={normalized_sql:?}\n"));
    }

    Ok(snapshot)
}
