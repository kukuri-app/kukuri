//! trust scoring の contract テスト（ADR 0026 §2.3 / §6.2 / §10, Issue #415 / #1702）。DB 不要。
//!
//! テスト名は ADR 0026 の必須 contract 名に対応する。成分は対象ごとの集計（`TrustTotals`）から求め、
//! basis は 1 ページ分の signal を説明する。集計が signal 1 行ごとの寄与（`signal_contribution`）と
//! 振り分け（`trust_component_for`）の和になることは、`cn-core` の結合試験（`trust_totals.rs`）が SQL の
//! 集計と突き合わせて固定する。

use chrono::{DateTime, Duration, Utc};

use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, Severity, Visibility,
};
use kukuri_cn_trust::{
    TrustComponentKind, TrustParams, TrustReadView, TrustRiskInput, TrustTotals, build_trust_read,
    compose_trust, signal_contribution, trust_component_for,
};

#[allow(clippy::unwrap_used)] // test fixture helper
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-02T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn input(
    id: &str,
    component: TrustComponentKind,
    category: SafetyCategory,
    severity: Severity,
    confidence: Option<u8>,
    persisted_at: DateTime<Utc>,
) -> TrustRiskInput {
    TrustRiskInput {
        signal_id: id.to_string(),
        issuer_node_id: "issuer-node".to_string(),
        target: RiskSignalTarget::UserPubkey,
        target_id: "pubkey-1".to_string(),
        component,
        category,
        severity,
        basis: Basis::KnownHashMatch,
        confidence,
        visibility: Visibility::Local,
        appeal_status: AppealStatus::None,
        expires_at: None,
        persisted_at,
        operator_adjusted_at: None,
    }
}

fn csam_input(id: &str, persisted_at: DateTime<Utc>) -> TrustRiskInput {
    input(
        id,
        TrustComponentKind::Absolute,
        SafetyCategory::Csam,
        Severity::Critical,
        Some(100),
        persisted_at,
    )
}

/// 相対成分の代表入力（spam。ADR 0026 §7 以降、nsfw / objectionable は寄与 0 なので相対成分の
/// 契約は spam / malware / phishing で固定する）。
fn spam_input(id: &str, persisted_at: DateTime<Utc>) -> TrustRiskInput {
    input(
        id,
        TrustComponentKind::Relative,
        SafetyCategory::Spam,
        Severity::High,
        Some(100),
        persisted_at,
    )
}

/// advisory-only（nsfw / objectionable）の入力。ラベル付き allow の signal は severity Low /
/// basis ClassifierScore。
fn advisory_input(
    id: &str,
    category: SafetyCategory,
    persisted_at: DateTime<Utc>,
) -> TrustRiskInput {
    let mut input = input(
        id,
        TrustComponentKind::Relative,
        category,
        Severity::Low,
        Some(84),
        persisted_at,
    );
    input.basis = Basis::ClassifierScore;
    input
}

/// 今の時刻を基準時刻にした集計（寄与は 1/1000 単位）。
fn totals(absolute_units: i64, relative_units: f64) -> TrustTotals {
    TrustTotals {
        absolute_units,
        relative_units,
        relative_at: now(),
        half_life_days: 30.0,
        ..TrustTotals::default()
    }
}

fn view(totals: &TrustTotals, page: &[TrustRiskInput]) -> TrustReadView {
    build_trust_read("pubkey-1", totals, page, now(), &TrustParams::default())
}

// --- 合成式（§6.2） ---

#[test]
fn trust_is_clamped_to_unit_interval() {
    let params = TrustParams::default();
    // absolute=-1, relative=-1 → 生値 (2*-1 + -1)/2 = -1.5 だが最終値は -1 で止まる。
    let composed = compose_trust(&params, -1.0, -1.0);
    assert_eq!(composed.trust, -1.0);
    // 成分に範囲外の値が渡っても ±1 に丸めてから合成する。
    let composed = compose_trust(&params, -5.0, 3.0);
    assert!((-1.0..=1.0).contains(&composed.trust));
    // 集計の和が ±1 を超えても成分は ±1 に収まる。
    let read = view(&totals(5_000, 9_000.0), &[]);
    assert_eq!(read.absolute, -1.0);
    assert_eq!(read.relative, -1.0);
    assert_eq!(read.trust, -1.0);
}

