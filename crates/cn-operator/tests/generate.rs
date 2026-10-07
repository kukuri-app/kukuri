mod generate_support;
use generate_support::{config_with_safety_providers, doc};

use kukuri_cn_operator::{
    Capability, NodeRole, SAMPLE_CONFIG, build_manifest, check_drift, generate_all,
    generate_legal_documents, load_and_validate, manifest_value, parse_config,
    policy_snapshot_revision, resolve_and_validate,
};

fn base_config(extra_features: &str, ack: bool) -> String {
    format!(
        "server:\n\
         \x20 domain: example-kukuri.net\n\
         \x20 operator_name: Example Operator\n\
         \x20 country: JP\n\
         \x20 cloud_provider: AWS\n\
         \x20 region: ap-northeast-1\n\
         features:\n{extra_features}\
         retention:\n\
         \x20 connection_logs_days: 30\n\
         \x20 moderation_logs_days: 180\n\
         acknowledge_planned_capabilities: {ack}\n"
    )
}

#[test]
fn sample_config_is_valid() {
    let resolved = load_and_validate(SAMPLE_CONFIG).expect("sample config must validate");
    assert!(resolved.enabled(Capability::IrohRelay));
    assert!(
        resolved.enabled(Capability::AuthConsent),
        "auth_consent is baseline"
    );
}

#[test]
fn legal_catalog_requires_every_retention_value_to_be_explicit() {
    let yaml = SAMPLE_CONFIG.replace("  tester_feedback_days: 180\n", "");
    let error = load_and_validate(&yaml).expect_err("implicit legal retention must fail");
    assert!(
        error.to_string().contains("retention.tester_feedback_days"),
        "got: {error}"
    );
}

#[test]
fn policy_snapshot_is_deterministic_and_ignores_reference_translation_edits() {
    let baseline = load_and_validate(SAMPLE_CONFIG).unwrap();
    let baseline_revision = policy_snapshot_revision(&baseline).unwrap();
    assert_eq!(
        policy_snapshot_revision(&baseline).as_deref(),
        Some(baseline_revision.as_str())
    );

    let with_translation = SAMPLE_CONFIG.replace(
        "      required: true\n    - kind: privacy",
        "      required: true\n      translations:\n        - language: en\n          revision: 1\n          translation_of_version: 1\n          title: Terms\n          body_markdown: English reference text.\n    - kind: privacy",
    );
    let translated = load_and_validate(&with_translation).unwrap();
    assert_eq!(
        policy_snapshot_revision(&translated).as_deref(),
        Some(baseline_revision.as_str()),
        "参考訳は正文 snapshot を変更しない"
    );
    let documents = generate_legal_documents(&translated);
    let translation = documents
        .iter()
        .find(|document| document.reference_translation)
        .expect("reference translation");
    assert_eq!(translation.translation_revision, Some(1));
    assert_eq!(translation.translation_of_version, Some(1));

    let changed_retention =
        SAMPLE_CONFIG.replace("  connection_logs_days: 30", "  connection_logs_days: 31");
    let changed = load_and_validate(&changed_retention).unwrap();
    assert_ne!(
        policy_snapshot_revision(&changed).as_deref(),
        Some(baseline_revision.as_str())
    );

    for changed in [
        SAMPLE_CONFIG.replace("  cloud_provider: AWS", "  cloud_provider: ExampleCloud"),
        SAMPLE_CONFIG.replace("  region: ap-northeast-1", "  region: eu-west-1"),
    ] {
        let changed = load_and_validate(&changed).unwrap();
        assert_ne!(
            policy_snapshot_revision(&changed).as_deref(),
            Some(baseline_revision.as_str()),
            "法務文書に表示する hosting 情報は snapshot を変更する"
        );
    }
}

#[test]
fn operator_can_select_a_non_japanese_authoritative_language() {
    let yaml = SAMPLE_CONFIG.replace("language: ja", "language: en");
    let resolved = load_and_validate(&yaml).expect("authoritative language is operator-selected");
    let documents = generate_legal_documents(&resolved);
    assert!(documents.iter().all(|document| {
        document.reference_translation || document.authoritative_language == "en"
    }));
    assert!(
        documents
            .iter()
            .filter(|document| !document.reference_translation)
            .all(
                |document| document.content.contains("Authoritative language: en")
                    && document.content.contains("Capability-derived policy facts")
                    && !document.content.contains("言語: en")
            )
    );
}

