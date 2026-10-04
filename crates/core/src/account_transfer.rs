//! QR・専用リンクによる端末間の移行の招待（#1211 W7）と、確認済みの接続で送る必須 bundle の frame（AC-2）・
//! 任意の投稿の履歴の frame（AC-3）。
//!
//! 招待は移行元が出す短命・1 回限りの接続情報で、アカウントの秘密鍵・チャンネルの秘密を含まない。
//! 秘密の値は移行先が移行元へ自分を証明する鍵にだけ使い、確認コードの導出にも両端末の endpoint id を束縛する。

use std::net::SocketAddr;

use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use secp256k1::rand::{RngCore, rng};
use serde::{Deserialize, Serialize};

use crate::{AccountSyncItem, AccountSyncItemKey, AccountSyncKeys, Pubkey, SealedAccountSyncItem};

/// リンクの形。秘密は fragment にだけ置き、HTTP の query・Referer へ出さない。
pub const ACCOUNT_TRANSFER_LINK_PREFIX: &str = "kukuri://transfer#v1.";
/// リンクの長さの上限。
pub const MAX_ACCOUNT_TRANSFER_LINK_BYTES: usize = 1024;
/// 招待の有効期間。
pub const ACCOUNT_TRANSFER_INVITE_TTL_MS: i64 = 5 * 60 * 1000;
/// 招待に載せる直接の addr の上限。
pub const MAX_ACCOUNT_TRANSFER_DIRECT_ADDRS: usize = 4;
const MAX_RELAY_URL_BYTES: usize = 256;
const HELLO_DOMAIN: &[u8] = b"kukuri account transfer hello v1\0";
const CODE_CONTEXT: &str = "kukuri.app 2026-10-02 account transfer code v1";

/// 移行元の接続情報と、1 回限りの秘密。秘密を含むので `Debug` と log へ中身を出さない。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountTransferInvite {
    /// 移行元の endpoint id（hex）。移行先はこの id の端末だけへ接続する。
    pub issuer: String,
    pub relay_url: Option<String>,
    pub direct_addrs: Vec<SocketAddr>,
    #[serde(with = "hex_secret")]
    secret: [u8; 32],
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for AccountTransferInvite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountTransferInvite")
            .field("issuer", &self.issuer)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

impl AccountTransferInvite {
    /// 新しい秘密で招待を作る。直接の addr は先頭から上限まで載せる。
    pub fn issue(
        issuer: String,
        relay_url: Option<String>,
        direct_addrs: impl IntoIterator<Item = SocketAddr>,
        now_ms: i64,
    ) -> Result<Self> {
        let mut secret = [0; 32];
        rng().fill_bytes(&mut secret);
        let invite = Self {
            issuer,
            relay_url,
            direct_addrs: direct_addrs
                .into_iter()
                .take(MAX_ACCOUNT_TRANSFER_DIRECT_ADDRS)
                .collect(),
            secret,
            expires_at_ms: now_ms + ACCOUNT_TRANSFER_INVITE_TTL_MS,
        };
        invite.validate()?;
        Ok(invite)
    }

    pub fn to_link(&self) -> String {
        let json = serde_json::to_vec(self).expect("invite serializes");
        format!("{ACCOUNT_TRANSFER_LINK_PREFIX}{}", BASE64_URL.encode(json))
    }

