use super::*;

pub(crate) static STORE_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// 起動時(SQLite 接続 / migration 適用)の失敗を型で区別するエラー。
///
/// desktop の起動 Failed 画面は、この variant を downcast して DatabaseOpen /
/// DatabaseMigration に分類する(WP-Q2、従来のエラー文字列 contains 判定を置換)。
/// `Display`(および anyhow の `{:#}`)は従来と同じ message + source を出すため、
/// 表示・ログ・文字列アサーションは不変。
#[derive(Debug, thiserror::Error)]
pub enum StoreStartupError {
    /// SQLite データベースへの接続 / オープン失敗。
    #[error("failed to connect sqlite database: {path}")]
    Open {
        path: String,
        #[source]
        source: sqlx::Error,
    },
    /// embedded migration の適用失敗(checksum 不一致・未知世代など)。
    #[error("failed to run sqlite migrations")]
    Migration(#[source] sqlx::migrate::MigrateError),
}

impl SqliteStore {
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = sqlite_pool_options(
            if database_url.contains(":memory:") {
                1
            } else {
                4
            },
            !database_url.contains(":memory:"),
        )
        .connect(database_url)
        .await
        .map_err(|source| StoreStartupError::Open {
            path: database_url.to_string(),
            source,
        })?;

        run_store_migrations(&pool).await?;

        Ok(Self::from_pool(pool, None))
    }

    pub async fn connect_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true);
        let pool = sqlite_pool_options(4, true)
            .connect_with(options)
            .await
            .map_err(|source| StoreStartupError::Open {
                path: path.display().to_string(),
                source,
            })?;

        run_store_migrations(&pool).await?;

        let remote_cache_files = path.with_extension("remote-blobs");
        tokio::fs::create_dir_all(&remote_cache_files).await?;
        Ok(Self::from_pool(pool, Some(remote_cache_files)))
    }

    pub async fn connect_memory() -> Result<Self> {
        Self::connect("sqlite::memory:").await
    }

    /// 計測用(#1239 AC-7): SQLite の仮想機械が実行した命令の数を `counter` に足していく in-memory の store。
    ///
    /// 命令の数は、読み書きした行の数とともに増え、B-tree の深さ(表の大きさ)には依存しない。表の件数を変えても
    /// 操作の命令の数が変わらないことで、件数に比例する読み書き(全件の走査・書き直し)が無いことを確かめる。
    #[cfg(any(test, feature = "test-support"))]
    pub async fn connect_memory_counting_vm_steps(
        counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) -> Result<Self> {
        let pool = sqlite_pool_options(1, false)
            .after_connect(move |connection, _meta| {
                let counter = counter.clone();
                Box::pin(async move {
                    configure_connection(connection, false).await?;
                    connection
                        .lock_handle()
                        .await?
                        .set_progress_handler(1, move || {
                            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            true
                        });
                    Ok(())
                })
            })
            .connect("sqlite::memory:")
            .await
            .map_err(|source| StoreStartupError::Open {
                path: "sqlite::memory:".to_string(),
                source,
            })?;
        run_store_migrations(&pool).await?;
        Ok(Self::from_pool(pool, None))
    }

    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    fn from_pool(pool: Pool<Sqlite>, remote_cache_files: Option<std::path::PathBuf>) -> Self {
        let (adult_label_evictions, _) = tokio::sync::broadcast::channel(64);
        Self {
            pool,
            remote_cache_files,
            remote_cache_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            remote_cache_reserved: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            adult_label_evictions,
        }
    }

    pub async fn close(&self) {
        // sqlx 0.8 の Pool::close は、最後の permit を待つ間に return_to_pool が idle へ戻した
        // 接続を閉じずに返る。その接続は後で別 thread が閉じ、WAL の checkpoint と -wal/-shm の
        // 削除が呼出元の file 操作と競合する。1 回目が返った時点でその接続は idle にあるため、
        // 2 回目で閉じる。
        self.pool.close().await;
        self.pool.close().await;
    }
}

fn sqlite_pool_options(max_connections: u32, enable_wal: bool) -> SqlitePoolOptions {
    SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(max_connections)
        .after_connect(move |connection, _meta| {
            Box::pin(async move { configure_connection(connection, enable_wal).await })
        })
}

async fn configure_connection(
    connection: &mut sqlx::SqliteConnection,
    enable_wal: bool,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&mut *connection)
        .await?;
    sqlx::query("PRAGMA synchronous = NORMAL")
        .execute(&mut *connection)
        .await?;
    if enable_wal {
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

// checksum 不一致(CRLF checkout 由来を含む)はここで失敗し、起動 Failed 画面で
// DatabaseMigration として通知される。かつての接続毎 CRLF 自己修復(#211)は
// WP-C6 で撤去済み — 根本原因は .gitattributes (*.sql text eol=lf、#211 同梱)で解消済み。
async fn run_store_migrations(pool: &Pool<Sqlite>) -> Result<()> {
    STORE_MIGRATOR
        .run(pool)
        .await
        .map_err(StoreStartupError::Migration)?;
    Ok(())
}
