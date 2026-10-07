//! コミュニティノード HTTP 境界の安定エラーコード(#712)。
//!
//! クライアントは機能の未提供・有効化の失効・利用拒否をこのコードで見分けて縮退する
//! (#670)。文字列はサーバ(`cn-user-api`)の発行、実行時層(`desktop-runtime`)の判別・
//! 後退処理、試験場面のスタブ(`harness`)が共有する通信契約であり、変更は互換性の検討と
//! セットで行うこと。通信境界の実値はサーバ側契約試験が `body.code` で固定する。
//!
//! TypeScript 側(`apps/desktop`)は生成型を持たないため文字列リテラルで判別する。
//! コード名を変更する場合はサーバ契約試験の失敗が検知装置になる。

/// 認証(bearer)が必要・無効(401)。
pub const AUTH_REQUIRED_CODE: &str = "AUTH_REQUIRED";

/// 必須ポリシーへの同意が未完了(403)。
pub const CONSENT_REQUIRED_CODE: &str = "CONSENT_REQUIRED";

/// このノードは索引を提供しないため索引申請を受け付けない(未構成。404。#713)。
pub const INDEXING_REQUEST_NOT_CONFIGURED_CODE: &str = "INDEXING_REQUEST_NOT_CONFIGURED";

/// 索引の有効化が失効しているため索引申請を受け付けない(404。#713)。
pub const INDEXING_REQUEST_NOT_ACTIVATED_CODE: &str = "INDEXING_REQUEST_NOT_ACTIVATED";

/// このノードは索引参照を提供しない(未構成。404)。
pub const INDEX_QUERY_NOT_CONFIGURED_CODE: &str = "INDEX_QUERY_NOT_CONFIGURED";

/// 索引参照の有効化(準備完了記録)が失効している(404)。
pub const INDEX_QUERY_NOT_ACTIVATED_CODE: &str = "INDEX_QUERY_NOT_ACTIVATED";

/// このノードは信頼読み取りを提供しない(未構成。404)。
pub const TRUST_READ_NOT_CONFIGURED_CODE: &str = "TRUST_READ_NOT_CONFIGURED";

/// 信頼読み取りの有効化が失効している(404)。
pub const TRUST_READ_NOT_ACTIVATED_CODE: &str = "TRUST_READ_NOT_ACTIVATED";

/// 観測提供の同意(任意文書 `trust_observation_sharing`)が成立していない(403)。
pub const TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED_CODE: &str =
    "TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED";

/// このノードは観測提供の同意文書を公開していない(404)。
pub const TRUST_OBSERVATION_SHARING_NOT_OFFERED_CODE: &str =
    "TRUST_OBSERVATION_SHARING_NOT_OFFERED";

/// 観測 envelope が不正(署名・署名者・種別・件数。400)。
pub const INVALID_TRUST_OBSERVATION_CODE: &str = "INVALID_TRUST_OBSERVATION";

/// 対象の関係観測が存在しない(404)。
pub const RELATION_NOT_FOUND_CODE: &str = "RELATION_NOT_FOUND";

/// このノードは距離利用停止(relation visibility)を提供しない(未構成。404)。
pub const RELATION_VISIBILITY_NOT_CONFIGURED_CODE: &str = "RELATION_VISIBILITY_NOT_CONFIGURED";

/// 距離利用停止の有効化が失効している(404)。
pub const RELATION_VISIBILITY_NOT_ACTIVATED_CODE: &str = "RELATION_VISIBILITY_NOT_ACTIVATED";

/// このノードはテスターフィードバックを受け付けない(未構成。404。#802)。
pub const TESTER_FEEDBACK_NOT_CONFIGURED_CODE: &str = "TESTER_FEEDBACK_NOT_CONFIGURED";

/// テスターフィードバックの入力が不正(空欄・文字数超過。400。#802)。
pub const INVALID_TESTER_FEEDBACK_CODE: &str = "INVALID_TESTER_FEEDBACK";

/// このノードは公開 blob の保持端末の検索を提供しない(未構成。404。#1632)。
pub const BLOB_PROVIDER_SEARCH_NOT_CONFIGURED_CODE: &str = "BLOB_PROVIDER_SEARCH_NOT_CONFIGURED";

/// 公開 blob の保持端末の検索の受付が満杯、または同じ端末の進行中の検索が上限(429。`Retry-After` つき。#1632)。
pub const BLOB_PROVIDER_SEARCH_BUSY_CODE: &str = "BLOB_PROVIDER_SEARCH_BUSY";
