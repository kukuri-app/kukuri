use kukuri_cn_protocol::{
    Proximity, ProximityBasisEntry, RELATION_OPTOUT_PATH, RelationOptoutResponse,
    RelationReadResponse, TRUST_EVALUATIONS_PATH, TRUST_OBSERVATION_SHARING_POLICY_SLUG,
    TRUST_OBSERVATIONS_PATH, TrustBasisEntry, TrustComponentKind, TrustEvaluation,
    TrustEvaluationReason, TrustEvaluationsResponse, TrustReadView, TrustUserReadResponse,
};
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, Severity, Visibility,
};

#[test]
fn trust_read_wire_contract_keeps_flattened_view_and_explainable_basis() {
    let response = TrustUserReadResponse {
        viewer_pubkey: "viewer".to_string(),
        view: TrustReadView {
            target_id: "target".to_string(),
            absolute: -0.4,
            relative: -0.2,
            trust: -0.3,
            w_abs_applied: 0.5,
            computed_at: "2026-08-13T00:00:00Z".to_string(),
            evaluation: None,
            basis_next_cursor: Some("1.1786000000000000.signal-1".to_string()),
            basis: vec![TrustBasisEntry {
                signal_id: "signal-1".to_string(),
                issuer_node_id: "node-1".to_string(),
                target: RiskSignalTarget::PostId,
                target_id: "post-1".to_string(),
                component: TrustComponentKind::Relative,
                category: SafetyCategory::Spam,
                severity: Severity::Medium,
                basis: Basis::ClassifierScore,
                confidence: Some(80),
                visibility: Visibility::Local,
                appeal_status: AppealStatus::None,
                expires_at: None,
                operator_adjusted_at: Some("2026-08-12T00:00:00Z".to_string()),
                raw_contribution: -0.4,
                decay_factor: 0.5,
                relation_weight: 1.0,
                contribution: -0.2,
            }],
        },
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["viewer_pubkey"], "viewer");
    assert_eq!(json["target_id"], "target");
    assert_eq!(json["basis"][0]["component"], "relative");
    assert_eq!(json["basis"][0]["category"], "spam");
    assert_eq!(json["basis"][0]["target"], "post_id");
    assert_eq!(json["basis"][0]["target_id"], "post-1");
    assert_eq!(
        json["basis"][0]["operator_adjusted_at"],
        "2026-08-12T00:00:00Z"
    );
    // #1702: basis は 1 ページで、続きの cursor を同じ階層に載せる。
    assert_eq!(json["basis_next_cursor"], "1.1786000000000000.signal-1");
    assert_eq!(
        serde_json::from_value::<TrustUserReadResponse>(json.clone()).unwrap(),
        response
    );

    // #1058: 印を持たない旧 node の応答も読める（未訂正として扱う）。
    let mut legacy = json;
    legacy["basis"][0]
        .as_object_mut()
        .unwrap()
        .remove("operator_adjusted_at");
    let legacy = serde_json::from_value::<TrustUserReadResponse>(legacy).unwrap();
    assert_eq!(legacy.view.basis[0].operator_adjusted_at, None);
}

#[test]
fn relation_wire_contract_keeps_flattened_proximity_and_distance_policy() {
    assert_eq!(RELATION_OPTOUT_PATH, "/v1/relation/optout");
    let relation = RelationReadResponse {
        viewer_pubkey: "viewer".to_string(),
        target_pubkey: "target".to_string(),
        proximity: Proximity {
            score: 0.75,
            basis: vec![ProximityBasisEntry {
                feature: "shared_topics".to_string(),
                value: 3.0,
                weight: 1.0,
                contribution: 0.75,
            }],
        },
    };
    let json = serde_json::to_value(&relation).unwrap();
    assert_eq!(json["score"], 0.75);
    assert!(json.get("proximity").is_none());

    let optout = RelationOptoutResponse {
        pubkey: "viewer".to_string(),
        opted_out: true,
        opted_out_at: Some("2026-08-13T00:00:00Z".to_string()),
        min_proximity: 0.25,
    };
    let optout_json = serde_json::to_value(optout).unwrap();
    assert_eq!(
        optout_json,
        serde_json::json!({
            "pubkey": "viewer",
            "opted_out": true,
            "opted_out_at": "2026-08-13T00:00:00Z",
            "min_proximity": 0.25
        })
    );
}

#[test]
fn trust_evaluation_wire_contract_is_optional_and_carries_no_observer() {
    // #1061: `trust` は CN が合算した S。`evaluation` は旧 node の応答では欠落する。
    assert_eq!(TRUST_EVALUATIONS_PATH, "/v1/trust/evaluations");
    assert_eq!(TRUST_OBSERVATIONS_PATH, "/v1/trust/observations");
    assert_eq!(
        TRUST_OBSERVATION_SHARING_POLICY_SLUG,
        "trust_observation_sharing"
    );
    let evaluation = TrustEvaluation {
        policy_version: "v1-policy".to_string(),
        trust_version: "t-1".to_string(),
        relation_version: "r-3-9".to_string(),
        computed_at: "2026-09-18T00:00:00Z".to_string(),
        expires_at: "2026-09-18T00:10:00Z".to_string(),
        hide_recommended: true,
        reasons: vec![
            TrustEvaluationReason::RiskSignals,
            TrustEvaluationReason::RelatedUsersBlockOrMute,
        ],
    };
    let view = TrustReadView {
        target_id: "target".to_string(),
        absolute: 0.0,
        relative: -0.2,
        trust: -0.9,
        w_abs_applied: 1.0,
        computed_at: "2026-09-18T00:00:00Z".to_string(),
        basis: Vec::new(),
        evaluation: Some(evaluation.clone()),
        basis_next_cursor: None,
    };
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["evaluation"]["hide_recommended"], true);
    assert_eq!(
        json["evaluation"]["reasons"],
        serde_json::json!(["risk_signals", "related_users_block_or_mute"])
    );
    let keys: Vec<&str> = json["evaluation"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "computed_at",
            "expires_at",
            "hide_recommended",
            "policy_version",
            "reasons",
            "relation_version",
            "trust_version",
        ]
    );

    // 最後のページでは cursor の欄を出さない。
    assert!(json.get("basis_next_cursor").is_none());

    // 評価の無い旧応答は evaluation = None として読め、書き出し時も欄を出さない。
    let mut legacy = json.clone();
    legacy.as_object_mut().unwrap().remove("evaluation");
    let legacy_view = serde_json::from_value::<TrustReadView>(legacy).unwrap();
    assert_eq!(legacy_view.evaluation, None);
    assert!(
        serde_json::to_value(&legacy_view)
            .unwrap()
            .get("evaluation")
            .is_none()
    );

    let batch: TrustEvaluationsResponse = serde_json::from_value(serde_json::json!({
        "viewer_pubkey": "viewer",
        "evaluations": [{
            "target_pubkey": "target",
            "trust": -0.9,
            "evaluation": serde_json::to_value(&evaluation).unwrap(),
        }],
    }))
    .unwrap();
    assert_eq!(batch.evaluations[0].evaluation, evaluation);
}