#[test]
fn multiple_operator_matrix_preserves_the_same_policy_contract() {
    let configs = [
        SAMPLE_CONFIG.to_string(),
        SAMPLE_CONFIG
            .replace("example-kukuri.net", "community.example.org")
            .replace("Example Operator", "Independent Operator")
            .replace("country: JP", "country: IE")
            .replace("cloud_provider: AWS", "cloud_provider: ExampleCloud")
            .replace("region: ap-northeast-1", "region: eu-west-1")
            .replace("language: ja", "language: en")
            .replace("connection_logs_days: 30", "connection_logs_days: 14"),
    ];
    let mut snapshots = Vec::new();
    for yaml in configs {
        let resolved = load_and_validate(&yaml).expect("operator matrix config validates");
        let documents = generate_legal_documents(&resolved);
        assert_eq!(
            documents
                .iter()
                .filter(|document| !document.reference_translation)
                .count(),
            7
        );
        assert!(documents.iter().all(|document| {
            document.reference_translation
                || (!document.content.trim().is_empty()
                    && !document.policy_snapshot_revision.trim().is_empty())
        }));
        snapshots.push(policy_snapshot_revision(&resolved).unwrap());
    }
    assert_ne!(snapshots[0], snapshots[1]);
}

#[test]
fn every_capability_descriptor_has_purpose_and_rights_request_paths() {
    for capability in Capability::ALL {
        let descriptor = capability.policy_descriptor();
        assert!(!descriptor.purpose.label().is_empty(), "{capability}");
        assert!(
            !descriptor.rights_request_paths.is_empty(),
            "{capability} must disclose deletion/correction/suspension routing"
        );
    }
}

#[test]
fn profiles_are_defined() {
    for key in ["minimal", "relay-enabled", "full-service"] {
        let yaml = format!(
            "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
             profile: {key}\nacknowledge_planned_capabilities: true\n"
        );
        let resolved = load_and_validate(&yaml).expect("profile config validates");
        assert!(resolved.enabled(Capability::BootstrapAssist));
    }
}

#[test]
fn relay_enabled_profile_turns_on_relay() {
    let yaml = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
                profile: relay-enabled\nacknowledge_planned_capabilities: true\n";
    let resolved = load_and_validate(yaml).unwrap();
    assert!(resolved.enabled(Capability::IrohRelay));
    assert!(resolved.enabled(Capability::TrafficRelayFallback));
}

#[test]
fn explicit_feature_overrides_profile() {
    let yaml = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
                profile: relay-enabled\nfeatures:\n  iroh_relay: false\n\
                acknowledge_planned_capabilities: true\n";
    let resolved = load_and_validate(yaml).unwrap();
    assert!(!resolved.enabled(Capability::IrohRelay));
}

#[test]
fn promoted_capability_validates_without_ack() {
    // #617: index / moderation / local trust は提供中へ昇格済み。承認フラグ無しで有効化できる。
    let yaml = base_config("  moderation: true\n", false);
    let resolved = load_and_validate(&yaml).unwrap();
    assert!(resolved.enabled(Capability::Moderation));
    assert!(resolved.enabled_planned_capabilities().is_empty());
}