    /// リンクを読む。形式・長さ・期限を確かめる（期限の正本は移行元が持つ）。
    pub fn parse_link(link: &str, now_ms: i64) -> Result<Self> {
        let link = link.trim();
        ensure!(
            link.len() <= MAX_ACCOUNT_TRANSFER_LINK_BYTES,
            "account transfer link is too long"
        );
        let payload = link
            .strip_prefix(ACCOUNT_TRANSFER_LINK_PREFIX)
            .context("not an account transfer link")?;
        let json = BASE64_URL
            .decode(payload)
            .context("invalid account transfer link")?;
        let invite: Self =
            serde_json::from_slice(&json).context("invalid account transfer link")?;
        invite.validate()?;
        ensure!(
            now_ms < invite.expires_at_ms,
            "account transfer link expired"
        );
        Ok(invite)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.issuer.len() == 64 && self.issuer.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid account transfer issuer"
        );
        ensure!(
            self.relay_url
                .as_ref()
                .is_none_or(|url| url.len() <= MAX_RELAY_URL_BYTES),
            "account transfer relay url is too long"
        );
        ensure!(
            self.direct_addrs.len() <= MAX_ACCOUNT_TRANSFER_DIRECT_ADDRS,
            "too many account transfer addresses"
        );
        Ok(())
    }

    /// 移行先が移行元へ送る証明。招待の秘密を鍵にし、両端末の endpoint id に束縛する。
    pub fn hello_proof(&self, target: &[u8; 32]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_keyed(&self.secret);
        hasher.update(HELLO_DOMAIN);
        hasher.update(self.issuer.as_bytes());
        hasher.update(target);
        *hasher.finalize().as_bytes()
    }

    /// 証明が正しいか。比較は定数時間で行う。
    pub fn verify_hello(&self, target: &[u8; 32], proof: &[u8; 32]) -> bool {
        blake3::Hash::from_bytes(self.hello_proof(target)) == blake3::Hash::from_bytes(*proof)
    }

    /// 両端末に出す 6 桁の確認コード。
    pub fn verification_code(&self, target: &[u8; 32]) -> String {
        let mut material = Vec::with_capacity(32 + 64 + 32);
        material.extend_from_slice(&self.secret);
        material.extend_from_slice(self.issuer.as_bytes());
        material.extend_from_slice(target);
        let key = blake3::derive_key(CODE_CONTEXT, &material);
        let value = u32::from_le_bytes([key[0], key[1], key[2], key[3]]) % 1_000_000;
        format!("{value:06}")
    }
}

/// 移行の画面へ返す状態。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AccountTransferStatus {
    Idle,
    /// 移行元: 招待を出し、移行先の接続を待っている。
    Waiting {
        expires_at_ms: i64,
    },
    /// 移行先: 移行元へ接続している。
    Connecting,
    /// 両端末: 確認コードを出し、承認を待っている。
    Confirming {
        role: AccountTransferRole,
        code: String,
        local_accepted: bool,
    },
    /// 両端末の承認の後、必須 bundle を送っている・保存している（送った・保存した item の数）。
    Transferring {
        role: AccountTransferRole,
        items: u64,
    },
    /// 必須の移行を終えた後、投稿の履歴を送っている・受けている（AC-3。送った・保存した投稿と、移行元に無かった
    /// 本文・添付の数）。移行先の `account_id` は受けたアカウント。
    History {
        role: AccountTransferRole,
        account_id: Option<String>,
        posts: u64,
        unavailable: u64,
    },
    /// 移行元は移行先の保存の ACK を受けた。移行先は保存を確定した（`account_id` は受けたアカウント）。履歴を選んだ
    /// ときは、その結果も持つ。
    Completed {
        role: AccountTransferRole,
        account_id: Option<String>,
        #[cfg_attr(feature = "ts", ts(inline))]
        history: Option<AccountTransferHistoryResult>,
    },
    Failed {
        role: AccountTransferRole,
        reason: AccountTransferFailure,
    },
}

impl AccountTransferStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }
}

/// 移行先が選ぶ投稿の履歴の範囲（AC-3）。履歴を移さないときは選ばない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AccountTransferHistory {
    /// 直近 30 日の時間 bucket。
    Month,
    /// 直近 1 年の時間 bucket。
    Year,
    /// すべて（時間 bucket へ切り替える前の旧形式の記録を含む）。
    All,
}

impl AccountTransferHistory {
    /// 範囲の日数。`All` は期間で絞らない。
    pub fn days(self) -> Option<i64> {
        match self {
            Self::Month => Some(30),
            Self::Year => Some(365),
            Self::All => None,
        }
    }
}

