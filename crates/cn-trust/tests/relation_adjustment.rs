//! 閲覧者別 relation 値 R と合算値 S の contract テスト（ADR 0026 §8、Issue #1061）。DB 不要。

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use kukuri_cn_protocol::TrustEvaluationReason;
use kukuri_cn_trust::{
    RelationAdjustment, RelationObservation, RelationObservationKind, TrustParams, TrustTotals,
    apply_viewer_relation, build_trust_read, compose_relation_adjustment, compose_viewer_trust,
    relation_version,
};

const VIEWER_A: &str = "viewer-a";
const VIEWER_C: &str = "viewer-c";
const OBSERVER_U1: &str = "observer-u1";
const OBSERVER_U2: &str = "observer-u2";

#[allow(clippy::unwrap_used)] // test fixture helper
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-18T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn observation(observer: &str, kind: RelationObservationKind) -> RelationObservation {
    RelationObservation {
        observer_pubkey: observer.to_string(),
        kind,
        observed_at: now(),
    }
}

fn weights(entries: &[(&str, f64)]) -> BTreeMap<String, f64> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), *value))
        .collect()
}

/// 対象 B の集計（spam・severity Medium・confidence 100 が今の時刻に 1 件）。
fn spam_totals() -> TrustTotals {
    TrustTotals {
        relative_units: 400.0,
        relative_at: now(),
        half_life_days: 30.0,
        digest: 0x5ba3,
        ..TrustTotals::default()
    }
}

fn trust_view(totals: &TrustTotals) -> kukuri_cn_protocol::TrustReadView {
    build_trust_read("target-b", totals, &[], now(), &TrustParams::default())
}

#[test]
fn viewer_with_higher_relation_gets_larger_penalty() {
    // 同じ観測集合（U1・U2 が B をブロック）でも、U 群との relation が高い A の方が下げ幅が大きい。
    let params = TrustParams::default();
    let observations = [
        observation(OBSERVER_U1, RelationObservationKind::Block),
        observation(OBSERVER_U2, RelationObservationKind::Block),
    ];
    let for_a = compose_relation_adjustment(
        VIEWER_A,
        &observations,
        &weights(&[(OBSERVER_U1, 0.9), (OBSERVER_U2, 0.8)]),
        now(),
        &params,
    );
    let for_c = compose_relation_adjustment(
        VIEWER_C,
        &observations,
        &weights(&[(OBSERVER_U1, 0.2), (OBSERVER_U2, 0.15)]),
        now(),
        &params,
    );
    assert!(for_a.value < for_c.value, "{for_a:?} vs {for_c:?}");
    assert!(for_c.value < 0.0);
    assert!((-1.0..=0.0).contains(&for_a.value));
}

#[test]
fn single_observation_penalty_is_not_cancelled_by_weight() {
    // 観測が 1 件でも、重み付き平均のように w が相殺されず、relation の高さで下げ幅が変わる。
    let params = TrustParams::default();
    let observations = [observation(OBSERVER_U1, RelationObservationKind::Block)];
    let high = compose_relation_adjustment(
        VIEWER_A,
        &observations,
        &weights(&[(OBSERVER_U1, 0.9)]),
        now(),
        &params,
    );
    let low = compose_relation_adjustment(
        VIEWER_C,
        &observations,
        &weights(&[(OBSERVER_U1, 0.3)]),
        now(),
        &params,
    );
    assert!((high.value - -0.9).abs() < 1e-9, "{high:?}");
    assert!((low.value - -0.3).abs() < 1e-9, "{low:?}");
}