#[test]
fn acknowledge_flag_remains_accepted_for_compatibility() {
    // 既存 config の後方互換: 承認フラグが設定されていてもエラーにしない。
    let yaml = base_config("  moderation: true\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    assert!(resolved.enabled(Capability::Moderation));
    assert!(resolved.enabled_planned_capabilities().is_empty());
}

#[test]
fn unknown_feature_key_is_rejected() {
    let yaml = base_config("  not_a_real_feature: true\n", true);
    let err = load_and_validate(&yaml).unwrap_err();
    assert!(err.to_string().contains("未知のキー"), "got: {err}");
}

#[test]
fn missing_required_fields_fail() {
    let yaml = "server:\n  domain: \"\"\n  operator_name: Op\n  country: JP\n";
    assert!(load_and_validate(yaml).is_err());
}

#[test]
fn report_endpoint_emitted_and_available_when_enabled() {
    // #370: report endpoint は実装済み（Phase A）。有効化すると manifest に絶対 URL を出力し、
    // available_enabled（planned ではなく）に入る。
    let yaml = base_config("  report_endpoint: true\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    let manifest = build_manifest(&resolved);
    assert_eq!(
        manifest.report_endpoint,
        "https://example-kukuri.net/v1/report"
    );

    let m = manifest_value(&resolved);
    let available = m["capability_scope"]["available_enabled"]
        .as_array()
        .unwrap();
    assert!(
        available.iter().any(|v| v == "report_endpoint"),
        "report_endpoint should be available, not planned"
    );
    let planned = m["capability_scope"]["planned_enabled"].as_array().unwrap();
    assert!(planned.iter().all(|v| v != "report_endpoint"));
}

#[test]
fn report_endpoint_absent_when_capability_disabled() {
    // report_endpoint を有効化しない node では空文字を出力し、client は abuse_contact 案内に切替。
    let yaml = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n";
    let resolved = load_and_validate(yaml).unwrap();
    assert_eq!(build_manifest(&resolved).report_endpoint, "");
}

#[test]
fn rights_request_endpoint_publishes_dedicated_intake_and_scope_policy() {
    let yaml = format!(
        "{}\nmanifest:\n  rights_request_initial_response_target_days: 5\n",
        base_config("  rights_request_endpoint: true\n", true)
    );
    let resolved = load_and_validate(&yaml).unwrap();
    let manifest = build_manifest(&resolved);

    assert_eq!(
        manifest.rights_request_url,
        "https://example-kukuri.net/rights-requests/new"
    );
    assert_eq!(
        manifest.rights_request_policy_url,
        "https://example-kukuri.net/rights-infringement-policy"
    );
    assert_eq!(manifest.rights_request_initial_response_target_days, 5);
    assert!(manifest.capabilities.rights_request_endpoint);
    assert!(
        manifest
            .capability_scope
            .available_enabled
            .iter()
            .any(|capability| capability == "rights_request_endpoint")
    );
}

#[test]
fn rights_request_endpoint_is_opt_in() {
    let resolved =
        load_and_validate("server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n")
            .unwrap();
    let manifest = build_manifest(&resolved);
    assert_eq!(manifest.rights_request_url, "");
    assert_eq!(manifest.rights_request_policy_url, "");
    assert_eq!(manifest.rights_request_initial_response_target_days, 7);
}

#[test]
fn manifest_has_authority_scope_and_p2p_boundary() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let m = manifest_value(&resolved);

    // P2P boundary は identity / profile / social graph / network authority を false 宣言。
    let boundary = &m["p2p_boundary"];
    assert_eq!(boundary["identity_authority"], false);
    assert_eq!(boundary["profile_canonical_store"], false);
    assert_eq!(boundary["social_graph_canonical_store"], false);
    assert_eq!(boundary["content_truth_source"], false);
    assert_eq!(boundary["network_wide_authority"], false);

    // authority scope の does_not_apply_to に user identity 等が含まれる。
    let does_not = m["authority_scope"]["does_not_apply_to"]
        .as_array()
        .unwrap();
    assert!(does_not.iter().any(|v| v == "user_identity"));
    assert!(does_not.iter().any(|v| v == "kukuri_network_as_a_whole"));

    // capability_scope は available と planned を分離する。#617 の昇格後、moderation は
    // available 側に入り、planned は空になる。
    let scope = &m["capability_scope"];
    assert!(scope["available_enabled"].is_array());
    assert!(scope["planned_enabled"].is_array());
    let available = scope["available_enabled"].as_array().unwrap();
    assert!(available.iter().any(|v| v == "moderation"));
    assert!(scope["planned_enabled"].as_array().unwrap().is_empty());
}

#[test]
fn all_expected_docs_are_generated() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let files = generate_all(&resolved);
    let names: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();
    for expected in [
        "server-manifest.json",
        "network-diagram.md",
        "telecom-notification-draft.md",
        "service-description-draft.md",
        "terms.md",
        "privacy-policy.md",
        "external-transmission-notice.md",
        "abuse-policy.md",
        "moderation-policy.md",
        "data-retention-policy.md",
        "prior-consultation-email.md",
        "capability-risk-and-practices.md",
    ] {
        assert!(names.contains(&expected), "missing {expected}");
    }
}

#[test]
fn capability_risk_guide_covers_enabled_and_disabled() {
    // #359: enabled capability は実践ガイドとして、disabled capability は
    // 「引き受けていない責務」として記述される。個人運営を discourage しない。
    let yaml = base_config("  report_endpoint: true\n  analytics: false\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    let guide = doc(&generate_all(&resolved), "capability-risk-and-practices.md");

    // discourage しないトーンの明示。
    assert!(guide.contains("企業だけが担うものとは考えない"));
    // セクション構造。
    assert!(guide.contains("## 有効化している capability"));
    assert!(guide.contains("## 引き受けていない責務（無効な capability）"));
    // 有効化した report_endpoint の実践記述。
    assert!(guide.contains("通報エンドポイント"));
    assert!(guide.contains("authority scope:"));
    assert!(guide.contains("推奨対応:"));
    assert!(guide.contains("scope を狭める / 無効化:"));
    // 無効化した analytics は「引き受けていない責務」側に出る。
    let disabled_section = guide.split("引き受けていない責務").nth(1).unwrap();
    assert!(disabled_section.contains("アナリティクス"));
    // 法的免責が含まれる（header 経由）。
    assert!(guide.contains("法的助言ではありません"));
}

#[test]
fn generated_docs_contain_legal_disclaimer() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    for file in generate_all(&resolved) {
        if file.filename.ends_with(".md") {
            assert!(
                file.content.contains("法的助言ではありません"),
                "{} should contain legal disclaimer",
                file.filename
            );
        }
    }
}