#[test]
fn trust_absolute_negative_is_weighted_double() {
    let params = TrustParams::default();
    // 絶対成分がマイナスのとき重み 2 倍（distrust 支配）。相対が最良でも救済されない。
    let negative = compose_trust(&params, -1.0, 1.0);
    assert_eq!(negative.w_abs_applied, 2.0);
    // §6.2 の式そのまま: (w_abs * absolute + relative) / 2 = (2.0 * (-1.0) + 1.0) / 2 = -0.5
    assert_eq!(negative.trust, -0.5);
    assert!(negative.trust < 0.0, "良好な相対指標で薄まってはいけない");
    // プラスのときは 1 倍。
    let positive = compose_trust(&params, 0.5, 0.0);
    assert_eq!(positive.w_abs_applied, 1.0);
    assert_eq!(positive.trust, 0.25);
}

#[test]
fn trust_composition_weights_are_operator_tunable() {
    // operator が w_abs を変えると合成結果が変わる。
    let default_params = TrustParams::default();
    let tuned = TrustParams {
        w_abs_negative: 4.0,
        ..TrustParams::default()
    };
    let a = compose_trust(&default_params, -0.4, 0.2);
    let b = compose_trust(&tuned, -0.4, 0.2);
    assert_ne!(a.trust, b.trust);
    assert_eq!(b.w_abs_applied, 4.0);

    // env 相当の lookup から可変（未設定キーは既定値、値は上書き）。
    let params = TrustParams::from_lookup(|name| {
        (name == "COMMUNITY_NODE_TRUST_W_ABS_NEGATIVE").then(|| "3.5".to_string())
    })
    .unwrap();
    assert_eq!(params.w_abs_negative, 3.5);
    assert_eq!(params.w_abs_positive, 1.0);
    assert_eq!(params.relative_half_life_days, 30.0);

    // 不正値は黙って既定値に倒さず Err（operator の設定ミスを検出する）。
    assert!(
        TrustParams::from_lookup(|name| {
            (name == "COMMUNITY_NODE_TRUST_W_ABS_NEGATIVE").then(|| "not-a-number".to_string())
        })
        .is_err()
    );
    assert!(
        TrustParams::from_lookup(|name| {
            (name == "COMMUNITY_NODE_TRUST_RELATIVE_HALF_LIFE_DAYS").then(|| "0".to_string())
        })
        .is_err()
    );
}

// --- 単一絶対スカラーにしない / 成分分離（§2.3） ---

#[test]
fn trust_is_not_single_absolute_scalar() {
    let read = view(
        &totals(1_000, 700.0),
        &[csam_input("sig-abs", now()), spam_input("sig-rel", now())],
    );
    // read は絶対 / 相対成分を分離して返し、合成値だけの単一スカラーにしない。
    assert_ne!(read.absolute, read.relative);
    assert!(read.absolute < 0.0);
    assert!(read.relative < 0.0);
    // 断定ラベルは存在しない（連続値 + 根拠のみ）。
    assert!(!read.basis.is_empty());
}

#[test]
fn trust_separates_absolute_and_relative_indicators() {
    // CSAM（critical）は絶対成分のみ、spam は相対成分のみに効く。
    let absolute_only = view(&totals(1_000, 0.0), &[csam_input("sig-abs", now())]);
    assert!(absolute_only.absolute < 0.0);
    assert_eq!(absolute_only.relative, 0.0);
    assert_eq!(
        absolute_only.basis[0].component,
        TrustComponentKind::Absolute
    );

    let relative_only = view(&totals(0, 700.0), &[spam_input("sig-rel", now())]);
    assert_eq!(relative_only.absolute, 0.0);
    assert!(relative_only.relative < 0.0);
    assert_eq!(
        relative_only.basis[0].component,
        TrustComponentKind::Relative
    );
}

#[test]
fn trust_reflects_risk_signals_split_by_category() {
    // category → 成分の振り分け規則（集計の trigger と同じ規則を scoring 側でも公開する）。
    for category in [
        SafetyCategory::Csam,
        SafetyCategory::Cse,
        SafetyCategory::Grooming,
    ] {
        assert_eq!(trust_component_for(category), TrustComponentKind::Absolute);
    }
    for category in [
        SafetyCategory::Spam,
        SafetyCategory::Malware,
        SafetyCategory::Phishing,
    ] {
        assert_eq!(trust_component_for(category), TrustComponentKind::Relative);
        assert!(!category.is_advisory_only());
    }
    // 3 分岐目（ADR 0026 §7.1）: nsfw / objectionable は wire 上 Relative のまま advisory-only。
    for category in [SafetyCategory::Nsfw, SafetyCategory::Objectionable] {
        assert_eq!(trust_component_for(category), TrustComponentKind::Relative);
        assert!(category.is_advisory_only());
    }
}

// --- decay（§6.2: 絶対は減衰しない / 相対は半減期） ---