#[test]
fn many_low_weight_observers_do_not_dominate() {
    // 低 relation の observer が 100 人いても、上位 K 件の集約なので 1 人の高 relation observer を
    // 件数だけで上回らない。
    let params = TrustParams::default();
    let many: Vec<RelationObservation> = (0..100)
        .map(|index| {
            observation(
                format!("low-{index:03}").as_str(),
                RelationObservationKind::Block,
            )
        })
        .collect();
    let many_weights: BTreeMap<String, f64> = (0..100)
        .map(|index| (format!("low-{index:03}"), 0.12))
        .collect();
    let crowd = compose_relation_adjustment(VIEWER_A, &many, &many_weights, now(), &params);
    let close = compose_relation_adjustment(
        VIEWER_A,
        &[observation(OBSERVER_U1, RelationObservationKind::Block)],
        &weights(&[(OBSERVER_U1, 0.9)]),
        now(),
        &params,
    );
    assert!(crowd.value > close.value, "{crowd:?} vs {close:?}");

    // 重みが下限未満の observer は寄与しない。
    let below = compose_relation_adjustment(
        VIEWER_A,
        &many,
        &many
            .iter()
            .map(|item| (item.observer_pubkey.clone(), 0.05))
            .collect(),
        now(),
        &params,
    );
    assert_eq!(below, RelationAdjustment::NONE);
}

#[test]
fn unrelated_observers_and_missing_relation_do_not_penalize() {
    // relation が未観測の observer（重み 0）や負の proximity は減点しない。加点にも反転しない。
    let params = TrustParams::default();
    let observations = [
        observation(OBSERVER_U1, RelationObservationKind::Block),
        observation(OBSERVER_U2, RelationObservationKind::Mute),
    ];
    let adjustment = compose_relation_adjustment(
        VIEWER_A,
        &observations,
        &weights(&[(OBSERVER_U2, -0.8)]),
        now(),
        &params,
    );
    assert_eq!(adjustment, RelationAdjustment::NONE);
}

#[test]
fn block_and_mute_same_observer_takes_max() {
    let params = TrustParams::default();
    let proximity = weights(&[(OBSERVER_U1, 0.8)]);
    let block_only = compose_relation_adjustment(
        VIEWER_A,
        &[observation(OBSERVER_U1, RelationObservationKind::Block)],
        &proximity,
        now(),
        &params,
    );
    let mute_only = compose_relation_adjustment(
        VIEWER_A,
        &[observation(OBSERVER_U1, RelationObservationKind::Mute)],
        &proximity,
        now(),
        &params,
    );
    let both = compose_relation_adjustment(
        VIEWER_A,
        &[
            observation(OBSERVER_U1, RelationObservationKind::Mute),
            observation(OBSERVER_U1, RelationObservationKind::Block),
            // 再送・複数端末で同じ観測が重複しても増幅しない。
            observation(OBSERVER_U1, RelationObservationKind::Block),
        ],
        &proximity,
        now(),
        &params,
    );
    assert!(block_only.value < mute_only.value);
    assert_eq!(both, block_only);
}

#[test]
fn observations_decay_and_viewer_own_observation_has_full_weight() {
    let params = TrustParams::default();
    let mut aged = observation(OBSERVER_U1, RelationObservationKind::Block);
    aged.observed_at = now() - Duration::days(30);
    let proximity = weights(&[(OBSERVER_U1, 0.8)]);
    let fresh = compose_relation_adjustment(
        VIEWER_A,
        &[observation(OBSERVER_U1, RelationObservationKind::Block)],
        &proximity,
        now(),
        &params,
    );
    let decayed = compose_relation_adjustment(VIEWER_A, &[aged], &proximity, now(), &params);
    assert!((decayed.value - fresh.value / 2.0).abs() < 1e-9);

    // 閲覧者自身の観測は proximity によらず重み 1。
    let own = compose_relation_adjustment(
        VIEWER_A,
        &[observation(VIEWER_A, RelationObservationKind::Mute)],
        &BTreeMap::new(),
        now(),
        &params,
    );
    assert!((own.value - -params.mute_strength).abs() < 1e-9);
}

