//! manifest wire 出力の golden テスト(WP-S3 T6)。
//!
//! build_manifest の JSON 出力(フィールド名・宣言順・値)は unauthenticated な
//! 公開 endpoint(/v1/node/manifest)の wire 契約そのもの。desktop-runtime 側の
//! round-trip テスト(manifest_support.rs、同一 fixture YAML)と対で drift を検出する。
//! fail した場合はテストでなく変更側の互換影響(client の tolerant reader が
//! default に落ちないか)を評価すること。後方互換な変更は optional フィールドの
//! 追加のみ(REFACTORING.md 凍結境界)。
use kukuri_cn_operator::{build_manifest, load_and_validate};

const FIXTURE_YAML: &str = r#"server:
  domain: golden.example
  operator_name: Golden Operator
  country: JP
  cloud_provider: ExampleCloud
  region: example-region-1
  contact: abuse@golden.example
  node_id: node-golden-1
  node_name: Golden Node

profile: relay-enabled

features:
  community_index: true
  moderation: true
  community_local_trust: true
  report_endpoint: true
  iroh_relay: true
  traffic_relay_fallback: true
  private_message_storage: false
  blob_cache: false
  analytics: false
  crash_report: false
  cloudflare_proxy: true
  dome_hosting: false

retention:
  connection_logs_days: 30
  moderation_logs_days: 180

safety:
  profile: public-node
  policy_version: 2026-06-public-node-v1
  indexing:
    index_before_scan: false
    on_scan_error: hold
  storage:
    permanent_blob_storage: false
  events:
    emit_signed_moderation_events: true
    signing_key_secret_id: kukuri-cn-safety-signing-key
  providers:
    known_csam:
      provider: project-arachnid-shield
      required: true
      credential_secret_id: kukuri-cn-safety-known-csam

manifest:
  manifest_version: v1
  node_role: default-onboarding-node

acknowledge_planned_capabilities: true
"#;

#[test]
fn manifest_wire_output_matches_golden() {
    let resolved = load_and_validate(FIXTURE_YAML).expect("fixture validates");
    let manifest = build_manifest(&resolved);
    let expected = r##"{"node_id":"node-golden-1","node_name":"Golden Node","node_role":"default-onboarding-node","server_name":"golden.example","operator_name":"Golden Operator","operator_country":"JP","cloud_provider":"ExampleCloud","region":"example-region-1","contact":"abuse@golden.example","abuse_contact":"abuse@golden.example","report_endpoint":"https://golden.example/v1/report","rights_request_url":"","rights_request_policy_url":"","rights_request_initial_response_target_days":7,"terms_url":"https://golden.example/terms","privacy_url":"https://golden.example/privacy","external_transmission_url":"https://golden.example/external-transmission","moderation_policy_url":"https://golden.example/moderation-policy","abuse_policy_url":"https://golden.example/abuse-policy","manifest_version":"v1","capabilities":{"auth_consent":true,"bootstrap_assist":true,"topic_rendezvous":true,"iroh_relay":true,"traffic_relay_fallback":true,"blob_cache":false,"private_message_storage":false,"analytics":false,"crash_report":false,"cloudflare_proxy":true,"push_notification":false,"community_index":true,"moderation":true,"community_local_trust":true,"report_endpoint":true,"rights_request_endpoint":false},"capability_scope":{"available_enabled":["auth_consent","bootstrap_assist","topic_rendezvous","iroh_relay","traffic_relay_fallback","cloudflare_proxy","community_index","moderation","community_local_trust","report_endpoint"],"planned_enabled":[]},"authority_scope":{"applies_to":["this_node","communities_indexed_by_this_node","moderation_events_issued_by_this_node","trust_signals_issued_by_this_node"],"does_not_apply_to":["kukuri_network_as_a_whole","third_party_nodes","user_identity","user_profile_canonical_source","user_social_graph_canonical_source"]},"p2p_boundary":{"identity_authority":false,"profile_canonical_store":false,"social_graph_canonical_store":false,"content_truth_source":false,"network_wide_authority":false},"features":{"community_index":true,"moderation":true,"trust_score":"community-local","iroh_relay":true,"iroh_relay_mode":"dedicated","traffic_relay_fallback":true,"private_message_storage":false,"blob_cache":false},"retention":{"connection_logs_days":30,"moderation_logs_days":180,"report_days":180,"report_contact_days":90,"rights_request_active_days":730,"rights_request_resolved_days":365,"rights_request_rejected_days":180,"operator_audit_days":365,"moderation_event_days":180,"risk_signal_days":180}}"##;
    let expected = expected
        .replace(
            "\"abuse_policy_url\":\"https://golden.example/abuse-policy\",",
            "\"abuse_policy_url\":\"https://golden.example/abuse-policy\",\"data_retention_url\":\"https://golden.example/data-retention\",",
        )
        .replace(
            "\"rights_request_endpoint\":false}",
            "\"rights_request_endpoint\":false,\"tester_feedback\":false,\"dome_hosting\":false,\"public_blob_search\":false}",
        );
    assert_eq!(
        serde_json::to_string(&manifest).expect("serialize"),
        expected
    );
}