/// 履歴の結果（AC-3）: 送った・保存した投稿の数（`posts`）、移行元に無く移せなかった本文・添付の数
/// （`unavailable`）、範囲の終わりまで移せなかったときの理由（`stopped`。もう一度つないで同じ範囲を選ぶと、続きから
/// 移す）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AccountTransferHistoryResult {
    pub posts: u64,
    pub unavailable: u64,
    pub stopped: Option<AccountTransferFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AccountTransferRole {
    Source,
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AccountTransferFailure {
    /// 招待の期限切れ、または確認の待ち時間切れ。
    Expired,
    /// 招待が無効・使用済み・改竄されている。
    Invalid,
    /// 移行元へ接続できない。
    Unreachable,
    /// どちらかの端末で「一致しない」を選んだ。
    Rejected,
    Cancelled,
    /// 確認・転送の途中で接続が切れた。
    Interrupted,
    /// 移行先で必須 bundle を保存できなかった（容量不足など）。
    Storage,
}

/// 1 つの Items・Records の frame の件数の上限。
pub const MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS: usize = 64;
/// 1 つの frame の JSON の上限。
pub const MAX_ACCOUNT_TRANSFER_FRAME_BYTES: usize = 1024 * 1024;
/// 1 つの Blob の frame で運ぶ bytes の上限（base64 にしても 1 frame に収まる）。
pub const MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES: usize = 512 * 1024;

/// 履歴の続きの位置（AC-3）。移行元の保護所有先の record の索引（保護参照・replica・key・docs author の順）で、この
/// 次から読む。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountHistoryCursor {
    pub reference: String,
    pub replica: String,
    pub key: String,
    pub author: String,
}

/// 履歴の 1 件（AC-3）: 移行元の保護所有先の自分の record。docs の entry の値をそのまま運ぶ（hash と長さは値から
/// 求める）。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountHistoryRecord {
    pub replica: String,
    pub key: String,
    pub docs_author: String,
    #[serde(with = "base64_bytes")]
    pub value: Vec<u8>,
}

/// 値は投稿の本文を含みうるので出さない。
impl std::fmt::Debug for AccountHistoryRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountHistoryRecord")
            .field("replica", &self.replica)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// 必須 bundle の 1 件: 移行元の account の replica の docs の key と、その封（ADR 0061 §3）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountTransferItem {
    pub key: String,
    pub sealed: SealedAccountSyncItem,
}

impl AccountTransferItem {
    /// アカウントの鍵で開いて確かめる。封の認証（AAD はアカウントの公開鍵と docs の key）と、移行する種類
    /// （profile・表示例外・channel の参加・世代の鍵・担当。変更の窓は送らない）を見る。
    pub fn open(&self, keys: &AccountSyncKeys, account: &Pubkey) -> Result<AccountSyncItem> {
        AccountSyncItemKey::from_docs_key(&self.key)?;
        keys.open(account, &self.key, &self.sealed)
    }
}

/// 両端末の承認の後、同じ接続で送る frame。必須 bundle の stream では、移行元が Key・Items（0 回以上）・End の順に
/// 送る。履歴の stream（AC-3）では、移行先が History を送り、移行元が page ごとに Records・Blob（0 回以上）・Page を
/// 送る。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountTransferFrame {
    /// アカウントの秘密鍵（hex）。
    Key {
        secret: String,
    },
    Items {
        items: Vec<AccountTransferItem>,
    },
    /// 送った item の総数。
    End {
        count: u64,
    },
    /// 履歴の範囲（最初の時間 bucket。すべては `None`）と、続きの位置（最初は `None`）。
    History {
        since: Option<u64>,
        cursor: Option<AccountHistoryCursor>,
    },
    Records {
        records: Vec<AccountHistoryRecord>,
    },
    /// 本文・添付の blob（長さ `len`）の `offset` からの一部。0 から順に、長さに届くまで送る。
    Blob {
        hash: String,
        len: u64,
        offset: u64,
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
    /// 1 page の終わり。次の位置（範囲の終わりは `None`）と、この page の投稿の数・移行元に無かった本文・添付の数。
    Page {
        next: Option<AccountHistoryCursor>,
        posts: u64,
        unavailable: u64,
    },
}

