//! community node ingestion（Model C）の scope 管理 state（#413 / ADR 0025 §2.2 / §6）。
//!
//! indexing = Model C（docs replica sync participant）は、operator が明示的に引き受けた
//! supported topic / 許可 channel の共有 replica のみを ingest する。本モジュールはその node-local な
//! 運用 state（supported set / user request / private channel capability）を Postgres に保持する。
//!
//! ここは canonical source ではない。canonical は sync 元の topic / channel docs replica であり、
//! これらの state は再構築可能な node-local projection の制御情報である（ADR 0025 §2.1）。
//!
//! private channel の capability（namespace secret）は「indexing リクエスト＝secret 送信」
//! （ADR 0025 §6.3）で受け取り、at-rest 暗号化（XChaCha20Poly1305）して保存する。平文は列に残さない。
//! 復号鍵は runtime（Secret Manager / env 注入）が供給し、DB には置かない。有効な secret を提示できる
//! こと自体を channel 権限の証明とみなし、CN は新しい権限体系を作らない。

use anyhow::{Context, Result, anyhow, bail};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use chrono::{DateTime, Utc};
use secp256k1::rand::{RngCore, rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgRow};
use uuid::Uuid;

pub use kukuri_cn_protocol::{IndexScopeKind, IndexingRequestStatus};

/// private channel capability 暗号化の AEAD associated data のドメイン分離 prefix。
/// 実際の AAD は `channel_id` を連結して channel 同一性に束縛する（`channel_secret_aad`）。
const CHANNEL_SECRET_AAD_PREFIX: &[u8] = b"kukuri-cn-index:channel-secret:v1:";

/// channel_id に束縛した AEAD associated data を作る。
///
/// AAD に channel_id を含めることで、ある channel の (nonce, ciphertext) を別 channel_id の行へ
/// 差し替えても復号（認証）に失敗する。DB 行の取り違え / コピーで別 channel の capability に化けるのを防ぐ。
fn channel_secret_aad(channel_id: &str) -> Vec<u8> {
    let mut aad = CHANNEL_SECRET_AAD_PREFIX.to_vec();
    aad.extend_from_slice(channel_id.as_bytes());
    aad
}

/// operator が index を引き受けた scope エントリ。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportedTopic {
    pub kind: IndexScopeKind,
    pub id: String,
    pub created_at: DateTime<Utc>,
}

/// user からの indexing request。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingRequest {
    pub id: String,
    pub requester_pubkey: String,
    pub kind: IndexScopeKind,
    pub target_id: String,
    pub status: IndexingRequestStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
}

/// 登録済み channel capability（平文の namespace secret hex を復号済みで保持する）。
///
/// この型は復号後の値を持つため、ログや外部へ露出させないこと。
#[derive(Clone, PartialEq, Eq)]
pub struct ChannelSecret {
    pub channel_id: String,
    pub epoch_id: Option<String>,
    pub rotated: bool,
    pub namespace_secret_hex: String,
}

impl std::fmt::Debug for ChannelSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // secret 値をログへ漏らさない。
        f.debug_struct("ChannelSecret")
            .field("channel_id", &self.channel_id)
            .finish_non_exhaustive()
    }
}

/// channel secret の at-rest 暗号化に使う鍵。runtime が供給する（DB には置かない）。
///
/// 32 byte を直接受け取る形にせず、任意長の material から HKDF ではなく単純な domain-separated
/// SHA-256 で 32 byte 鍵を導出する。material は Secret Manager / env 由来を想定する。
#[derive(Clone)]
pub struct ChannelSecretCipher {
    key: [u8; 32],
}

impl std::fmt::Debug for ChannelSecretCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelSecretCipher")
            .finish_non_exhaustive()
    }
}

impl ChannelSecretCipher {
    /// runtime 供給の鍵 material から cipher を作る。material が短すぎる / placeholder の場合は拒否する。
    pub fn from_key_material(material: &str) -> Result<Self> {
        let trimmed = material.trim();
        if trimmed.len() < 32 {
            bail!("channel secret encryption key must be at least 32 bytes");
        }
        if kukuri_core::is_placeholder_secret(trimmed) {
            bail!("channel secret encryption key still contains a placeholder value");
        }
        let mut hasher = Sha256::new();
        hasher.update(b"kukuri-cn-index:channel-secret-key:v1");
        hasher.update(trimmed.as_bytes());
        Ok(Self {
            key: hasher.finalize().into(),
        })
    }

