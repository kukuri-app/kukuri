//! デスクトップが対向する community node の HTTP パス(cn endpoint contract の一部)。
//!
//! cn-user-api の route 定義と desktop-runtime の URL 構築が同じ定数を参照することで、
//! パス変更が両側にコンパイルエラーとして届く(文字列の二重持ちを解消。WP-H3 PR2)。

pub const AUTH_CHALLENGE_PATH: &str = "/v1/auth/challenge";
pub const AUTH_VERIFY_PATH: &str = "/v1/auth/verify";
pub const CONSENTS_PATH: &str = "/v1/consents";
pub const CONSENTS_STATUS_PATH: &str = "/v1/consents/status";
/// 認証不要の公開 policy カタログ(#857)。Node 同意はこのカタログの提示で成立させ、
/// 認証後に POST /v1/consents で記録を同期する。
pub const POLICIES_PATH: &str = "/v1/policies";
pub const BOOTSTRAP_NODES_PATH: &str = "/v1/bootstrap/nodes";
pub const BOOTSTRAP_HEARTBEAT_PATH: &str = "/v1/bootstrap/heartbeat";
pub const NODE_MANIFEST_PATH: &str = "/v1/node/manifest";
pub const TOPIC_RENDEZVOUS_HEARTBEAT_PATH: &str = "/v1/rendezvous/topics/heartbeat";
pub const INDEXING_REQUESTS_PATH: &str = "/v1/indexing/requests";
/// 自分の索引申請の状態と、任意の対象が supported set に含まれるかの読取り(#975)。
pub const INDEXING_STATUS_PATH: &str = "/v1/indexing/status";
pub const INDEX_SEARCH_PATH: &str = "/v1/index/search";
pub const INDEX_DISCOVERY_PATH: &str = "/v1/index/discovery";
pub const INDEX_RECOMMENDATIONS_PATH: &str = "/v1/index/recommendations";
/// タイムライン向け content advisory 一括照会(#1056)。
pub const ADVISORY_LOOKUP_PATH: &str = "/v1/advisories/lookup";
/// 公開 blob の保持端末の検索（#1632）。
pub const BLOB_PROVIDER_SEARCH_PATH: &str = "/v1/blob-providers/search";
pub const TRUST_USERS_PATH_PREFIX: &str = "/v1/trust/users/";
pub const TRUST_USERS_ROUTE: &str = "/v1/trust/users/{pubkey}";
/// 閲覧者向け信頼値の一括評価(#1061)。
pub const TRUST_EVALUATIONS_PATH: &str = "/v1/trust/evaluations";
/// ブロック / ミュート観測の提供と取消(#1061)。
pub const TRUST_OBSERVATIONS_PATH: &str = "/v1/trust/observations";
pub const RELATION_USERS_PATH_PREFIX: &str = "/v1/relation/users/";
pub const RELATION_USERS_ROUTE: &str = "/v1/relation/users/{target}";
pub const RELATION_NEIGHBORS_PATH: &str = "/v1/relation/neighbors";
pub const RELATION_OPTOUT_PATH: &str = "/v1/relation/optout";
pub const DOME_HOSTING_ASSIGNMENTS_PATH: &str = "/v1/dome-hosting/assignments";
pub const DOME_HOSTING_ACTIVATE_PATH: &str = "/v1/dome-hosting/activate";
pub const DOME_HOSTING_RELEASE_PATH: &str = "/v1/dome-hosting/release";
pub const DOME_HOSTING_STATUS_ROUTE: &str = "/v1/dome-hosting/status/{instance_id}";
pub const DOME_HOSTING_SESSION_INPUT_PATH: &str = "/v1/dome-hosting/session/input";
pub const DOME_HOSTING_SESSION_WS_PATH: &str = "/v1/dome-hosting/session/ws";
pub const DOME_HOSTING_LAYOUT_CANDIDATE_PATH: &str = "/v1/dome-hosting/session/layout-candidate";
pub const DOME_HOSTING_SNAPSHOT_RESYNC_PATH: &str = "/v1/dome-hosting/session/resync";
pub const DOME_TRANSITION_PREPARE_PATH: &str = "/v1/dome-hosting/transition/prepare";
pub const DOME_TRANSITION_COMMIT_PATH: &str = "/v1/dome-hosting/transition/commit";
pub const DOME_TRANSITION_ABORT_PATH: &str = "/v1/dome-hosting/transition/abort";
/// 通報受付。client は manifest の `report_endpoint` から動的に解決するため
/// 直接この定数で URL を組み立てるのはサーバ側(route 定義)のみ。
pub const REPORT_PATH: &str = "/v1/report";
/// テスターフィードバック受付(#802 / ADR 0039)。
pub const TESTER_FEEDBACK_PATH: &str = "/v1/tester-feedback";