impl std::fmt::Debug for AccountTransferFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Key { .. } => f.write_str("Key { .. }"),
            Self::Items { items } => write!(f, "Items {{ {} items }}", items.len()),
            Self::End { count } => write!(f, "End {{ count: {count} }}"),
            Self::History { since, .. } => write!(f, "History {{ since: {since:?}, .. }}"),
            Self::Records { records } => write!(f, "Records {{ {} records }}", records.len()),
            Self::Blob { len, offset, .. } => write!(f, "Blob {{ len: {len}, offset: {offset} }}"),
            Self::Page {
                next,
                posts,
                unavailable,
            } => write!(
                f,
                "Page {{ last: {}, posts: {posts}, unavailable: {unavailable} }}",
                next.is_none()
            ),
        }
    }
}

impl AccountTransferFrame {
    /// item を、64 件・1 MiB までの Items の frame に分ける。
    pub fn chunks(items: Vec<AccountTransferItem>) -> Result<Vec<Self>> {
        split(items, |items| Self::Items { items })
    }

    /// 履歴の record を、64 件・1 MiB までの Records の frame に分ける。
    pub fn record_chunks(records: Vec<AccountHistoryRecord>) -> Result<Vec<Self>> {
        split(records, |records| Self::Records { records })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)?;
        ensure!(
            bytes.len() <= MAX_ACCOUNT_TRANSFER_FRAME_BYTES,
            "account transfer frame is too large"
        );
        Ok(bytes)
    }

    /// frame を読む。大きさと、Items・Records の件数、Blob の一部の大きさと範囲を確かめる。
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_ACCOUNT_TRANSFER_FRAME_BYTES,
            "account transfer frame is too large"
        );
        let frame: Self =
            serde_json::from_slice(bytes).context("invalid account transfer frame")?;
        let count = match &frame {
            Self::Items { items } => items.len(),
            Self::Records { records } => records.len(),
            Self::Blob {
                len, offset, bytes, ..
            } => {
                ensure!(
                    (1..=MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES).contains(&bytes.len())
                        && offset
                            .checked_add(bytes.len() as u64)
                            .is_some_and(|end| end <= *len),
                    "account history blob part is out of range"
                );
                return Ok(frame);
            }
            _ => return Ok(frame),
        };
        ensure!(
            (1..=MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS).contains(&count),
            "account transfer chunk must contain 1..={MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS} items"
        );
        Ok(frame)
    }
}

/// `items` を、64 件・1 MiB までの `wrap` の frame に分ける。
fn split<T: Serialize>(
    items: Vec<T>,
    wrap: impl Fn(Vec<T>) -> AccountTransferFrame,
) -> Result<Vec<AccountTransferFrame>> {
    let empty = serde_json::to_vec(&wrap(Vec::new()))?.len();
    let mut frames = Vec::new();
    let mut chunk = Vec::new();
    let mut bytes = empty;
    for item in items {
        let size = serde_json::to_vec(&item)?.len();
        // 2 件目からは区切りの `,` が 1 byte 増える。
        if !chunk.is_empty()
            && (chunk.len() == MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS
                || bytes + 1 + size > MAX_ACCOUNT_TRANSFER_FRAME_BYTES)
        {
            frames.push(wrap(std::mem::take(&mut chunk)));
            bytes = empty;
        }
        bytes += usize::from(!chunk.is_empty()) + size;
        ensure!(
            bytes <= MAX_ACCOUNT_TRANSFER_FRAME_BYTES,
            "account transfer item is too large"
        );
        chunk.push(item);
    }
    if !chunk.is_empty() {
        frames.push(wrap(chunk));
    }
    Ok(frames)
}

mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        STANDARD
            .decode(String::deserialize(deserializer)?)
            .map_err(D::Error::custom)
    }
}

mod hex_secret {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(value: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        let mut out = [0; 32];
        hex::decode_to_slice(text, &mut out).map_err(D::Error::custom)?;
        Ok(out)
    }
}
