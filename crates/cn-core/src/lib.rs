//! コミュニティノードのサーバ側 合成・永続化層。
//!
//! 収容基準: **Postgres / Redis を伴うサーバ側の合成・永続化だけ**をここに置く。
//! 次に該当するものは本 crate に足さない:
//! - wire 型・正規化・認証封筒(serde 表現 = 凍結境界)→ `kukuri-cn-protocol`
//! - ドメインロジック(safety / trust の純粋規則)→ `kukuri-cn-safety` / `kukuri-cn-trust`
//!   (DB 非依存の safety 合成は `kukuri-cn-safety-runtime`。WP-Q8)
//! - 常駐プロセス → `kukuri-cn-indexer` / `kukuri-cn-user-api`、運用 CLI → `kukuri-cn-cli`
//! - tracing 等の実行時共通部品 → `kukuri-cn-runtime-support`
//!
//! 新しいサーバ側機能は、まず上のどれに属するかを判定し、DB との合成・永続化に
//! 該当する場合だけ本 crate へ足す(「何でも入る受け皿」に戻さない。WP-B10)。

mod admission;
mod advisory_lookup;
mod appeal_reviews;
mod auth;
mod bootstrap;
mod co_participation;
mod config;
mod consents;
mod content_scan;
mod database;
mod dome_hosting;
mod env;
mod errors;
mod index_entries;
mod index_scope;
mod legal_data;
mod legal_holds;
#[cfg(feature = "safety-openai-provider")]
mod moderation_budget;
mod operator_actions;
mod readiness_activation;
mod readiness_probe;
mod readiness_runtime;
mod relation_optouts;
mod rendezvous;
mod reports;
mod retention;
mod rights_request_sensitive;
mod rights_requests;
mod rollout;
mod safety_appeals;
mod safety_events;
mod safety_runtime;
mod scan_verdicts;
mod tester_feedback;
#[cfg(test)]
mod tests;
mod transmission_preventions;
mod trust_inputs;
mod trust_observations;