    fn cipher(&self) -> Result<XChaCha20Poly1305> {
        XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|_| anyhow!("invalid channel secret encryption key length"))
    }

    /// 平文の namespace secret hex を (nonce, ciphertext) に暗号化する。
    ///
    /// AAD に `channel_id` を束縛するため、暗号文は特定 channel にのみ有効になる。
    fn encrypt(&self, channel_id: &str, namespace_secret_hex: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut nonce = [0u8; 24];
        rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher()?
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: namespace_secret_hex.as_bytes(),
                    aad: channel_secret_aad(channel_id).as_slice(),
                },
            )
            .map_err(|_| anyhow!("failed to encrypt channel secret"))?;
        Ok((nonce.to_vec(), ciphertext))
    }

    /// (nonce, ciphertext) から平文の namespace secret hex を復号する。
    ///
    /// `channel_id` を AAD として要求するため、別 channel の行へ差し替えた暗号文は復号に失敗する。
    fn decrypt(&self, channel_id: &str, nonce: &[u8], ciphertext: &[u8]) -> Result<String> {
        if nonce.len() != 24 {
            bail!("channel secret nonce must be 24 bytes");
        }
        let plaintext = self
            .cipher()?
            .decrypt(
                <&XNonce>::try_from(nonce).expect("nonce length checked"),
                Payload {
                    msg: ciphertext,
                    aad: channel_secret_aad(channel_id).as_slice(),
                },
            )
            .map_err(|_| anyhow!("failed to decrypt channel secret"))?;
        String::from_utf8(plaintext).context("decrypted channel secret is not valid utf8")
    }
}

/// supported set へ scope を追加する（冪等）。
pub async fn add_supported_topic(
    pool: &PgPool,
    kind: IndexScopeKind,
    id: &str,
) -> Result<SupportedTopic> {
    let id = id.trim();
    if id.is_empty() {
        bail!("supported topic id must not be empty");
    }
    let row = sqlx::query(
        "INSERT INTO cn_index.supported_topics (id, kind)
         VALUES ($1, $2)
         ON CONFLICT (kind, id) DO UPDATE SET id = EXCLUDED.id
         RETURNING id, kind, created_at",
    )
    .bind(id)
    .bind(kind.as_str())
    .fetch_one(pool)
    .await?;
    supported_topic_from_row(&row)
}