#[test]
fn revoked_and_expired_observations_are_excluded() {
    // 呼出側は active な観測だけを渡す。解除後に観測集合から外れれば R は元に戻り、他の根拠は残る。
    let params = TrustParams::default();
    let proximity = weights(&[(OBSERVER_U1, 0.9), (OBSERVER_U2, 0.5)]);
    let before = compose_relation_adjustment(
        VIEWER_A,
        &[
            observation(OBSERVER_U1, RelationObservationKind::Block),
            observation(OBSERVER_U2, RelationObservationKind::Block),
        ],
        &proximity,
        now(),
        &params,
    );
    let after_u1_revoked = compose_relation_adjustment(
        VIEWER_A,
        &[observation(OBSERVER_U2, RelationObservationKind::Block)],
        &proximity,
        now(),
        &params,
    );
    let after_all_revoked = compose_relation_adjustment(VIEWER_A, &[], &proximity, now(), &params);
    assert!(before.value < after_u1_revoked.value);
    assert!((after_u1_revoked.value - -0.5).abs() < 1e-9);
    assert_eq!(after_all_revoked, RelationAdjustment::NONE);
}

#[test]
fn adjustment_is_order_independent() {
    let params = TrustParams::default();
    let proximity = weights(&[(OBSERVER_U1, 0.9), (OBSERVER_U2, 0.4), ("u3", 0.6)]);
    let forward = [
        observation(OBSERVER_U1, RelationObservationKind::Block),
        observation(OBSERVER_U2, RelationObservationKind::Mute),
        observation("u3", RelationObservationKind::Block),
    ];
    let mut reversed = forward.clone();
    reversed.reverse();
    let a = compose_relation_adjustment(VIEWER_A, &forward, &proximity, now(), &params);
    let b = compose_relation_adjustment(VIEWER_A, &reversed, &proximity, now(), &params);
    assert_eq!(a, b);
    // 同じ入力の再評価で減点が累積しない。
    let again = compose_relation_adjustment(VIEWER_A, &forward, &proximity, now(), &params);
    assert_eq!(a, again);
}

#[test]
fn trust_component_is_unchanged_by_observations() {
    // T は閲覧者・観測に依存しない。A と C で R は異なるが T（内訳）は共通。
    let params = TrustParams::default();
    let totals = spam_totals();
    let t_view = trust_view(&totals);
    let observations = [observation(OBSERVER_U1, RelationObservationKind::Block)];
    let for_a = apply_viewer_relation(
        t_view.clone(),
        totals.version(),
        compose_relation_adjustment(
            VIEWER_A,
            &observations,
            &weights(&[(OBSERVER_U1, 0.9)]),
            now(),
            &params,
        ),
        relation_version(Some(7), &observations),
        now(),
        &params,
    );
    let for_c = apply_viewer_relation(
        t_view.clone(),
        totals.version(),
        compose_relation_adjustment(
            VIEWER_C,
            &observations,
            &weights(&[(OBSERVER_U1, 0.2)]),
            now(),
            &params,
        ),
        relation_version(Some(7), &observations),
        now(),
        &params,
    );
    for view in [&for_a, &for_c] {
        assert_eq!(view.absolute, t_view.absolute);
        assert_eq!(view.relative, t_view.relative);
        assert_eq!(view.w_abs_applied, t_view.w_abs_applied);
        assert_eq!(view.basis, t_view.basis);
        assert_eq!(
            view.evaluation.as_ref().unwrap().trust_version,
            totals.version()
        );
    }
    assert!(for_a.trust < for_c.trust);
    assert!((for_a.trust - compose_viewer_trust(t_view.trust, -0.9)).abs() < 1e-9);
}

