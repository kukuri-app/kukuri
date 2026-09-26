use super::*;
use base64::Engine;
use sqlx::{
    Connection, Row,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AccountDisplay {
    pub id: String,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub picture: Option<String>,
    pub unavailable: bool,
}

/// 表示名と avatar の hash。avatar が保護所有先(#1221 R5-G)にあれば、その bytes も返す。
pub(crate) async fn read_profile(
    dir: &Path,
    account: &AccountRecord,
) -> Result<(AccountDisplay, Option<String>, Option<Vec<u8>>)> {
    let mut display = AccountDisplay {
        id: account.id.clone(),
        name: None,
        display_name: None,
        picture: None,
        unavailable: false,
    };
    let db = account_db_path(dir, &account.id);
    if !db.exists() {
        return Ok((display, None, None));
    }
    let options = SqliteConnectOptions::new()
        .filename(&db)
        .read_only(true)
        .create_if_missing(false);
    let mut conn = SqliteConnection::connect_with(&options).await?;
    let row = sqlx::query("SELECT name, display_name, picture_blob_hash, picture_mime, picture_bytes FROM profiles WHERE pubkey = ?")
        .bind(&account.pubkey).fetch_optional(&mut conn).await?;
    let mut hash = None;
    if let Some(row) = row {
        display.name = row.try_get("name")?;
        display.display_name = row.try_get("display_name")?;
        let bytes: Option<i64> = row.try_get("picture_bytes")?;
        let mime: Option<String> = row.try_get("picture_mime")?;
        if bytes.is_some_and(|bytes| (1..=2_000_000).contains(&bytes))
            && mime.as_deref().is_some_and(|mime| {
                matches!(
                    mime,
                    "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                )
            })
        {
            hash = row.try_get("picture_blob_hash")?;
            display.picture = mime;
        }
    }
    let mut protected = None;
    if let Some(hash) = &hash {
        let row = sqlx::query(
            "SELECT payload, file_name FROM remote_content_cache \
             WHERE kind = 'blob' AND cache_key = ? AND is_protected = 1",
        )
        .bind(hash)
        .fetch_optional(&mut conn)
        .await
        // 移行前の版で閉じた account の DB には表が無い。そのときは旧領域から読む。
        .unwrap_or_default();
        if let Some(row) = row {
            protected = match row.try_get::<Option<String>, _>("file_name")? {
                Some(name) => tokio::fs::read(db.with_extension("remote-blobs").join(name))
                    .await
                    .ok(),
                None => row.try_get("payload")?,
            };
        }
    }
    conn.close().await?;
    Ok((display, hash, protected))
}

pub(crate) fn set_picture(display: &mut AccountDisplay, bytes: Option<Vec<u8>>) {
    display.picture = display.picture.take().and_then(|mime| {
        bytes.filter(|b| b.len() <= 2_000_000).map(|b| {
            format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(b)
            )
        })
    });
}