pub use admission::{
    AdmissionConfig, AdmissionMode, AdmissionRejection, AllowlistEntry, BannedEntry,
    InviteCodeSummary, add_allowlist, ban_subscriber, invite_code_hash, issue_invite_code,
    list_allowlist, list_banned, list_invite_codes, load_admission_config, remove_allowlist,
    revoke_invite_code, set_admission_mode, unban_subscriber,
};
pub use advisory_lookup::list_content_advisories_for_subjects;
pub use appeal_reviews::{
    AppealReview, AppealReviewOperation, AppealReviewReport, AppealReviewVersion,
    apply_appeal_review_action, get_appeal_review, list_appeal_reviews,
};
pub use auth::{
    create_auth_challenge, require_bearer_identity, require_bearer_pubkey,
    verify_auth_envelope_and_issue_token,
};
pub use bootstrap::{
    load_bootstrap_nodes, load_bootstrap_seed_peers, refresh_bootstrap_peer_registration,
    upsert_bootstrap_node,
};
pub use co_participation::{
    AuthorDominantTopic, CoParticipationPair, CoParticipationSource, PgCoParticipationSource,
};
pub use config::{
    AUTH_CHALLENGE_TTL_SECONDS, AUTH_EVENT_MAX_SKEW_SECONDS, AuthMode, AuthRolloutConfig,
    BOOTSTRAP_PEER_REGISTRATION_TTL_SECONDS, COMMUNITY_NODE_ADMISSION_SERVICE_NAME,
    COMMUNITY_NODE_AUTH_SERVICE_NAME, COMMUNITY_NODE_DATABASE_INIT_MODE_ENV,
    COMMUNITY_NODE_RENDEZVOUS_KEY_PREFIX_ENV, COMMUNITY_NODE_RENDEZVOUS_REDIS_URL_ENV,
    DEFAULT_TOKEN_TTL_SECONDS, DatabaseInitMode, JwtConfig, TOPIC_RENDEZVOUS_TTL_SECONDS,
    USER_API_BEARER_CHALLENGE,
};
pub use consents::{
    accept_consents, get_consent_status, get_policy_revision, get_policy_snapshot_revision,
    list_policies, list_policies_for_language, list_policy_revisions, require_consents,
    sync_policies,
};
pub use content_scan::PgContentScanStore;
pub use database::{
    TestDatabase, connect_postgres, ensure_database_ready, initialize_database,
    initialize_database_for_runtime, migrate_postgres, migrate_postgres_up_to,
    seed_default_policies,
};
pub use dome_hosting::{
    COMMUNITY_NODE_DOME_BLOB_CACHE_CAPACITY_BYTES, DOME_BLOB_CACHE_GC_GRACE_MILLIS,
    DomeHostingAssignment, NewDomeHostingAssignment, StagedDomeBlob,
    activate_dome_hosting_assignment, activate_dome_hosting_blob_pins,
    close_dome_hosting_assignment, collect_dome_blob_cache_garbage, get_dome_hosting_assignment,
    list_recoverable_dome_hosting_assignments, release_dome_hosting_blob_pins,
    stage_dome_hosting_blobs, upsert_pending_dome_hosting_assignment,
};
pub use env::{parse_bool_env, parse_csv_env, parse_u32_env, parse_u64_env};
pub use errors::{ApiError, ApiResult, auth_required_error, consent_required_error};
pub use index_entries::{
    IndexEntryStore, MemoryIndexEntryStore, NewIndexEntry, PgIndexEntryStore, StoredIndexEntry,
    SurfaceableEntry, filter_surfaceable_objects, get_index_entry, remove_index_entry,
    remove_index_scope, upsert_index_entry,
};
pub use index_scope::{
    ChannelSecret, ChannelSecretCipher, ChannelSecretConflict, IndexScopeKind, IndexingRequest,
    IndexingRequestStatus, SupportedTopic, add_supported_topic, approve_indexing_request,
    get_channel_secret, insert_indexing_request, is_topic_supported, list_channel_secrets,
    list_indexing_requests, list_indexing_requests_for_requester, list_supported_topics,
    mark_index_demand, register_channel_secret, register_channel_secret_with_epoch,
    reject_indexing_request, remove_channel_secret, remove_supported_topic,
    revoke_indexing_request, rotate_channel_secret_epoch, upsert_channel_secret,
};
pub use legal_data::{
    LegalDataCipher, SensitiveDataCategory, load_sensitive_json, upsert_sensitive_json_in_tx,
    verify_sensitive_items,
};
pub use legal_holds::{
    LegalHold, LegalHoldExport, export_legal_hold, release_legal_hold, start_legal_hold,
};
#[cfg(feature = "safety-openai-provider")]
pub use moderation_budget::PgModerationBudget;
pub use operator_actions::{
    AdminOperation, OperatorAction, OperatorReportStatus, apply_operator_action,
    list_operator_actions, validate_admin_operation,
};
pub use readiness_activation::{
    ReadinessActivation, latest_readiness_activation, readiness_context_fingerprint,
    record_readiness_activation, record_readiness_revocation,
};
pub use readiness_probe::{ReadinessProbeRecord, list_readiness_probes, upsert_readiness_probe};
pub use readiness_runtime::{
    IndexIntegrityFindings, RelationAnalyzeRun, inspect_index_integrity,
    latest_relation_analyze_run, record_relation_analyze_run,
};
pub use relation_optouts::{
    clear_relation_optout, filter_relation_visible, get_relation_optout, is_relation_opted_out,
    relation_pair_is_suppressed, set_relation_optout, should_suppress_relation_pair,
};
pub use rendezvous::TopicRendezvousStore;
pub use reports::{
    COMMUNITY_NODE_REPORT_STATUS_RECEIVED, CommunityNodeReport, NewCommunityNodeReport,
    get_community_node_report, get_community_node_report_with_contact,
    insert_community_node_appeal, insert_community_node_appeal_with_retention,
    insert_community_node_report, insert_community_node_report_with_retention,
    list_community_node_reports, seal_legacy_report_contacts,
};
pub use retention::{
    CleanupCounts, RetentionPolicy, apply_retention_policy, cleanup_expired, retention_counts,
};
pub use rights_request_sensitive::seal_legacy_rights_request_data;
pub use rights_requests::{
    CreatedRightsRequest, RightsRequestActionResult, RightsRequestEvent, RightsRequestRecord,
    action_rights_request, get_public_rights_request_status, get_rights_request,
    get_rights_request_with_sensitive, insert_rights_request, list_rights_requests,
    list_rights_requests_with_sensitive, resolve_rights_request_scope, transition_rights_request,
    withdraw_rights_request,
};
pub use rollout::{ensure_default_auth_rollout, load_auth_rollout, store_auth_rollout};
pub use safety_appeals::{
    RiskSignalCorrection, RiskSignalMetadataEdit, dispute_risk_signal,
    edit_risk_signal_detection_metadata, reissue_corrected_risk_signal,
    update_risk_signal_appeal_status, validate_optional_confidence, validate_optional_expires_at,
};
pub use safety_events::{
    DistributionAudience, PersistedRiskSignal, StoredModerationEvent, StoredRiskSignal,
    attribute_risk_signal_subject_author, expire_superseded_advisory_signals, get_risk_signal,
    get_signed_moderation_event, list_distributable_moderation_events,
    list_distributable_risk_signals, list_risk_signals, list_risk_signals_for_target,
    list_risk_signals_for_user, list_signed_moderation_events, persist_risk_signal,
    persist_risk_signal_deduplicated, persist_risk_signal_with_author,
    persist_signed_moderation_event,
};
pub use safety_runtime::{
    PgSafetyArtifactStore, resolve_safety_providers, resolve_safety_providers_with_pool,
};
pub use scan_verdicts::{
    StoredScanVerdict, get_scan_verdict, update_scan_verdict_advisories, upsert_scan_verdict,
};
pub use tester_feedback::{
    NewTesterFeedback, TesterFeedback, get_tester_feedback, insert_tester_feedback_with_retention,
    list_tester_feedback,
};
pub use transmission_preventions::{
    NewTransmissionPrevention, TransmissionPrevention, TransmissionPreventionBasis,
    TransmissionPreventionCapability, TransmissionPreventionMutation,
    apply_transmission_prevention, get_active_transmission_prevention, is_transmission_prevented,
    is_transmission_prevented_for_any, release_transmission_prevention,
};
pub use trust_inputs::{list_trust_risk_inputs, trust_risk_inputs_from};
pub use trust_observations::{
    ACTIVE_TRUST_OBSERVATION_RETENTION_DAYS, RELATION_OBSERVATIONS_PER_TARGET_LIMIT,
    REVOKED_TRUST_OBSERVATION_RETENTION_DAYS, StoreTrustObservationsOutcome,
    TRUST_OBSERVATION_MAX_CLOCK_SKEW_SECONDS, TrustObservationSharingStatus,
    cleanup_trust_observations, latest_successful_relation_snapshot_id,
    list_active_relation_observations, revoke_trust_observation_sharing, store_trust_observations,
    trust_observation_revisions, trust_observation_sharing_status,
};