#[test]
fn relation_update_does_not_touch_trust_component() {
    // R のみの更新（観測の追加）で T の版・内訳は変わらず、S だけが再合算される。
    // T のみの更新（signal の追加）で relation の版は変わらず、S は新しい T で再合算される。
    let params = TrustParams::default();
    let empty_totals = TrustTotals::default();
    let empty = trust_view(&empty_totals);
    let no_relation = apply_viewer_relation(
        empty.clone(),
        empty_totals.version(),
        RelationAdjustment::NONE,
        relation_version(Some(7), &[]),
        now(),
        &params,
    );
    let with_relation = apply_viewer_relation(
        empty.clone(),
        empty_totals.version(),
        compose_relation_adjustment(
            VIEWER_A,
            &[observation(OBSERVER_U1, RelationObservationKind::Block)],
            &weights(&[(OBSERVER_U1, 0.9)]),
            now(),
            &params,
        ),
        relation_version(
            Some(7),
            &[observation(OBSERVER_U1, RelationObservationKind::Block)],
        ),
        now(),
        &params,
    );
    let no_eval = no_relation.evaluation.as_ref().unwrap();
    let with_eval = with_relation.evaluation.as_ref().unwrap();
    assert_eq!(no_eval.trust_version, with_eval.trust_version);
    assert_ne!(no_eval.relation_version, with_eval.relation_version);
    assert_eq!(no_relation.trust, 0.0);
    assert!(with_relation.trust < 0.0);

    let with_signal_totals = spam_totals();
    let with_signal = trust_view(&with_signal_totals);
    let t_updated = apply_viewer_relation(
        with_signal.clone(),
        with_signal_totals.version(),
        RelationAdjustment::NONE,
        relation_version(Some(7), &[]),
        now(),
        &params,
    );
    let t_eval = t_updated.evaluation.as_ref().unwrap();
    assert_ne!(t_eval.trust_version, no_eval.trust_version);
    assert_eq!(t_eval.relation_version, no_eval.relation_version);
    assert_eq!(t_updated.trust, with_signal.trust);
}

#[test]
fn relation_version_digests_the_observations_used_for_the_evaluation() {
    // 観測が無ければ 0。版は解析の id と観測の集合だけで決まり、並びによらない（#1699）。
    assert_eq!(relation_version(Some(7), &[]), "r-7-0");
    assert_eq!(relation_version(None, &[]), "r-none-0");
    let u1 = observation(OBSERVER_U1, RelationObservationKind::Block);
    let u2 = observation(OBSERVER_U2, RelationObservationKind::Mute);
    let base = relation_version(Some(7), &[u1.clone(), u2.clone()]);
    assert_eq!(base, relation_version(Some(7), &[u2.clone(), u1.clone()]));
    assert_ne!(base, relation_version(Some(8), &[u1.clone(), u2.clone()]));
    // 観測の追加・評価からの外れ（解除・同意の無効化）・種別や観測時刻の更新で版が変わる。
    let changed = [
        vec![
            u1.clone(),
            u2.clone(),
            observation(VIEWER_C, RelationObservationKind::Block),
        ],
        vec![u1.clone()],
        vec![
            u1.clone(),
            RelationObservation {
                kind: RelationObservationKind::Block,
                ..u2.clone()
            },
        ],
        vec![
            u1.clone(),
            RelationObservation {
                observed_at: now() + Duration::seconds(1),
                ..u2.clone()
            },
        ],
    ];
    for observations in changed {
        assert_ne!(base, relation_version(Some(7), &observations));
    }
    // 元の集合に戻れば元の版に戻る。
    assert_eq!(base, relation_version(Some(7), &[u1, u2]));
}

