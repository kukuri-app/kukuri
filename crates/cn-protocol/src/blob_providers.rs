//! 公開 blob の保持端末の検索の wire contract（#1632 AC-5、ADR 0063 §7）。
//!
//! client（Web）は公開 blob の hash だけを送り、node は Mainline DHT と kukuri の補助 index で保持端末を探して、
//! relay URL を持つ候補の endpoint ID と住所だけを返す。node は blob を取得・保存しない。候補は保持・権限の証明では
//! なく、client は既存の接続と hash の検証で取得する。

use serde::{Deserialize, Serialize};

/// 1 応答の候補の上限。
pub const BLOB_PROVIDER_SEARCH_MAX_CANDIDATES: usize = 4;
/// 1 候補の relay URL と、直接の address の上限。
pub const BLOB_PROVIDER_MAX_RELAY_URLS: usize = 4;
pub const BLOB_PROVIDER_MAX_DIRECT_ADDRS: usize = 8;
/// 検索の期限の上限（ms）。node は受け取ってから `min(budget_ms, これ)` で打ち切る。
pub const BLOB_PROVIDER_SEARCH_MAX_BUDGET_MS: u64 = 10_000;

/// 要求の形式が不正（hash が 64 桁の 16 進でない・budget が 0）なときの安定コード（400）。
pub const INVALID_BLOB_PROVIDER_SEARCH_CODE: &str = "INVALID_BLOB_PROVIDER_SEARCH";

/// `POST /v1/blob-providers/search` の要求本文。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobProviderSearchRequest {
    /// 公開 blob の BLAKE3 hash（64 桁の 16 進）。
    pub hash: String,
    /// client の取得に残る期限（ms）。
    pub budget_ms: u64,
}

/// `POST /v1/blob-providers/search` の応答本文。候補が空でも、保持端末が無いことの証明ではない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobProviderSearchResponse {
    pub candidates: Vec<BlobProviderCandidate>,
    /// 期限か処理の上限で、候補を集め終える前に打ち切った。
    pub partial: bool,
}

/// 保持端末の候補（署名つきの住所 record から読んだ到達情報）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobProviderCandidate {
    pub endpoint_id: String,
    pub relay_urls: Vec<String>,
    #[serde(default)]
    pub direct_addrs: Vec<String>,
}
