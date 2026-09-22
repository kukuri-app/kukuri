//! Shared wire contracts for Community Node index queries (Issue #663).
//!
//! The server and desktop runtime both use these types so field and variant
//! names cannot drift between the two sides of the HTTP boundary.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub use kukuri_cn_safety::{AdvisorySubjectKind, ContentAdvisory};

/// Scope kinds supported by the Community Node index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum IndexScopeKind {
    PublicTopic,
    PrivateChannel,
}

/// indexing request の処理状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum IndexingRequestStatus {
    Pending,
    Approved,
    Rejected,
}

impl IndexingRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            other => bail!("unknown indexing request status `{other}`"),
        }
    }
}

/// `POST /v1/indexing/requests` の wire request。
///
/// `kind` は server が stable な `INVALID_INDEXING_REQUEST` を返せるよう文字列のまま保持する。
/// private channel の secret を含むため TypeScript へは export しない。
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitIndexingRequestRequest {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_secret_hex: Option<String>,
}

impl std::fmt::Debug for SubmitIndexingRequestRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SubmitIndexingRequestRequest")
            .field("kind", &self.kind)
            .field("target_id", &self.target_id)
            .field(
                "channel_secret_hex",
                &self.channel_secret_hex.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SubmitIndexingRequestResponse {
    pub request_id: String,
    pub status: IndexingRequestStatus,
}

/// `GET /v1/indexing/status` の query(#975)。
///
/// `scope_kind` と `scope_id` は両方あるか両方ないか。両方あれば `target` の supported 判定を
/// 併せて返し、無ければ自分の申請一覧だけを返す。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct IndexingStatusParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
}

/// 呼出し主自身の索引申請 1 件の現在状態(#975)。
///
/// `requester_pubkey` は bearer identity と一致するため wire には載せない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct IndexingRequestView {
    pub request_id: String,
    pub scope_kind: IndexScopeKind,
    pub target_id: String,
    pub status: IndexingRequestStatus,
    /// 申請時刻(unix ms)。
    pub created_at: i64,
    /// 承認・却下時刻(unix ms)。審査待ちでは null。
    pub decided_at: Option<i64>,
}

/// 指定した対象がこのノードの supported set に含まれるか(#975)。
///
/// `supported` は scope ゲート(ADR 0025 §2.6 の 1 段目)だけを表す。個々の投稿が検索に出るかは
/// safety ゲートと sync の反映に依存するため、これだけで「索引済み」とは断定しない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IndexingTargetStatus {
    pub scope_kind: IndexScopeKind,
    pub scope_id: String,
    pub supported: bool,
}

/// `GET /v1/indexing/status` の応答(#975)。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct IndexingStatusResponse {
    /// 呼出し主の申請だけ。他利用者の申請は含まない。
    pub requests: Vec<IndexingRequestView>,
    /// `scope_kind` / `scope_id` を指定した場合のみ。非公開チャンネルは所属証明が必要。
    pub target: Option<IndexingTargetStatus>,
}

impl IndexScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PublicTopic => "public_topic",
            Self::PrivateChannel => "private_channel",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "public_topic" => Ok(Self::PublicTopic),
            "private_channel" => Ok(Self::PrivateChannel),
            other => bail!("unknown index scope kind `{other}`"),
        }
    }
}

/// 非公開チャンネル範囲指定読みの所属証明(channel secret hex)を運ぶ HTTP ヘッダ名(#711)。
///
/// 秘密値を URL クエリへ載せるとアクセスログに露出するため、専用ヘッダで送る。
/// 提示された値はサーバが保存済み capability の復号値と照合する(ADR 0025 §6.3
/// 「秘密値の提示が権限の証明」を read にも適用)。
pub const CHANNEL_MEMBERSHIP_SECRET_HEADER: &str = "x-kukuri-channel-secret";

/// 非公開チャンネル範囲指定読みで所属証明が満たされないときの安定コード(#711)。
///
/// 未提示・不一致・チャンネル未登録・暗号鍵未設定を区別せず同一コード(403)で返し、
/// 非所属者に索引の存在有無を漏らさない。
pub const CHANNEL_MEMBERSHIP_REQUIRED_CODE: &str = "CHANNEL_MEMBERSHIP_REQUIRED";

/// Query parameters shared by search, discovery, and recommendations.
///
/// `scope_kind` and `scope_id` must either both be present or both be absent.
/// The HTTP handler validates that cross-field rule.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct IndexQueryParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// One projected index result.
///
/// `text` may contain derived tags and is not canonical post content.
/// `content_advisories` は issuer node の node-local な content advisory（ADR 0028 §8.6 /
/// ADR 0025 §7.2）。署名済み `content_labels` とは別欄で、canonical でも署名対象でもない。
/// client は署名済み投稿を解決した後も第 2 のラベル源として保持する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IndexEntryView {
    pub scope_kind: IndexScopeKind,
    pub scope_id: String,
    pub object_id: String,
    pub author_pubkey: String,
    pub text: String,
    pub created_at: i64,
    // 公開bucket投稿の保存先の手がかり。clientはscopeと署名を検証し、権限の証明にはしない。
    // 旧形式とprivate epoch未対応の応答では省略し、既存の解決経路を維持する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub source_replica_id: Option<String>,
    #[serde(default)]
    pub content_advisories: Vec<ContentAdvisory>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IndexQueryResponse {
    pub entries: Vec<IndexEntryView>,
}

/// Stable non-2xx JSON body returned by `cn-user-api`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
}