#[test]
fn terms_limit_content_permissions_to_enabled_capabilities() {
    let yaml = base_config(
        "  community_index: true\n  moderation: true\n  blob_cache: true\n  private_message_storage: true\n",
        true,
    );
    let resolved = load_and_validate(&yaml).unwrap();
    let terms = doc(&generate_all(&resolved), "terms.md");

    for enabled_clause in [
        "索引・検索・発見・おすすめ",
        "安全性走査と走査に必要な一時取得",
        "添付 blob の一時 cache",
        "暗号化済み private message の一時保管",
    ] {
        assert!(
            terms.contains(enabled_clause),
            "terms must contain `{enabled_clause}`"
        );
    }
    for required_limit in [
        "本ノードの authority scope",
        "公開 topic に転用",
        "広告・宣伝・AI 学習",
        "既に受信 peer が取得した copy",
    ] {
        assert!(
            terms.contains(required_limit),
            "terms must contain `{required_limit}`"
        );
    }
}

#[test]
fn relay_only_terms_do_not_grant_index_scan_cache_or_private_storage_permissions() {
    let yaml = base_config(
        "  iroh_relay: true\n  traffic_relay_fallback: true\n  community_index: false\n  moderation: false\n  blob_cache: false\n  private_message_storage: false\n",
        true,
    );
    let resolved = load_and_validate(&yaml).unwrap();
    let terms = doc(&generate_all(&resolved), "terms.md");

    assert!(terms.contains("暗号化済み通信の経路上の一時的な伝送"));
    for disabled_clause in [
        "索引・検索・発見・おすすめのために",
        "安全性走査と走査に必要な一時取得のために",
        "添付 blob の一時 cache のために",
        "暗号化済み private message の一時保管のために",
    ] {
        assert!(
            !terms.contains(disabled_clause),
            "relay-only terms must not contain `{disabled_clause}`"
        );
    }
}

#[test]
fn terms_content_license_section_matches_golden() {
    let yaml = base_config(
        "  community_index: true\n  moderation: true\n  blob_cache: true\n  private_message_storage: true\n",
        true,
    );
    let resolved = load_and_validate(&yaml).unwrap();
    let terms = doc(&generate_all(&resolved), "terms.md");
    let section = terms
        .split("## 第3条（本ノードへの限定的な利用許諾）\n\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## 第4条（共有範囲の維持）").next())
        .expect("content-license section");
    let expected = concat!(
        "ユーザーは、本ノードの運営者に対し、ユーザーが選択した共有範囲と本ノードの authority scope の双方に含まれるコンテンツについて、次の有効な capability を提供するために必要な範囲に限り、非独占的かつ無償の利用を許諾します。著作権その他の権利はユーザーから移転しません。\n\n",
        "- サポート対象の公開 topic に含まれる投稿の索引・検索・発見・おすすめのために、本文とメタデータを取得、複製、保存、解析、表示すること。\n",
        "- 本ノードの authority scope 内の安全性走査と走査に必要な一時取得のために、本文、メタデータ、添付を取得、複製、解析すること。\n",
        "- 添付 blob の一時 cache のために、対象 blob を取得、複製、一時保存、配信すること。\n",
        "- 指定された受信者への暗号化済み private message の一時保管のために、暗号文を取得、複製、一時保存、配信すること。\n",
        "- 上記の許諾は、各 capability が有効であり、本ノードが対象コンテンツへ実際に関与する期間と範囲に限られます。無効な capability、関与していないコンテンツ、他 node、kukuri network 全体には及びません。\n",
    );
    assert_eq!(section, expected);
}