#[test]
fn trust_absolute_component_does_not_decay() {
    let old = now() - Duration::days(365);
    let mut aged = totals(1_000, 0.0);
    aged.relative_at = old;
    // known-hash 由来の絶対成分は 1 年経っても同じ寄与（時間で薄めない）。
    let read = build_trust_read(
        "pk",
        &aged,
        &[csam_input("sig", old)],
        now(),
        &TrustParams::default(),
    );
    assert_eq!(read.absolute, view(&totals(1_000, 0.0), &[]).absolute);
    assert_eq!(read.basis[0].decay_factor, 1.0);
    assert_eq!(read.basis[0].contribution, read.basis[0].raw_contribution);
}

#[test]
fn trust_relative_component_decays_over_time() {
    // 1 半減期（30 日）前を基準時刻にした和は、今の時刻でちょうど半分の寄与になる。
    let fresh = view(&totals(0, 700.0), &[]);
    let mut month_old = totals(0, 700.0);
    month_old.relative_at = now() - Duration::days(30);
    let aged = view(&month_old, &[spam_input("sig", now() - Duration::days(30))]);
    let ratio = aged.relative / fresh.relative;
    assert!(
        (ratio - 0.5).abs() < 1e-9,
        "30 日（1 半減期）で寄与が半分になること: ratio={ratio}"
    );
    assert!((aged.basis[0].decay_factor - 0.5).abs() < 1e-9);
    // 半減期は operator 可変: 短い半減期で作った集計はより速く減衰する。
    let short = TrustTotals {
        half_life_days: 7.0,
        ..month_old.clone()
    };
    assert!(
        view(&short, &[]).relative > aged.relative,
        "短半減期はより浅い寄与（減衰が速い）"
    );
}

// --- appeal 反映（§6.2） ---

#[test]
fn trust_appeal_pending_holds_contribution() {
    // pending（Disputed）は寄与据え置き: 申し立て中に勝手に緩めない。
    let mut disputed = csam_input("sig-disputed", now());
    disputed.appeal_status = AppealStatus::Disputed;
    let read = view(&totals(1_000, 0.0), &[disputed]);
    assert_eq!(read.basis[0].appeal_status, AppealStatus::Disputed);
    assert!(read.basis[0].contribution < 0.0, "pending は寄与したまま");
}

#[test]
fn trust_appeal_accepted_excludes_contribution() {
    // accepted（Cleared）は説明用根拠に残すが、評価値へは寄与させない。
    let mut cleared = csam_input("sig-cleared", now());
    cleared.appeal_status = AppealStatus::Cleared;
    let read = view(&totals(0, 0.0), &[cleared]);
    assert_eq!(read.absolute, 0.0);
    assert_eq!(read.basis.len(), 1);
    assert_eq!(read.basis[0].appeal_status, AppealStatus::Cleared);
    assert_eq!(read.basis[0].contribution, 0.0);
}

// --- 根拠つき read（trust-semantics §4） ---

#[test]
fn trust_read_is_explainable_with_basis() {
    let read = view(
        &totals(1_000, 350.0),
        &[
            csam_input("sig-abs", now()),
            spam_input("sig-rel", now() - Duration::days(30)),
        ],
    );
    assert_eq!(read.target_id, "pubkey-1");
    assert_eq!(read.basis.len(), 2);

    let abs = read
        .basis
        .iter()
        .find(|e| e.signal_id == "sig-abs")
        .unwrap();
    // issuer / basis / confidence / visibility / expiry / appeal を説明できる。
    assert_eq!(abs.issuer_node_id, "issuer-node");
    assert_eq!(abs.component, TrustComponentKind::Absolute);
    assert_eq!(abs.category, SafetyCategory::Csam);
    assert_eq!(abs.basis, Basis::KnownHashMatch);
    assert_eq!(abs.confidence, Some(100));
    assert_eq!(abs.visibility, Visibility::Local);
    assert_eq!(abs.appeal_status, AppealStatus::None);
    assert_eq!(abs.decay_factor, 1.0);
    assert_eq!(abs.relation_weight, 1.0);

    let rel = read
        .basis
        .iter()
        .find(|e| e.signal_id == "sig-rel")
        .unwrap();
    // 実効寄与の内訳（decay）まで説明できる。閲覧者別の relation は T に入れない（§8 の R）。
    assert!((rel.decay_factor - 0.5).abs() < 1e-9);
    assert_eq!(rel.relation_weight, 1.0);
    assert!((rel.contribution - rel.raw_contribution * rel.decay_factor).abs() < 1e-12);
    assert_eq!(rel.operator_adjusted_at, None, "未訂正の判定は印を持たない");

    // 合成値と成分・適用重みが view から再構成できる（説明可能性）。
    let recomposed = compose_trust(&TrustParams::default(), read.absolute, read.relative);
    assert_eq!(read.trust, recomposed.trust);
    assert_eq!(read.w_abs_applied, recomposed.w_abs_applied);
}