/// supported set から scope を除去する。除去できたら true。
///
/// 除去後の replica sync 停止 / de-index は呼び出し側（cn-indexer）が担う。
pub async fn remove_supported_topic(pool: &PgPool, kind: IndexScopeKind, id: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM cn_index.supported_topics WHERE kind = $1 AND id = $2")
        .bind(kind.as_str())
        .bind(id.trim())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// supported set を新着順で列挙する。
pub async fn list_supported_topics(pool: &PgPool) -> Result<Vec<SupportedTopic>> {
    let rows = sqlx::query(
        "SELECT id, kind, created_at
         FROM cn_index.supported_topics
         ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    rows.iter().map(supported_topic_from_row).collect()
}

/// Authorized scoped search/discovery marks demand at most once a minute.
pub async fn mark_index_demand(pool: &PgPool, kind: IndexScopeKind, scope_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE cn_index.supported_topics SET last_index_demand_at = NOW()
         WHERE kind = $1 AND id = $2
           AND (last_index_demand_at IS NULL OR last_index_demand_at < NOW() - INTERVAL '1 minute')",
    )
    .bind(kind.as_str())
    .bind(scope_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// scope が supported set に含まれるか（scope ゲートの単一判定点）。
pub async fn is_topic_supported(pool: &PgPool, kind: IndexScopeKind, id: &str) -> Result<bool> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM cn_index.supported_topics WHERE kind = $1 AND id = $2
        )",
    )
    .bind(kind.as_str())
    .bind(id.trim())
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

/// user からの indexing request を保存する（同一 requester の同一対象は冪等更新）。
///
/// request は index を保証しない。status は `pending` で入り、operator の承認を待つ。
/// 既に承認 / 却下済みの request がある場合は上書きせず既存を返す。
pub async fn insert_indexing_request(
    pool: &PgPool,
    requester_pubkey: &str,
    kind: IndexScopeKind,
    target_id: &str,
) -> Result<IndexingRequest> {
    let requester_pubkey = requester_pubkey.trim();
    let target_id = target_id.trim();
    if requester_pubkey.is_empty() {
        bail!("indexing request requester_pubkey must not be empty");
    }
    if target_id.is_empty() {
        bail!("indexing request target_id must not be empty");
    }
    let id = Uuid::new_v4().to_string();
    let row = sqlx::query(
        "INSERT INTO cn_index.indexing_requests
            (id, requester_pubkey, kind, target_id, status)
         VALUES ($1, $2, $3, $4, 'pending')
         ON CONFLICT (kind, target_id, requester_pubkey) DO UPDATE
            SET requester_pubkey = EXCLUDED.requester_pubkey
         RETURNING id, requester_pubkey, kind, target_id, status, created_at, decided_at",
    )
    .bind(&id)
    .bind(requester_pubkey)
    .bind(kind.as_str())
    .bind(target_id)
    .fetch_one(pool)
    .await?;
    indexing_request_from_row(&row)
}

/// indexing request を status 絞り込み（任意）で新着順に列挙する。
pub async fn list_indexing_requests(
    pool: &PgPool,
    status: Option<IndexingRequestStatus>,
    limit: i64,
    offset: i64,
) -> Result<Vec<IndexingRequest>> {
    let rows = match status {
        Some(status) => {
            sqlx::query(
                "SELECT id, requester_pubkey, kind, target_id, status, created_at, decided_at
                 FROM cn_index.indexing_requests
                 WHERE status = $1
                 ORDER BY created_at DESC
                 LIMIT $2 OFFSET $3",
            )
            .bind(status.as_str())
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query(
                "SELECT id, requester_pubkey, kind, target_id, status, created_at, decided_at
                 FROM cn_index.indexing_requests
                 ORDER BY created_at DESC
                 LIMIT $1 OFFSET $2",
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };
    rows.iter().map(indexing_request_from_row).collect()
}

/// 呼出し主自身の indexing request を新着順に列挙する(#975 の read 面)。
///
/// `requester_pubkey` で絞るため他利用者の申請は含まない。却下済みの行も残っているので
/// `rejected` を含めて返す(申請者が自分の却下を確認できる)。
pub async fn list_indexing_requests_for_requester(
    pool: &PgPool,
    requester_pubkey: &str,
) -> Result<Vec<IndexingRequest>> {
    let requester_pubkey = requester_pubkey.trim();
    if requester_pubkey.is_empty() {
        bail!("indexing request requester_pubkey must not be empty");
    }
    let rows = sqlx::query(
        "SELECT id, requester_pubkey, kind, target_id, status, created_at, decided_at
         FROM cn_index.indexing_requests
         WHERE requester_pubkey = $1
         ORDER BY created_at DESC",
    )
    .bind(requester_pubkey)
    .fetch_all(pool)
    .await?;
    rows.iter().map(indexing_request_from_row).collect()
}

/// indexing request を承認する。承認と同時に対象 scope を supported set に入れる
/// （`request → operator 承認（supported 化）` の接続。ADR 0025 §2.2）。
///
/// request が存在しなければ None を返す。既に承認済みでも冪等に supported set を保証する。
pub async fn approve_indexing_request(pool: &PgPool, id: &str) -> Result<Option<IndexingRequest>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "UPDATE cn_index.indexing_requests
         SET status = 'approved', decided_at = NOW()
         WHERE id = $1
         RETURNING id, requester_pubkey, kind, target_id, status, created_at, decided_at",
    )
    .bind(id.trim())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let request = indexing_request_from_row(&row)?;
    sqlx::query(
        "INSERT INTO cn_index.supported_topics (id, kind)
         VALUES ($1, $2)
         ON CONFLICT (kind, id) DO NOTHING",
    )
    .bind(&request.target_id)
    .bind(request.kind.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(request))
}

/// indexing request を却下する。却下は supported set を変更しない。
pub async fn reject_indexing_request(pool: &PgPool, id: &str) -> Result<Option<IndexingRequest>> {
    let row = sqlx::query(
        "UPDATE cn_index.indexing_requests
         SET status = 'rejected', decided_at = NOW()
         WHERE id = $1
         RETURNING id, requester_pubkey, kind, target_id, status, created_at, decided_at",
    )
    .bind(id.trim())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(indexing_request_from_row).transpose()
}

/// channel capability の登録エラー。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelSecretConflict {
    /// 既に別の capability が登録済み（別 requester が違う secret を提示した）。
    AlreadyRegistered,
}