#[test]
fn relay_enabled_explains_encrypted_traffic_fallback() {
    let yaml = base_config("  iroh_relay: true\n  traffic_relay_fallback: true\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    let files = generate_all(&resolved);
    let telecom = doc(&files, "telecom-notification-draft.md");
    assert!(telecom.contains("暗号化済み"));
    let ext = doc(&files, "external-transmission-notice.md");
    assert!(ext.contains("relay"));
}

#[test]
fn analytics_disabled_omits_analytics_destination() {
    let yaml = base_config("  analytics: false\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    let ext = doc(&generate_all(&resolved), "external-transmission-notice.md");
    // 「現在の外部送信先」セクションにアナリティクスが運用中として出ないこと。
    let active_section = ext.split("送信していない").next().unwrap();
    assert!(!active_section.contains("### アナリティクスプロバイダ"));
    // 無効として明示はされる。
    assert!(ext.contains("アナリティクスプロバイダ: 該当機能が無効"));
}

#[test]
fn cloudflare_enabled_emits_external_transmission() {
    let yaml = base_config("  cloudflare_proxy: true\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    let ext = doc(&generate_all(&resolved), "external-transmission-notice.md");
    let active_section = ext.split("送信していない").next().unwrap();
    assert!(active_section.contains("Cloudflare"));
}

/// safety providers 付きの config（外部送信の動的開示の検証用）。
/// `vlm_hosting_line` は general provider 配下の行（例: `"      hosting: self_host\n"`）か空。
#[test]
fn safety_providers_surface_in_external_transmission_notice() {
    // 自前ホスト宣言: 視覚言語モデルは運営者管理基盤、Arachnid は第三者への外部送信。
    let yaml = config_with_safety_providers("      hosting: self_host\n");
    let resolved = load_and_validate(&yaml).unwrap();
    let ext = doc(&generate_all(&resolved), "external-transmission-notice.md");
    assert!(ext.contains("安全性走査プロバイダへの送信"));
    assert!(ext.contains("Project Arachnid Shield"));
    assert!(ext.contains("第三者への外部送信"));
    assert!(ext.contains("運営者が管理する視覚言語モデル基盤"));
    assert!(ext.contains("第三者への外部送信ではない"));

    // 外部 API 宣言: 視覚言語モデルも第三者への外部送信として表示される。
    let yaml = config_with_safety_providers("      hosting: external\n");
    let resolved = load_and_validate(&yaml).unwrap();
    let ext = doc(&generate_all(&resolved), "external-transmission-notice.md");
    assert!(ext.contains("外部の視覚言語モデル API"));
    assert!(!ext.contains("運営者が管理する視覚言語モデル基盤"));

    // 未指定は保守側（第三者への外部送信）として扱う。
    let yaml = config_with_safety_providers("");
    let resolved = load_and_validate(&yaml).unwrap();
    let ext = doc(&generate_all(&resolved), "external-transmission-notice.md");
    assert!(ext.contains("外部の視覚言語モデル API"));
}

#[test]
fn generated_docs_contain_no_planned_wording_after_promotion() {
    // #617 T6: 昇格後、全生成物から「計画中」表記が消えている（分離セクション含む）。
    let yaml = config_with_safety_providers("      hosting: self_host\n");
    let resolved = load_and_validate(&yaml).unwrap();
    for file in generate_all(&resolved) {
        assert!(
            !file.content.contains("計画中"),
            "{}: planned wording must not remain",
            file.filename
        );
    }
}

#[test]
fn network_diagram_shows_index_stack_data_flow() {
    // #617 T5: 索引系が有効な node の構成図に、実データフロー（3 ブロック）と境界説明が載る。
    let yaml = config_with_safety_providers("      hosting: self_host\n");
    let resolved = load_and_validate(&yaml).unwrap();
    let diagram = doc(&generate_all(&resolved), "network-diagram.md");
    for needle in [
        "構成要素とデータフロー",
        "利用者端末 / 他ピア",
        "Direct P2P",
        "cn-user-api",
        "cn-indexer",
        "Postgres",
        "Valkey",
        "ArcadeDB",
        "関係解析の定期実行",
        "iroh docs / blob ピア",
        "Project Arachnid Shield",
        "運営者が管理する視覚言語モデル基盤",
        "サポート対象（公開トピック）内に",
        "恒久保存しない",
    ] {
        assert!(diagram.contains(needle), "missing: {needle}");
    }

    // 索引系が無効な node には実データフロー節を出さない（過大表示の防止）。
    let yaml = base_config("", false);
    let resolved = load_and_validate(&yaml).unwrap();
    let diagram = doc(&generate_all(&resolved), "network-diagram.md");
    assert!(!diagram.contains("構成要素とデータフロー"));
}

#[test]
fn telecom_notification_carries_service_name_and_server() {
    // 届出様式への転記元: サービス名と使用サーバーの行を持つ。
    let yaml = config_with_safety_providers("      hosting: self_host\n");
    let resolved = load_and_validate(&yaml).unwrap();
    let telecom = doc(&generate_all(&resolved), "telecom-notification-draft.md");
    assert!(telecom.contains("提供するサービス: P2P コミュニケーションネットワークの補助サービス"));
    // fixture に cloud_provider が無い場合は行ごと出ない（誤記入の防止）。
    assert!(!telecom.contains("使用するサーバー:"));

    let with_cloud = yaml.replace(
        "  country: JP\n",
        "  country: JP\n  cloud_provider: Google Cloud\n",
    );
    let resolved = load_and_validate(&with_cloud).unwrap();
    let telecom = doc(&generate_all(&resolved), "telecom-notification-draft.md");
    assert!(telecom.contains("使用するサーバー: Google Cloud"));
    let diagram = doc(&generate_all(&resolved), "network-diagram.md");
    assert!(diagram.contains("使用するサーバー: Google Cloud"));
}

#[test]
fn data_retention_lists_storage_classes_for_index_stack() {
    // #617 T4: 索引・モデレーション・信頼の系統が有効な node では、データ区分と保存先・
    // 再構築/バックアップ区分が保持ポリシーへ載る。
    let yaml = config_with_safety_providers("      hosting: self_host\n");
    let resolved = load_and_validate(&yaml).unwrap();
    let retention = doc(&generate_all(&resolved), "data-retention-policy.md");
    assert!(retention.contains("データ区分と保存先"));
    for needle in [
        "Postgres",
        "ArcadeDB",
        "Valkey",
        "恒久保存しない",
        "再構築可能",
        "バックアップ対象は Postgres のみ",
        "canonical store ではない",
    ] {
        assert!(retention.contains(needle), "missing: {needle}");
    }

    // 系統が無効な node には索引系の保存先区分を書かない（誤開示防止）。
    let yaml = base_config("", false);
    let resolved = load_and_validate(&yaml).unwrap();
    let retention = doc(&generate_all(&resolved), "data-retention-policy.md");
    assert!(!retention.contains("データ区分と保存先"));
}

#[test]
fn generated_docs_never_contain_private_endpoints_or_secret_ids_values() {
    // 公開資料の非含有監査: URL・secret 値らしき文字列が生成物に出ないこと。
    // （secret は ID のみ config に書かれ、値はそもそも config に無い。ここでは
    //   接続先アドレスの類が漏れないことを固定する）
    let yaml = config_with_safety_providers("      hosting: self_host\n");
    let resolved = load_and_validate(&yaml).unwrap();
    for file in generate_all(&resolved) {
        for needle in ["http://10.", "http://192.168.", "wireguard", "WireGuard"] {
            assert!(
                !file.content.contains(needle),
                "{}: must not leak private endpoints: {needle}",
                file.filename
            );
        }
    }
}

#[test]
fn promoted_capability_metadata_describes_implemented_behavior() {
    // #617 T2: 昇格した 3 capability の説明が「（計画）」ではなく実装済みのデータフローを
    // 記述していることを固定する（開示文書の生成元となる契約）。
    for cap in [
        Capability::CommunityIndex,
        Capability::Moderation,
        Capability::CommunityLocalTrust,
    ] {
        let meta = cap.meta();
        for text in [
            meta.purpose,
            meta.telecom_note,
            meta.privacy_note,
            meta.terms_note,
        ] {
            assert!(
                !text.contains("計画"),
                "{cap}: metadata must not read as planned: {text}"
            );
        }
    }

    // index: 許可 content のみ・真実源と投影の分離・生メディア非保存。
    let index = Capability::CommunityIndex.meta();
    let index_descriptor = index.policy_descriptor();
    assert_eq!(index_descriptor.data_classes.len(), 1);
    assert!(index_descriptor.data_classes_text().contains("公開 topic"));
    assert!(index_descriptor.retention_text().contains("対象 topic"));
    assert!(index.purpose.contains("走査を通過した許可"));
    assert!(index.telecom_note.contains("真実源ではない"));

    // moderation: 既知一致 + 分類器・fail-closed・Match Data 非保存・authority 限定。
    let moderation = Capability::Moderation.meta();
    assert!(moderation.purpose.contains("Project Arachnid Shield"));
    assert!(moderation.purpose.contains("視覚言語モデル"));
    assert!(moderation.purpose.contains("fail-closed"));
    assert_eq!(moderation.policy_descriptor().safety_actions.len(), 4);
    assert!(moderation.telecom_note.contains("authority scope"));
    assert!(moderation.terms_note.contains("申し立て"));

    // trust / relation: 双方を含む・node-local advisory・公開の 2 者間のアクションのみ・opt-out 可逆。
    let trust = Capability::CommunityLocalTrust.meta();
    assert!(trust.display_name.contains("relation"));
    assert!(trust.purpose.contains("node-local advisory"));
    assert!(trust.privacy_note.contains("2 者間のアクション"));
    assert!(trust.privacy_note.contains("プライベートチャンネル"));
    assert!(trust.privacy_note.contains("可逆"));
    assert!(trust.telecom_note.contains("canonical"));
    assert!(trust.terms_note.contains("network-wide command"));
}

#[test]
fn promoted_capability_listed_as_operating() {
    // #617 の昇格後、moderation は運用中の補助機能として記載され、「計画中」分離は出ない。
    let yaml = base_config("  moderation: true\n", false);
    let resolved = load_and_validate(&yaml).unwrap();
    let svc = doc(&generate_all(&resolved), "service-description-draft.md");
    assert!(!svc.contains("計画中（この配布物では未提供）"));
    assert!(svc.contains("モデレーション"));
}

#[test]
fn output_is_deterministic() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let first = generate_all(&resolved);
    let second = generate_all(&resolved);
    assert_eq!(first, second);
}

#[test]
fn drift_check_detects_changes_and_clean() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let dir = tempfile::tempdir().unwrap();

    // 生成前は missing。
    let report = check_drift(&resolved, dir.path()).unwrap();
    assert!(!report.is_clean());
    assert!(!report.missing.is_empty());

    // 生成後は clean。
    for file in generate_all(&resolved) {
        std::fs::write(dir.path().join(&file.filename), &file.content).unwrap();
    }
    let report = check_drift(&resolved, dir.path()).unwrap();
    assert!(report.is_clean(), "{}", report.summary());

    // 改変すると changed 検出。
    std::fs::write(dir.path().join("terms.md"), "tampered").unwrap();
    let report = check_drift(&resolved, dir.path()).unwrap();
    assert!(report.changed.contains(&"terms.md".to_string()));
}

#[test]
fn disclosure_check_rejects_private_endpoint_leaks() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for file in generate_all(&resolved) {
        std::fs::write(dir.path().join(&file.filename), &file.content).unwrap();
    }
    let terms = dir.path().join("terms.md");
    let mut content = std::fs::read_to_string(&terms).unwrap();
    content.push_str("\ninternal endpoint: http://10.24.1.8:8000\n");
    std::fs::write(&terms, content).unwrap();

    let report = check_drift(&resolved, dir.path()).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.sensitive, vec!["terms.md".to_string()]);
}