/// #1058 AC-3: operator が値を確定した判定は、根拠一覧で確定時刻（RFC3339）として判別できる。
/// 寄与の計算は印の有無で変わらない。
#[test]
fn trust_read_basis_marks_operator_adjusted_signals() {
    let adjusted_at: DateTime<Utc> = "2026-09-16T01:02:03Z".parse().unwrap();
    let mut adjusted = spam_input("sig-adjusted", now());
    adjusted.operator_adjusted_at = Some(adjusted_at);
    let plain = spam_input("sig-plain", now());
    let read = view(&totals(0, 1_400.0), &[adjusted, plain]);
    let entry = |id: &str| read.basis.iter().find(|e| e.signal_id == id).unwrap();
    assert_eq!(
        entry("sig-adjusted").operator_adjusted_at.as_deref(),
        Some("2026-09-16T01:02:03Z")
    );
    assert_eq!(entry("sig-plain").operator_adjusted_at, None);
    assert_eq!(
        entry("sig-adjusted").contribution,
        entry("sig-plain").contribution
    );
}

// --- scenario: CSAM 系 risk は相対成分の量で揺れない ---

#[test]
fn csam_absolute_component_is_immune_to_relation_and_mass_signals() {
    let alone = view(&totals(1_000, 0.0), &[]);
    // 大量の相対シグナル（spam 50 件）を足しても、絶対成分は同じ。
    let noisy = view(&totals(1_000, 50.0 * 700.0), &[]);
    assert_eq!(alone.absolute, noisy.absolute);
    assert!(alone.absolute <= -1.0 + 1e-12);
    // 絶対成分マイナスなので合成でも distrust が支配的（相対が最悪でも -1 未満に落ちない）。
    assert!(noisy.trust <= -0.5);
    assert!(noisy.trust >= -1.0);
}

// --- ADR 0026 §7: advisory-only category（nsfw / objectionable）は寄与 0 ---

#[test]
fn general_advisory_contributes_zero_to_trust() {
    // nsfw / objectionable は basis に残るが raw_contribution / contribution とも 0。集計にも 0 として
    // 入る（`cn-core` の結合試験）ので relative / trust は動かない。
    for category in [SafetyCategory::Nsfw, SafetyCategory::Objectionable] {
        let advisory = advisory_input("sig-adv", category, now());
        assert_eq!(signal_contribution(&advisory), 0.0);
        let read = view(&totals(0, 0.0), &[advisory]);
        assert_eq!(read.relative, 0.0, "{category:?}");
        assert_eq!(read.trust, 0.0, "{category:?}");
        let entry = &read.basis[0];
        assert_eq!(entry.component, TrustComponentKind::Relative);
        assert_eq!(entry.category, category);
        assert_eq!(entry.severity, Severity::Low);
        assert_eq!(entry.basis, Basis::ClassifierScore);
        assert_eq!(entry.raw_contribution, 0.0);
        assert_eq!(entry.contribution, 0.0);
        assert_eq!(entry.appeal_status, AppealStatus::None);
    }

    // severity に依らず 0（#1050 の集約後も契約は同じ）。
    let mut high = advisory_input("sig-high", SafetyCategory::Nsfw, now());
    high.severity = Severity::High;
    assert_eq!(signal_contribution(&high), 0.0);

    // Cleared になっても値は動かず、basis の状態表示だけが変わる（ADR 0026 §7.3）。
    let mut cleared = advisory_input("sig-cleared", SafetyCategory::Objectionable, now());
    cleared.appeal_status = AppealStatus::Cleared;
    let read = view(&totals(0, 0.0), &[cleared]);
    assert_eq!(read.relative, 0.0);
    assert_eq!(read.basis[0].appeal_status, AppealStatus::Cleared);
    assert_eq!(read.basis[0].contribution, 0.0);
}

// --- §10: trust_version は集計の digest から決まる ---

#[test]
fn trust_version_follows_totals_digest() {
    assert_eq!(TrustTotals::default().version(), "t-0000000000000000");
    let one = TrustTotals {
        digest: 1,
        ..TrustTotals::default()
    };
    let negative = TrustTotals {
        digest: -1,
        ..TrustTotals::default()
    };
    assert_eq!(one.version(), "t-0000000000000001");
    assert_eq!(negative.version(), "t-ffffffffffffffff");
    // 時間減衰では変わらない（digest は signal の識別と状態だけから決まる）。
    let mut later = one.clone();
    later.relative_at = now() - Duration::days(10);
    assert_eq!(later.version(), one.version());
}