impl std::fmt::Display for ChannelSecretConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelSecretConflict::AlreadyRegistered => {
                write!(
                    f,
                    "a different channel capability is already registered for this channel"
                )
            }
        }
    }
}

impl std::error::Error for ChannelSecretConflict {}

/// channel capability（namespace secret）を暗号化して登録する（channel_id で冪等更新）。
///
/// 運営者経路（CLI 等）向けの無条件 upsert。user 経路（indexing request）は権限の乗っ取りを防ぐため
/// [`register_channel_secret`]（first-writer-wins）を使うこと。
pub async fn upsert_channel_secret(
    pool: &PgPool,
    cipher: &ChannelSecretCipher,
    channel_id: &str,
    namespace_secret_hex: &str,
) -> Result<()> {
    let channel_id = channel_id.trim();
    let namespace_secret_hex = namespace_secret_hex.trim();
    if channel_id.is_empty() {
        bail!("channel secret channel_id must not be empty");
    }
    validate_namespace_secret_hex(namespace_secret_hex)?;
    let (nonce, ciphertext) = cipher.encrypt(channel_id, namespace_secret_hex)?;
    sqlx::query(
        "INSERT INTO cn_index.channel_secrets (channel_id, nonce, ciphertext)
         VALUES ($1, $2, $3)
         ON CONFLICT (channel_id) DO UPDATE
            SET nonce = EXCLUDED.nonce,
                ciphertext = EXCLUDED.ciphertext,
                epoch_id = NULL,
                rotated = FALSE,
                request_managed = FALSE,
                updated_at = NOW()",
    )
    .bind(channel_id)
    .bind(nonce)
    .bind(ciphertext)
    .execute(pool)
    .await?;
    Ok(())
}

/// user の indexing request 経路で channel capability を登録する（first-writer-wins）。
///
/// - 未登録なら登録する。
/// - 同一 secret の再提示は冪等（no-op）。
/// - 既に **別の** secret が登録されている場合は [`ChannelSecretConflict::AlreadyRegistered`] を返し、
///   後から来た requester が既存 capability を上書き（乗っ取り / DoS）できないようにする。
///
/// これにより「secret を提示できること自体が権限の証明」（ADR 0025 §6.3）を維持しつつ、既存 capability の
/// 破壊を防ぐ。operator は必要なら [`remove_channel_secret`] + CLI で明示的に差し替えられる。
pub async fn register_channel_secret(
    pool: &PgPool,
    cipher: &ChannelSecretCipher,
    channel_id: &str,
    namespace_secret_hex: &str,
) -> Result<()> {
    let channel_id = channel_id.trim();
    let namespace_secret_hex = namespace_secret_hex.trim();
    if channel_id.is_empty() {
        bail!("channel secret channel_id must not be empty");
    }
    validate_namespace_secret_hex(namespace_secret_hex)?;

    if let Some(existing) = get_channel_secret(pool, cipher, channel_id).await? {
        // 同一 secret の再提示は冪等。別 secret なら乗っ取りを拒否する。
        if existing.namespace_secret_hex == namespace_secret_hex {
            return Ok(());
        }
        return Err(ChannelSecretConflict::AlreadyRegistered.into());
    }

    let (nonce, ciphertext) = cipher.encrypt(channel_id, namespace_secret_hex)?;
    // INSERT ... ON CONFLICT DO NOTHING で TOCTOU（並行登録）でも既存を保護する。
    let result = sqlx::query(
        "INSERT INTO cn_index.channel_secrets (channel_id, nonce, ciphertext, request_managed)
         VALUES ($1, $2, $3, TRUE)
         ON CONFLICT (channel_id) DO NOTHING",
    )
    .bind(channel_id)
    .bind(nonce)
    .bind(ciphertext)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        // 競合で別 writer が先に登録した。同一 secret なら許容、別 secret なら拒否。
        if let Some(existing) = get_channel_secret(pool, cipher, channel_id).await?
            && existing.namespace_secret_hex == namespace_secret_hex
        {
            return Ok(());
        }
        return Err(ChannelSecretConflict::AlreadyRegistered.into());
    }
    Ok(())
}