#[test]
fn parse_then_resolve_roundtrip() {
    let cfg = parse_config(SAMPLE_CONFIG).unwrap();
    assert_eq!(cfg.server.country, "JP");
    let resolved = resolve_and_validate(cfg).unwrap();
    assert!(resolved.enabled(Capability::CloudflareProxy));
}

// --- #355: manifest authority scope / P2P boundary / node role ---

#[test]
fn typed_manifest_roundtrips_through_json() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let manifest = build_manifest(&resolved);
    let json = serde_json::to_string(&manifest).unwrap();
    let back: kukuri_cn_operator::CommunityNodeManifest = serde_json::from_str(&json).unwrap();
    // capabilities が型付きで往復できる。
    assert_eq!(
        back.capabilities.iroh_relay,
        manifest.capabilities.iroh_relay
    );
    assert_eq!(back.node_role, manifest.node_role);
}

#[test]
fn node_role_defaults_to_community_node() {
    let yaml = base_config("  iroh_relay: true\n  community_index: true\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    // 複数 capability を持つため community-node に推定される。
    assert_eq!(build_manifest(&resolved).node_role, NodeRole::CommunityNode);
}

#[test]
fn node_role_infers_relay_assist_for_relay_only() {
    let yaml = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
                features:\n  iroh_relay: true\n";
    let resolved = load_and_validate(yaml).unwrap();
    assert_eq!(build_manifest(&resolved).node_role, NodeRole::RelayAssist);
}

#[test]
fn explicit_node_role_is_respected() {
    let yaml = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
                manifest:\n  node_role: default-onboarding-node\n";
    let resolved = load_and_validate(yaml).unwrap();
    assert_eq!(
        build_manifest(&resolved).node_role,
        NodeRole::DefaultOnboardingNode
    );
}

#[test]
fn default_onboarding_node_distinguished_from_community_node() {
    let onboarding = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
                      manifest:\n  node_role: default-onboarding-node\n";
    let community = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
                     manifest:\n  node_role: community-node\n";
    let a = build_manifest(&load_and_validate(onboarding).unwrap()).node_role;
    let b = build_manifest(&load_and_validate(community).unwrap()).node_role;
    assert_ne!(a, b);
    assert_eq!(a, NodeRole::DefaultOnboardingNode);
}

#[test]
fn authority_scope_applies_to_derives_from_capabilities() {
    let yaml = base_config("  community_index: true\n", true);
    let resolved = load_and_validate(&yaml).unwrap();
    let m = build_manifest(&resolved);
    assert!(
        m.authority_scope
            .applies_to
            .contains(&"this_node".to_string())
    );
    assert!(
        m.authority_scope
            .applies_to
            .contains(&"communities_indexed_by_this_node".to_string())
    );
}

#[test]
fn operator_can_extend_applies_to() {
    let yaml = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n\
                manifest:\n  authority_scope:\n    additional_applies_to:\n      - custom_scope\n";
    let resolved = load_and_validate(yaml).unwrap();
    let m = build_manifest(&resolved);
    assert!(
        m.authority_scope
            .applies_to
            .contains(&"custom_scope".to_string())
    );
}

#[test]
fn does_not_apply_to_has_safe_default() {
    let yaml = "server:\n  domain: d.net\n  operator_name: Op\n  country: JP\n";
    let resolved = load_and_validate(yaml).unwrap();
    let m = build_manifest(&resolved);
    for expected in [
        "kukuri_network_as_a_whole",
        "user_identity",
        "user_profile_canonical_source",
        "user_social_graph_canonical_source",
        "third_party_nodes",
    ] {
        assert!(
            m.authority_scope
                .does_not_apply_to
                .contains(&expected.to_string()),
            "missing {expected}"
        );
    }
}

#[test]
fn p2p_boundary_is_all_false_invariant() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let b = build_manifest(&resolved).p2p_boundary;
    assert!(!b.identity_authority);
    assert!(!b.profile_canonical_store);
    assert!(!b.social_graph_canonical_store);
    assert!(!b.content_truth_source);
    assert!(!b.network_wide_authority);
}