#[test]
fn hide_recommendation_follows_node_local_threshold_and_reasons() {
    let params = TrustParams::default();
    let empty_totals = TrustTotals::default();
    let empty = trust_view(&empty_totals);
    // 根拠が無ければ非表示を推奨しない（欠落を悪質扱いしない）。
    let neutral = apply_viewer_relation(
        empty.clone(),
        empty_totals.version(),
        RelationAdjustment::NONE,
        relation_version(None, &[]),
        now(),
        &params,
    );
    let neutral_eval = neutral.evaluation.unwrap();
    assert!(!neutral_eval.hide_recommended);
    assert!(neutral_eval.reasons.is_empty());

    let adjusted = apply_viewer_relation(
        empty,
        empty_totals.version(),
        compose_relation_adjustment(
            VIEWER_A,
            &[observation(OBSERVER_U1, RelationObservationKind::Block)],
            &weights(&[(OBSERVER_U1, 0.9)]),
            now(),
            &params,
        ),
        relation_version(
            Some(1),
            &[observation(OBSERVER_U1, RelationObservationKind::Block)],
        ),
        now(),
        &params,
    );
    let eval = adjusted.evaluation.unwrap();
    assert!(eval.hide_recommended);
    assert_eq!(
        eval.reasons,
        vec![TrustEvaluationReason::RelatedUsersBlockOrMute]
    );
    assert_eq!(eval.computed_at, "2026-09-18T09:00:00Z");
    assert_eq!(eval.expires_at, "2026-09-18T09:10:00Z");
    assert_eq!(eval.policy_version, params.policy_version());

    // 閾値は operator 可変。
    let lenient = TrustParams {
        hide_threshold: -0.95,
        ..TrustParams::default()
    };
    let not_hidden = apply_viewer_relation(
        trust_view(&empty_totals),
        empty_totals.version(),
        compose_relation_adjustment(
            VIEWER_A,
            &[observation(OBSERVER_U1, RelationObservationKind::Block)],
            &weights(&[(OBSERVER_U1, 0.9)]),
            now(),
            &lenient,
        ),
        relation_version(
            Some(1),
            &[observation(OBSERVER_U1, RelationObservationKind::Block)],
        ),
        now(),
        &lenient,
    );
    assert!(!not_hidden.evaluation.unwrap().hide_recommended);
    assert_ne!(lenient.policy_version(), params.policy_version());
}

#[test]
fn viewer_trust_is_clamped_and_critical_absolute_is_not_offset() {
    // 合算値は [-1, 1] に収まり、R は非正なので T の減点を打ち消さない。
    assert_eq!(compose_viewer_trust(-1.0, -1.0), -1.0);
    assert_eq!(compose_viewer_trust(-0.6, 0.5), -0.6);
    assert_eq!(compose_viewer_trust(0.2, -0.1), 0.1);
}

#[test]
fn relation_params_are_operator_tunable_and_validated() {
    let tuned = TrustParams::from_lookup(|name| match name {
        "COMMUNITY_NODE_TRUST_RELATION_TOP_K" => Some("3".to_string()),
        "COMMUNITY_NODE_TRUST_MUTE_STRENGTH" => Some("0.25".to_string()),
        "COMMUNITY_NODE_TRUST_HIDE_THRESHOLD" => Some("-0.7".to_string()),
        "COMMUNITY_NODE_TRUST_EVALUATION_TTL_SECONDS" => Some("60".to_string()),
        _ => None,
    })
    .expect("valid params");
    assert_eq!(tuned.relation_top_k, 3);
    assert_eq!(tuned.mute_strength, 0.25);
    assert_eq!(tuned.hide_threshold, -0.7);
    assert_eq!(tuned.evaluation_ttl_seconds, 60);

    for (name, value) in [
        ("COMMUNITY_NODE_TRUST_RELATION_TOP_K", "0"),
        ("COMMUNITY_NODE_TRUST_RELATION_MIN_WEIGHT", "1.5"),
        ("COMMUNITY_NODE_TRUST_BLOCK_STRENGTH", "0"),
        ("COMMUNITY_NODE_TRUST_HIDE_THRESHOLD", "0.2"),
        ("COMMUNITY_NODE_TRUST_EVALUATION_TTL_SECONDS", "-1"),
    ] {
        assert!(
            TrustParams::from_lookup(|key| (key == name).then(|| value.to_string())).is_err(),
            "{name}={value} must be rejected"
        );
    }
}