/// Attach one explicitly disclosed epoch to the existing first-writer-wins capability.
/// A different secret or epoch cannot replace another requester's channel registration.
pub async fn register_channel_secret_with_epoch(
    pool: &PgPool,
    cipher: &ChannelSecretCipher,
    channel_id: &str,
    epoch_id: &str,
    namespace_secret_hex: &str,
) -> Result<()> {
    let epoch_id = epoch_id.trim();
    if epoch_id.is_empty() || epoch_id.len() > 1024 {
        bail!("channel epoch id must contain 1..=1024 bytes");
    }
    register_channel_secret(pool, cipher, channel_id, namespace_secret_hex).await?;
    let updated = sqlx::query(
        "UPDATE cn_index.channel_secrets SET epoch_id = $2
         WHERE channel_id = $1 AND (epoch_id IS NULL OR epoch_id = $2)",
    )
    .bind(channel_id.trim())
    .bind(epoch_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ChannelSecretConflict::AlreadyRegistered.into());
    }
    Ok(())
}

/// An existing requester can follow a channel into a new epoch by proving the
/// capability currently held by this node. The row lock serializes rotations;
/// an operator removal or replacement cannot be undone by a stale retry.
pub async fn rotate_channel_secret_epoch(
    pool: &PgPool,
    cipher: &ChannelSecretCipher,
    requester_pubkey: &str,
    channel_id: &str,
    previous: (&str, &str),
    next: (&str, &str),
) -> Result<()> {
    let (previous_epoch_id, previous_secret_hex) = previous;
    let (epoch_id, secret_hex) = next;
    let channel_id = channel_id.trim();
    let previous_epoch_id = previous_epoch_id.trim();
    let epoch_id = epoch_id.trim();
    if channel_id.is_empty()
        || previous_epoch_id.is_empty()
        || epoch_id.is_empty()
        || epoch_id.len() > 1024
        || previous_epoch_id.len() > 1024
        || previous_epoch_id == epoch_id
    {
        bail!("invalid channel epoch transition");
    }
    validate_namespace_secret_hex(previous_secret_hex)?;
    validate_namespace_secret_hex(secret_hex)?;
    let mut tx = pool.begin().await?;
    let eligible: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM cn_index.indexing_requests
         WHERE kind='private_channel' AND target_id=$1 AND requester_pubkey=$2
           AND status IN ('pending','approved'))",
    )
    .bind(channel_id)
    .bind(requester_pubkey)
    .fetch_one(&mut *tx)
    .await?;
    if !eligible {
        return Err(ChannelSecretConflict::AlreadyRegistered.into());
    }
    let row = sqlx::query(
        "SELECT epoch_id, nonce, ciphertext FROM cn_index.channel_secrets
         WHERE channel_id=$1 FOR UPDATE",
    )
    .bind(channel_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(ChannelSecretConflict::AlreadyRegistered.into());
    };
    let stored_epoch: Option<String> = row.try_get("epoch_id")?;
    let old_secret = cipher.decrypt(
        channel_id,
        &row.try_get::<Vec<u8>, _>("nonce")?,
        &row.try_get::<Vec<u8>, _>("ciphertext")?,
    )?;
    if stored_epoch.as_deref() == Some(epoch_id) && old_secret == secret_hex {
        tx.commit().await?;
        return Ok(());
    }
    if stored_epoch.as_deref() != Some(previous_epoch_id)
        || old_secret.len() != previous_secret_hex.len()
        || old_secret
            .bytes()
            .zip(previous_secret_hex.bytes())
            .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
            != 0
    {
        return Err(ChannelSecretConflict::AlreadyRegistered.into());
    }
    let (nonce, ciphertext) = cipher.encrypt(channel_id, secret_hex)?;
    sqlx::query(
        "UPDATE cn_index.channel_secrets
         SET epoch_id=$2, nonce=$3, ciphertext=$4, rotated=TRUE, updated_at=NOW()
         WHERE channel_id=$1",
    )
    .bind(channel_id)
    .bind(epoch_id)
    .bind(nonce)
    .bind(ciphertext)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Remove only this account's grant. An operator-owned capability or another