#[test]
fn generated_docs_reflect_authority_scope() {
    let resolved = load_and_validate(SAMPLE_CONFIG).unwrap();
    let diagram = doc(&generate_all(&resolved), "network-diagram.md");
    assert!(diagram.contains("authority scope"));
    assert!(diagram.contains("does_not_apply_to"));
    assert!(diagram.contains("network-wide authority: false"));
}

#[test]
fn phase_a_legal_documents_publish_operator_and_versioned_identity() {
    let yaml = r#"server:
  domain: api.kukuri.app
  operator_name: KingYoSun
  country: JP
  contact: ops@kukuri.app
legal:
  identity_disclosure_request: "運営主体の氏名・住所が必要な場合は ops@kukuri.app へ請求してください。"
  documents:
    - { kind: terms, slug: terms_of_service, version: 1, effective_date: 2026-09-02, language: ja, required: true }
    - { kind: privacy, slug: privacy_policy, version: 1, effective_date: 2026-09-02, language: ja, required: true }
    - { kind: external_transmission, slug: external_transmission, version: 1, effective_date: 2026-09-02, language: ja }
    - { kind: moderation_policy, slug: moderation_policy, version: 1, effective_date: 2026-09-02, language: ja }
    - { kind: abuse_policy, slug: abuse_policy, version: 1, effective_date: 2026-09-02, language: ja }
    - { kind: data_retention, slug: data_retention, version: 1, effective_date: 2026-09-02, language: ja }
    - { kind: rights_infringement, slug: rights_infringement, version: 1, effective_date: 2026-09-02, language: ja }
manifest:
  manifest_version: v1
  rights_request_initial_response_target_days: 7
retention:
  connection_logs_days: 30
  moderation_logs_days: 180
  report_days: 180
  report_contact_days: 90
  tester_feedback_days: 180
  rights_request_active_days: 730
  rights_request_resolved_days: 365
  rights_request_rejected_days: 180
  rights_request_contact_days: 180
  rights_request_identity_days: 180
  rights_request_evidence_days: 180
  rights_request_history_days: 365
  operator_audit_days: 365
  moderation_event_days: 180
  risk_signal_days: 180
"#;
    let resolved = load_and_validate(yaml).unwrap();
    let files = generate_all(&resolved);
    let terms = doc(&files, "terms.md");
    let privacy = doc(&files, "privacy-policy.md");

    for (document, slug) in [
        (terms.as_str(), "terms_of_service"),
        (privacy.as_str(), "privacy_policy"),
    ] {
        assert!(document.contains("運営者: KingYoSun"));
        assert!(document.contains("連絡先: ops@kukuri.app"));
        assert!(document.contains(&format!("文書 slug: {slug}")));
        assert!(document.contains("文書版: 1"));
        assert!(document.contains("施行日: 2026-09-02"));
        assert!(document.contains("言語: ja"));
        assert!(document.contains("氏名・住所"));
        assert!(document.contains("ops@kukuri.app"));
    }
}

// #706: モデレーションを提供する公開ノードでは、公開ノード情報の node_id を
// リスク判定の issuer_node_id と一致させるために必須にする。
#[test]
fn moderation_public_node_requires_server_node_id() {
    let yaml = config_with_safety_providers("").replace(
        "  node_id: 79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\n",
        "",
    );
    let err = load_and_validate(&yaml).expect_err("node_id missing must fail");
    let message = err.to_string();
    assert!(message.contains("server.node_id"), "got: {message}");
    assert!(message.contains("issuer_node_id"), "got: {message}");

    let blank = config_with_safety_providers("").replace(
        "  node_id: 79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\n",
        "  node_id: \"  \"\n",
    );
    assert!(
        load_and_validate(&blank).is_err(),
        "blank node_id must fail"
    );
}

#[test]
fn server_node_id_stays_optional_without_safety_or_moderation() {
    // safety 節が無い設定(開発用・relay 専用など)は従来どおり node_id 無しで通る。
    let yaml = base_config("  moderation: true\n", false);
    assert!(
        load_and_validate(&yaml).is_ok(),
        "no safety section must stay valid"
    );
    // safety 節があってもモデレーションが無効なら node_id は不要。
    let yaml = config_with_safety_providers("").replace(
        "  node_id: 79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\n",
        "",
    );
    let yaml = yaml.replace("  moderation: true\n", "  moderation: false\n");
    assert!(
        load_and_validate(&yaml).is_ok(),
        "moderation disabled must stay valid"
    );
}