/// requester's grant keeps the channel available. Derived rows are gated by
/// the missing capability immediately and reclaimed by the index cache owner.
pub async fn revoke_indexing_request(
    pool: &PgPool,
    requester_pubkey: &str,
    kind: IndexScopeKind,
    target_id: &str,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let removed = sqlx::query(
        "DELETE FROM cn_index.indexing_requests
         WHERE requester_pubkey=$1 AND kind=$2 AND target_id=$3",
    )
    .bind(requester_pubkey)
    .bind(kind.as_str())
    .bind(target_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if removed && kind == IndexScopeKind::PrivateChannel {
        sqlx::query(
            "DELETE FROM cn_index.channel_secrets c
             WHERE c.channel_id=$1 AND c.request_managed=TRUE
               AND NOT EXISTS (
                 SELECT 1 FROM cn_index.indexing_requests r
                 WHERE r.kind='private_channel' AND r.target_id=c.channel_id)",
        )
        .bind(target_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(removed)
}

/// 登録済み channel capability を復号して取得する。未登録なら None。
pub async fn get_channel_secret(
    pool: &PgPool,
    cipher: &ChannelSecretCipher,
    channel_id: &str,
) -> Result<Option<ChannelSecret>> {
    let row = sqlx::query(
        "SELECT channel_id, epoch_id, rotated, nonce, ciphertext
         FROM cn_index.channel_secrets
         WHERE channel_id = $1",
    )
    .bind(channel_id.trim())
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let channel_id: String = row.try_get("channel_id")?;
    let epoch_id: Option<String> = row.try_get("epoch_id")?;
    let rotated: bool = row.try_get("rotated")?;
    let nonce: Vec<u8> = row.try_get("nonce")?;
    let ciphertext: Vec<u8> = row.try_get("ciphertext")?;
    let namespace_secret_hex = cipher.decrypt(channel_id.as_str(), &nonce, &ciphertext)?;
    Ok(Some(ChannelSecret {
        channel_id,
        epoch_id,
        rotated,
        namespace_secret_hex,
    }))
}

/// 登録済み channel capability をすべて復号して列挙する（起動時の replica open 用）。
pub async fn list_channel_secrets(
    pool: &PgPool,
    cipher: &ChannelSecretCipher,
) -> Result<Vec<ChannelSecret>> {
    let rows = sqlx::query(
        "SELECT channel_id, epoch_id, rotated, nonce, ciphertext
         FROM cn_index.channel_secrets
         ORDER BY channel_id",
    )
    .fetch_all(pool)
    .await?;
    let mut secrets = Vec::with_capacity(rows.len());
    for row in &rows {
        let channel_id: String = row.try_get("channel_id")?;
        let epoch_id: Option<String> = row.try_get("epoch_id")?;
        let rotated: bool = row.try_get("rotated")?;
        let nonce: Vec<u8> = row.try_get("nonce")?;
        let ciphertext: Vec<u8> = row.try_get("ciphertext")?;
        let namespace_secret_hex = cipher.decrypt(channel_id.as_str(), &nonce, &ciphertext)?;
        secrets.push(ChannelSecret {
            channel_id,
            epoch_id,
            rotated,
            namespace_secret_hex,
        });
    }
    Ok(secrets)
}

/// channel capability を失効させる。除去できたら true。
///
/// 失効後の replica sync 停止 / de-index は呼び出し側（cn-indexer）が担う。
pub async fn remove_channel_secret(pool: &PgPool, channel_id: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM cn_index.channel_secrets WHERE channel_id = $1")
        .bind(channel_id.trim())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// namespace secret hex が 32 byte hex であることを検証する（docs-sync の parse と同じ前提）。
fn validate_namespace_secret_hex(value: &str) -> Result<()> {
    let decoded = hex::decode(value).context("channel secret must be valid hex")?;
    if decoded.len() != 32 {
        bail!("channel secret must decode to 32 bytes");
    }
    Ok(())
}

fn supported_topic_from_row(row: &PgRow) -> Result<SupportedTopic> {
    Ok(SupportedTopic {
        kind: IndexScopeKind::parse(&row.try_get::<String, _>("kind")?)?,
        id: row.try_get("id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn indexing_request_from_row(row: &PgRow) -> Result<IndexingRequest> {
    Ok(IndexingRequest {
        id: row.try_get("id")?,
        requester_pubkey: row.try_get("requester_pubkey")?,
        kind: IndexScopeKind::parse(&row.try_get::<String, _>("kind")?)?,
        target_id: row.try_get("target_id")?,
        status: IndexingRequestStatus::parse(&row.try_get::<String, _>("status")?)?,
        created_at: row.try_get("created_at")?,
        decided_at: row.try_get("decided_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cipher() -> ChannelSecretCipher {
        ChannelSecretCipher::from_key_material("unit-test-channel-secret-encryption-key-0123456789")
            .expect("cipher")
    }

    #[test]
    fn channel_secret_roundtrips_through_encrypt_decrypt() {
        let cipher = test_cipher();
        let secret_hex = hex::encode([7u8; 32]);
        let (nonce, ciphertext) = cipher.encrypt("secret-room", &secret_hex).expect("encrypt");
        assert_ne!(ciphertext, secret_hex.as_bytes());
        let decrypted = cipher
            .decrypt("secret-room", &nonce, &ciphertext)
            .expect("decrypt");
        assert_eq!(decrypted, secret_hex);
    }

    #[test]
    fn channel_secret_decrypt_rejects_tampered_ciphertext() {
        let cipher = test_cipher();
        let secret_hex = hex::encode([9u8; 32]);
        let (nonce, mut ciphertext) = cipher.encrypt("secret-room", &secret_hex).expect("encrypt");
        ciphertext[0] ^= 0xff;
        assert!(cipher.decrypt("secret-room", &nonce, &ciphertext).is_err());
    }

    #[test]
    fn channel_secret_decrypt_rejects_wrong_key() {
        let cipher = test_cipher();
        let secret_hex = hex::encode([1u8; 32]);
        let (nonce, ciphertext) = cipher.encrypt("secret-room", &secret_hex).expect("encrypt");
        let other = ChannelSecretCipher::from_key_material(
            "a-different-channel-secret-encryption-key-abcdef",
        )
        .expect("cipher");
        assert!(other.decrypt("secret-room", &nonce, &ciphertext).is_err());
    }

    #[test]
    fn channel_secret_decrypt_rejects_wrong_channel_id() {
        // AAD に channel_id を束縛するため、別 channel_id で復号すると失敗する（行差し替え防止）。
        let cipher = test_cipher();
        let secret_hex = hex::encode([2u8; 32]);
        let (nonce, ciphertext) = cipher.encrypt("secret-room", &secret_hex).expect("encrypt");
        assert!(cipher.decrypt("other-room", &nonce, &ciphertext).is_err());
    }

    #[test]
    fn channel_secret_cipher_rejects_weak_key_material() {
        assert!(ChannelSecretCipher::from_key_material("short").is_err());
        assert!(
            ChannelSecretCipher::from_key_material("this-is-long-enough-but-change-me-placeholder")
                .is_err()
        );
    }

    #[test]
    fn nonce_is_unique_per_encryption() {
        let cipher = test_cipher();
        let secret_hex = hex::encode([3u8; 32]);
        let (nonce_a, _) = cipher.encrypt("secret-room", &secret_hex).expect("encrypt");
        let (nonce_b, _) = cipher.encrypt("secret-room", &secret_hex).expect("encrypt");
        assert_ne!(nonce_a, nonce_b);
    }

    #[test]
    fn index_scope_kind_roundtrips() {
        for kind in [IndexScopeKind::PublicTopic, IndexScopeKind::PrivateChannel] {
            assert_eq!(IndexScopeKind::parse(kind.as_str()).unwrap(), kind);
        }
        assert!(IndexScopeKind::parse("nope").is_err());
    }

    #[test]
    fn indexing_request_status_roundtrips() {
        for status in [
            IndexingRequestStatus::Pending,
            IndexingRequestStatus::Approved,
            IndexingRequestStatus::Rejected,
        ] {
            assert_eq!(
                IndexingRequestStatus::parse(status.as_str()).unwrap(),
                status
            );
        }
        assert!(IndexingRequestStatus::parse("nope").is_err());
    }

    #[test]
    fn validate_namespace_secret_hex_enforces_32_bytes() {
        assert!(validate_namespace_secret_hex(&hex::encode([0u8; 32])).is_ok());
        assert!(validate_namespace_secret_hex(&hex::encode([0u8; 16])).is_err());
        assert!(validate_namespace_secret_hex("not-hex").is_err());
    }
}
