//! operator config から運営者向け文書群を決定論的に生成する。
//!
//! 出力は wall-clock 非依存（version は config 由来）であり、同じ config からは同じ出力が得られる。
//!
//! Phase A / Phase B の分離:
//! - 運用中の開示（外部送信表示・データ取扱い）は Available かつ有効な capability のみに基づく。
//! - Planned（計画中・未提供）capability は、各文書で明示的に「計画中」として分離して記述し、
//!   運用中であるかのような開示には含めない。

use std::fmt::Write as _;

use crate::capability::{Availability, Capability, ExternalDestination};
use crate::config::{LegalDocumentKind, ResolvedConfig};
use crate::manifest::{build_manifest, render_manifest};

/// すべての生成文書に付す共通の注記。
const LEGAL_DISCLAIMER: &str = "> 注記: この文書は operator config から自動生成された下書きであり、法的助言ではありません。\n\
> 最終的な内容・適法性の判断は、運営者自身および必要に応じて総合通信局・弁護士等の専門家への確認が必要です。";

const MANIFEST_FILE: &str = "server-manifest.json";

/// 生成された 1 ファイル。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    pub filename: String,
    pub content: String,
}

/// 有効かつ Available な capability。
fn available_enabled(config: &ResolvedConfig) -> Vec<Capability> {
    config
        .enabled_capabilities()
        .into_iter()
        .filter(|c| c.availability() == Availability::Available)
        .collect()
}

/// 有効かつ Planned な capability。
fn planned_enabled(config: &ResolvedConfig) -> Vec<Capability> {
    config.enabled_planned_capabilities()
}

/// 有効な Available capability に基づく外部送信先（重複排除、Capability::ALL 順）。
fn external_destinations(config: &ResolvedConfig) -> Vec<ExternalDestination> {
    let mut dests = vec![ExternalDestination::CommunityServer];
    for cap in available_enabled(config) {
        for destination in cap.policy_descriptor().external_destinations {
            if !dests.contains(destination) {
                dests.push(*destination);
            }
        }
    }
    dests
}

/// 索引・モデレーション・信頼のいずれかが有効か（実データフロー開示の出し分けに使う）。
fn index_stack_active(config: &ResolvedConfig) -> bool {
    config.enabled(Capability::CommunityIndex)
        || config.enabled(Capability::Moderation)
        || config.enabled(Capability::CommunityLocalTrust)
}

/// 安全性走査プロバイダへの送信先（operator config 由来の動的開示。#617）。
///
/// 真実源はプロバイダ構成そのもの（構成されていれば走査時に送信が発生する）。
/// 公開資料には送信先名・区分・目的・データ区分のみを出し、接続先 URL・内部
/// アドレス・資格情報は出さない。
struct SafetyProviderDestination {
    display_name: &'static str,
    /// 真 = 運営者が管理する基盤（第三者への外部送信ではない）。
    operator_controlled: bool,
    purpose: &'static str,
    data_categories: &'static str,
}

fn safety_provider_destinations(config: &ResolvedConfig) -> Vec<SafetyProviderDestination> {
    let Some(safety) = config.raw.safety.as_ref() else {
        return Vec::new();
    };
    let providers = &safety.providers;
    let normalized = |entry: &crate::SafetyProviderEntry| entry.provider.trim().replace('_', "-");
    let mut dests = Vec::new();

    if let Some(entry) = providers.known_csam.as_ref()
        && normalized(entry) == "project-arachnid-shield"
    {
        dests.push(SafetyProviderDestination {
            display_name:
                "Project Arachnid Shield（Canadian Centre for Child Protection が運営する照合 API）",
            operator_controlled: false,
            purpose: "既知 CSAM（児童性的虐待コンテンツ）との照合による検知走査",
            data_categories: "走査対象メディアのバイト列またはそのハッシュ。認証は運営者自身の\
                資格情報で行う。照合結果の詳細（Match Data）は保存・配布しない",
        });
    }

    // 視覚言語モデルは general / unknown_csam のどちらの slot でも同一の送信先として
    // 1 件に集約する。区分は hosting 宣言に従い、未指定は保守側（第三者への外部送信）。
    let vlm_entry = [providers.general.as_ref(), providers.unknown_csam.as_ref()]
        .into_iter()
        .flatten()
        .find(|entry| normalized(entry) == "openai-compatible-vlm");
    if let Some(entry) = vlm_entry {
        let self_host = matches!(entry.hosting, Some(crate::ProviderHosting::SelfHost));
        dests.push(SafetyProviderDestination {
            display_name: if self_host {
                "運営者が管理する視覚言語モデル基盤（OpenAI 互換 API）"
            } else {
                "外部の視覚言語モデル API（OpenAI 互換）"
            },
            operator_controlled: self_host,
            purpose: "投稿本文・メディアのモデレーション目的の分類走査",
            data_categories: "走査対象の本文テキストおよびメディアのバイト列。モデルの生応答は\
                保存・配布しない",
        });
    }
    if providers
        .general
        .as_ref()
        .is_some_and(|entry| normalized(entry) == "openai-moderation")
    {
        dests.push(SafetyProviderDestination {
            display_name: "OpenAI Moderation API",
            operator_controlled: false,
            purpose: "投稿本文・静止画像・動画の抽出画像のモデレーション分類",
            data_categories: "本文テキスト、正規化した静止画像、動画から採取したJPEGフレーム。動画本体・音声はOpenAIへ送信しない。CNは元メディア・抽出画像・API生応答を恒久保存せず、判定と最小coverageを内容hashと構成に結び付けて再利用する。動画は全フレーム検査ではない",
        });
    }
    dests
}

/// 文書ヘッダ（タイトル + 運営者情報 + 注記）。
pub(crate) fn header(
    config: &ResolvedConfig,
    title: &str,
    kind: Option<LegalDocumentKind>,
) -> String {
    let s = &config.raw.server;
    let mut header = format!(
        "# {title}\n\n\
         - 運営者: {operator}\n\
         - サーバー: {domain}\n\
         - 所在国: {country}\n\
         - 連絡先: {contact}\n\
         - manifest version: {version}\n\n\
         {disclaimer}\n",
        title = title,
        operator = s.operator_name,
        domain = s.domain,
        country = s.country,
        contact = config.contact(),
        version = config.raw.manifest.manifest_version,
        disclaimer = LEGAL_DISCLAIMER,
    );
    if let Some(document) = kind.and_then(|kind| config.legal_document(kind)) {
        let _ = writeln!(header, "\n## 文書情報\n");
        let _ = writeln!(header, "- 文書 slug: {}", document.slug);
        let _ = writeln!(header, "- 文書版: {}", document.version);
        let _ = writeln!(header, "- 施行日: {}", document.effective_date);
        let _ = writeln!(header, "- 言語: {}", document.language);
        if let Some(legal) = config.raw.legal.as_ref() {
            let _ = writeln!(
                header,
                "- 氏名・住所の請求方法: {}",
                legal.identity_disclosure_request
            );
        }
    }
    header
}

/// 計画中 capability があれば、それを明示する共通セクション。
pub(crate) fn planned_section(config: &ResolvedConfig) -> String {
    let planned = planned_enabled(config);
    if planned.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    let _ = writeln!(s, "\n## 計画中（この配布物では未提供）の capability\n");
    let _ = writeln!(
        s,
        "以下の capability は config 上で宣言されていますが、現行の community node 実装では提供されていません。"
    );
    let _ = writeln!(
        s,
        "そのため、本文書では「運用中の機能」としては扱わず、将来提供する予定の設計（spec）として記載します。\n"
    );
    for cap in planned {
        let m = cap.meta();
        let _ = writeln!(
            s,
            "- **{}**（{}）: {}",
            m.display_name,
            Availability::Planned.label_ja(),
            m.policy_descriptor().purpose.label()
        );
    }
    s
}

// ---------------------------------------------------------------------------
// 各文書ジェネレータ
// ---------------------------------------------------------------------------

fn gen_network_diagram(config: &ResolvedConfig) -> String {
    let mut s = header(config, "ネットワーク構成説明", None);
    let _ = writeln!(s, "\n## 通信経路の基本優先度\n");
    let _ = writeln!(
        s,
        "kukuri の基本通信優先度は `Direct P2P -> Relay Supported P2P -> Relay Fallback` です。\
         community node はこの経路を補助する層であり、ユーザーの所属先（home server）ではありません。\n"
    );
    let _ = writeln!(s, "## 有効な接続補助 capability\n");
    let _ = writeln!(s, "```text");
    let _ = writeln!(s, "client");
    let _ = writeln!(s, "  |");
    for cap in available_enabled(config) {
        let _ = writeln!(s, "  +-- {} ({})", cap.meta().display_name, cap.key());
    }
    let _ = writeln!(s, "```\n");

    if config.enabled(Capability::IrohRelay) || config.enabled(Capability::TrafficRelayFallback) {
        let _ = writeln!(s, "## relay に関する補足\n");
        let _ = writeln!(
            s,
            "iroh relay / traffic relay fallback が有効です。これらは単なる signaling ではなく、\
             Direct / Relay Supported P2P が成立しない場合に、暗号化済みであっても実 traffic が relay を\
             経由し得ます。届出要否は構成と所在地に依存するため、別途確認してください。\n"
        );
    }

    // 索引・モデレーション・信頼の系統が有効な node の実データフロー（#617）。
    if index_stack_active(config) {
        if let Some(cloud) = config.raw.server.cloud_provider.as_deref() {
            let _ = writeln!(s, "使用するサーバー: {cloud}\n");
        }
        let _ = writeln!(s, "## 構成要素とデータフロー\n");
        let _ = writeln!(s, "```text");
        let _ = writeln!(s, "利用者端末 / 他ピア");
        let _ = writeln!(
            s,
            "  ├─ Direct P2P … 端末間の直接通信（本ノードを経由しない）"
        );
        let _ = writeln!(
            s,
            "  ├─ cn-user-api（HTTPS。リバースプロキシ経由）… 認証・同意・検索/発見/おすすめ・\
             信頼/関係の読み取り・通報"
        );
        let _ = writeln!(
            s,
            "  └─ cn-iroh-relay（HTTP/QUIC）… 接続補助と、直接通信が成立しない場合の\
             暗号化済み traffic の中継"
        );
        let _ = writeln!(s);
        let _ = writeln!(s, "ノード内部（private network。外部へ公開しない）");
        let _ = writeln!(
            s,
            "  ├─ cn-indexer … 公開トピックのレプリカ同期（iroh-docs）・安全性走査・索引書き込み\
             の常駐ワーカー"
        );
        let _ = writeln!(
            s,
            "  ├─ Postgres … 管理系データ・走査判定・署名付き event / risk signal・索引の真実源（永続）"
        );
        let _ = writeln!(
            s,
            "  ├─ Valkey … ランデブー / presence（TTL 付きの揮発データ）"
        );
        let _ = writeln!(
            s,
            "  ├─ ArcadeDB … 検索投影・relation graph（真実源から再構築可能な派生データ）"
        );
        let _ = writeln!(
            s,
            "  └─ 関係解析の定期実行 … 公開トピックでの 2 者間のアクションから relation graph を差分で更新"
        );
        let _ = writeln!(s);
        let _ = writeln!(s, "外部 / 運営者基盤（ノードからの outbound のみ）");
        let safety_dests = safety_provider_destinations(config);
        let _ = writeln!(
            s,
            "  {} iroh docs / blob ピア … レプリカ同期と、走査用メディアの一時取得（恒久保存しない）",
            if safety_dests.is_empty() {
                "└─"
            } else {
                "├─"
            }
        );
        for (index, dest) in safety_dests.iter().enumerate() {
            let branch = if index + 1 == safety_dests.len() {
                "└─"
            } else {
                "├─"
            };
            let _ = writeln!(
                s,
                "  {branch} {} … {}",
                dest.display_name,
                if dest.operator_controlled {
                    "運営者が管理する基盤（第三者への外部送信ではない）"
                } else {
                    "第三者への外部送信（outbound HTTPS）"
                }
            );
        }
        let _ = writeln!(s, "```");
        let _ = writeln!(s);
        let _ = writeln!(s, "境界の要点:\n");
        let _ = writeln!(
            s,
            "- 公開するのは利用者向け API（HTTPS）と relay（HTTP/QUIC）のみ。データベース類は\
             外部へ公開しない。"
        );
        let _ = writeln!(
            s,
            "- 索引・モデレーション・信頼・関係の権限は、本ノードのサポート対象（公開トピック）内に\
             限定される。"
        );
        let _ = writeln!(
            s,
            "- 保存区分: 永続（Postgres）/ 揮発（Valkey、TTL）/ 再構築可能（ArcadeDB）/ \
             一時（走査用メディア。恒久保存しない）。\n"
        );
    }

    // manifest の authority scope / P2P boundary を文書へ反映する。
    let manifest = build_manifest(config);
    let _ = writeln!(s, "## node role と責任境界 (authority scope)\n");
    let _ = writeln!(
        s,
        "- node role: `{}`",
        serde_json::to_value(manifest.node_role)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "community-node".to_string())
    );
    let _ = writeln!(s, "\n本ノードが責任を負う範囲 (applies_to):\n");
    for item in &manifest.authority_scope.applies_to {
        let _ = writeln!(s, "- `{item}`");
    }
    let _ = writeln!(s, "\n本ノードが責任を負わない範囲 (does_not_apply_to):\n");
    for item in &manifest.authority_scope.does_not_apply_to {
        let _ = writeln!(s, "- `{item}`");
    }
    let _ = writeln!(s, "\n## P2P boundary\n");
    let _ = writeln!(
        s,
        "本ノードは以下のいずれの権威も持ちません（kukuri の P2P-first 設計の不変条件）。\n"
    );
    let _ = writeln!(s, "- identity authority: false");
    let _ = writeln!(s, "- profile canonical store: false");
    let _ = writeln!(s, "- social graph canonical store: false");
    let _ = writeln!(s, "- content truth source: false");
    let _ = writeln!(s, "- network-wide authority: false\n");
    let _ = writeln!(
        s,
        "詳細は `server-manifest.json` の `authority_scope` / `p2p_boundary` を参照してください。\n"
    );

    s.push_str(&planned_section(config));
    s
}

fn gen_telecom_notification(config: &ResolvedConfig) -> String {
    let mut s = header(
        config,
        "電気通信事業 届出補助資料（役務説明ドラフト）",
        None,
    );
    let _ = writeln!(s, "\n## 前提\n");
    let _ = writeln!(
        s,
        "この資料は、クラウド / VPS 利用・回線非設置を前提とした説明ドラフトです。\
         自宅サーバー構成や回線設置を伴う構成は advanced であり、個別確認が必要です。\n"
    );
    let _ = writeln!(s, "## 役務の概要\n");
    let _ = writeln!(
        s,
        "提供するサービス: P2P コミュニケーションネットワークの補助サービス"
    );
    if let Some(cloud) = config.raw.server.cloud_provider.as_deref() {
        let _ = writeln!(s, "使用するサーバー: {cloud}");
    }
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "本サービスは、P2P network の補助層として動作する community node です。\
         ユーザーの identity / profile / social graph を所有せず、以下の補助機能を提供します。\n"
    );
    for cap in available_enabled(config) {
        let m = cap.meta();
        let _ = writeln!(s, "- {}: {}", m.display_name, m.telecom_note);
    }
    let _ = writeln!(s, "\n## relay に関する注意\n");
    if config.enabled(Capability::IrohRelay) || config.enabled(Capability::TrafficRelayFallback) {
        let _ = writeln!(
            s,
            "iroh relay / traffic relay fallback が有効なため、暗号化済み traffic の中継が発生し得ます。\
             これを signaling only と混同せず、役務区分・届出要否を総合通信局・専門家に事前確認してください。"
        );
    } else {
        let _ = writeln!(
            s,
            "relay 系 capability は無効です。実 traffic の中継は前提としていません。\
             ただし届出要否は最終的に運営者自身で確認してください。"
        );
    }
    let _ = writeln!(s, "\n## 構成情報\n");
    let _ = writeln!(
        s,
        "- クラウド / インフラ: {}",
        config
            .raw
            .server
            .cloud_provider
            .clone()
            .unwrap_or_else(|| "未指定".to_string())
    );
    let _ = writeln!(
        s,
        "- リージョン: {}",
        config
            .raw
            .server
            .region
            .clone()
            .unwrap_or_else(|| "未指定".to_string())
    );
    s.push_str(&planned_section(config));
    s
}

fn gen_service_description(config: &ResolvedConfig) -> String {
    let mut s = header(config, "サービス説明ドラフト", None);
    let _ = writeln!(s, "\n## 提供する補助機能（運用中）\n");
    for cap in available_enabled(config) {
        let m = cap.meta();
        let descriptor = m.policy_descriptor();
        let _ = writeln!(s, "### {}\n", m.display_name);
        let _ = writeln!(s, "- 目的: {}", descriptor.purpose.label());
        let _ = writeln!(s, "- 取扱いデータ: {}", descriptor.data_classes_text());
        let _ = writeln!(s, "- 保持への影響: {}", descriptor.retention_text());
        let _ = writeln!(
            s,
            "- 削除・訂正・停止等の請求経路: {}\n",
            descriptor.rights_request_paths_text()
        );
    }
    s.push_str(&planned_section(config));
    s
}

/// operator config から、投稿内容を直接扱う提供中 capability だけを抽出する。
///
/// ここでは技術的な有効／無効だけを決め、規約文言は `render_node_content_license` に閉じる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TermsContentScope {
    community_index: bool,
    moderation: bool,
    blob_cache: bool,
    private_message_storage: bool,
    encrypted_transit: bool,
}

fn terms_content_scope(config: &ResolvedConfig) -> TermsContentScope {
    let enabled = available_enabled(config);
    let has = |capability| enabled.contains(&capability);
    TermsContentScope {
        community_index: has(Capability::CommunityIndex),
        moderation: has(Capability::Moderation),
        blob_cache: has(Capability::BlobCache),
        private_message_storage: has(Capability::PrivateMessageStorage),
        encrypted_transit: has(Capability::IrohRelay) || has(Capability::TrafficRelayFallback),
    }
}

/// 技術的な capability scope を、当該 node に限る法的な許諾文言へ変換する。
fn render_node_content_license(scope: TermsContentScope) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "ユーザーは、本ノードの運営者に対し、ユーザーが選択した共有範囲と本ノードの authority scope の双方に含まれるコンテンツについて、次の有効な capability を提供するために必要な範囲に限り、非独占的かつ無償の利用を許諾します。著作権その他の権利はユーザーから移転しません。\n"
    );
    if scope.community_index {
        let _ = writeln!(
            s,
            "- サポート対象の公開 topic に含まれる投稿の索引・検索・発見・おすすめのために、本文とメタデータを取得、複製、保存、解析、表示すること。"
        );
    }
    if scope.moderation {
        let _ = writeln!(
            s,
            "- 本ノードの authority scope 内の安全性走査と走査に必要な一時取得のために、本文、メタデータ、添付を取得、複製、解析すること。"
        );
    }
    if scope.blob_cache {
        let _ = writeln!(
            s,
            "- 添付 blob の一時 cache のために、対象 blob を取得、複製、一時保存、配信すること。"
        );
    }
    if scope.private_message_storage {
        let _ = writeln!(
            s,
            "- 指定された受信者への暗号化済み private message の一時保管のために、暗号文を取得、複製、一時保存、配信すること。"
        );
    }
    if scope.encrypted_transit {
        let _ = writeln!(
            s,
            "- relay capability による暗号化済み通信の経路上の一時的な伝送を行うこと。この許諾は投稿内容の表示、索引、解析、保存または二次利用を含みません。"
        );
    }
    let _ = writeln!(
        s,
        "- 上記の許諾は、各 capability が有効であり、本ノードが対象コンテンツへ実際に関与する期間と範囲に限られます。無効な capability、関与していないコンテンツ、他 node、kukuri network 全体には及びません。\n"
    );
    s
}

fn gen_terms(config: &ResolvedConfig) -> String {
    let mut s = header(config, "利用規約", Some(LegalDocumentKind::Terms));
    let _ = writeln!(s, "\n## 適用範囲\n");
    let _ = writeln!(
        s,
        "本規約はこの community node の利用にだけ適用され、kukuri クライアント本体、他の community node、Direct P2P の利用条件を定めるものではありません。\n"
    );
    let _ = writeln!(s, "\n## 第1条（本ノードの位置づけ）\n");
    let _ = writeln!(
        s,
        "本 community node は P2P network の補助層であり、ユーザーの identity / profile / social graph を\
         所有しません。本ノードの停止・変更によってもこれらは失われません。\n"
    );
    let _ = writeln!(s, "## 第2条（投稿コンテンツの権利と権利保有の表明）\n");
    let _ = writeln!(
        s,
        "投稿コンテンツの著作権その他の権利は、原則としてユーザーまたは正当な権利者に帰属します。本ノードへ権利が譲渡されることはありません。\n"
    );
    let _ = writeln!(
        s,
        "ユーザーは、投稿する著作物、肖像、氏名、音源、映像、3D モデル、添付その他のコンテンツについて、投稿・共有と本規約に定める処理に必要な権利を有するか、正当な権利者から許諾を得ていることを表明します。\n"
    );
    let _ = writeln!(s, "## 第3条（本ノードへの限定的な利用許諾）\n");
    s.push_str(&render_node_content_license(terms_content_scope(config)));
    let _ = writeln!(s, "## 第4条（共有範囲の維持）\n");
    let _ = writeln!(
        s,
        "公開 topic に含まれる投稿は、本ノードがサポートする当該公開 topic の範囲でのみ扱います。private channel と DM は、指定された受信者への提供に必要な処理だけを対象とし、公開 topic に転用したり、不特定多数へ表示したりする許諾を含みません。\n"
    );
    let _ = writeln!(s, "## 第5条（撤回・送信防止後の取扱い）\n");
    let _ = writeln!(
        s,
        "有効な投稿撤回または本ノードの送信防止を認識した場合、本ノードは適用対象 capability での将来の新規索引、検索、発見、推薦、走査用取得、cache または配信を停止します。法令上必要な場合は、対象・目的・期間を限定して記録を保持することがあります。\n"
    );
    let _ = writeln!(
        s,
        "P2P の性質上、既に受信 peer が取得した copy、他 node、Direct P2P の経路から、投稿や添付を完全に回収または消去することは保証できません。\n"
    );
    let _ = writeln!(s, "## 第6条（許諾に含まれない利用）\n");
    let _ = writeln!(
        s,
        "本規約の限定的な利用許諾には、投稿コンテンツを広告・宣伝・AI 学習・機械学習モデルの訓練その他の独立した二次目的へ利用する権利は含まれません。default node、kukuri project または本ノードが、network 全体の恒久的・包括的な権利主体になることもありません。\n"
    );
    let _ = writeln!(s, "## 第7条（禁止事項）\n");
    let _ = writeln!(s, "- 法令に違反する目的での利用");
    let _ = writeln!(s, "- 他者の権利を侵害する行為");
    let _ = writeln!(s, "- 本ノードの補助機能の妨害\n");
    let _ = writeln!(s, "## 第8条（免責）\n");
    let _ = writeln!(
        s,
        "運営者は、本ノードが関与した補助機能の範囲でのみ責任を負い、kukuri network 全体・他ノードの\
         活動については責任を負いません。\n"
    );
    let _ = writeln!(s, "## 第9条（その他の capability 別の取扱い）\n");
    for cap in available_enabled(config) {
        let m = cap.meta();
        let descriptor = m.policy_descriptor();
        let _ = writeln!(
            s,
            "- {}: {}",
            m.display_name,
            descriptor.policy_summary_text()
        );
    }
    s.push_str(&planned_section(config));
    s
}

fn gen_privacy(config: &ResolvedConfig) -> String {
    let mut s = header(
        config,
        "プライバシーポリシー",
        Some(LegalDocumentKind::Privacy),
    );
    let _ = writeln!(s, "\n## 適用範囲\n");
    let _ = writeln!(
        s,
        "本ポリシーはこの community node が取り扱うデータにだけ適用されます。kukuri クライアントのローカルデータ、他 node、Direct P2P で peer 間に流れるデータは、この node が実際に受信・保存する場合を除き対象外です。\n"
    );
    let _ = writeln!(s, "\n## 取得・取扱いするデータ（運用中の capability）\n");
    for cap in available_enabled(config) {
        let m = cap.meta();
        let _ = writeln!(s, "### {}\n", m.display_name);
        let _ = writeln!(
            s,
            "- 取扱いデータ: {}",
            m.policy_descriptor().data_classes_text()
        );
        let _ = writeln!(
            s,
            "- 取扱いの説明: {}\n",
            m.policy_descriptor().policy_summary_text()
        );
    }
    let _ = writeln!(s, "## 接続ログ・保持期間\n");
    let _ = writeln!(
        s,
        "- 接続ログ保持期間: {} 日",
        config.raw.retention.connection_logs_days
    );
    let _ = writeln!(
        s,
        "- モデレーションログ保持期間: {} 日\n",
        config.raw.retention.moderation_logs_days
    );
    let _ = writeln!(s, "## 案件データの保持と保全\n");
    for (label, days) in retention_rows(config) {
        let _ = writeln!(s, "- {label}: {days} 日");
    }
    let _ = writeln!(
        s,
        "\n期限切れデータは通常の読取から除外し、案件・区分を限定した legal hold が無ければ削除します。\
         hold は通常の表示期限を延長しません。連絡先、本人・代理権確認情報、証拠参照は専用鍵で暗号化します。\n"
    );
    let _ = writeln!(s, "## 外部送信\n");
    let _ = writeln!(
        s,
        "外部送信の詳細は `external-transmission-notice.md` を参照してください。\n"
    );
    s.push_str(&planned_section(config));
    s
}

fn gen_external_transmission(config: &ResolvedConfig) -> String {
    let mut s = header(
        config,
        "外部送信表示",
        Some(LegalDocumentKind::ExternalTransmission),
    );
    let _ = writeln!(s, "\n## 現在の外部送信先（有効な機能に基づく）\n");
    let _ = writeln!(
        s,
        "以下は、現在有効な機能の構成に基づいて発生し得る外部送信先です。\n"
    );
    for dest in external_destinations(config) {
        let _ = writeln!(s, "### {}\n", dest.display_name());
        let _ = writeln!(s, "{}\n", dest.description());
    }

    // 安全性走査プロバイダへの送信（構成されている場合のみ。#617）。
    let safety_dests = safety_provider_destinations(config);
    if !safety_dests.is_empty() {
        let _ = writeln!(s, "## 安全性走査プロバイダへの送信\n");
        let _ = writeln!(
            s,
            "モデレーション（安全性走査）のため、構成済みプロバイダへ次の送信が発生します。\
             接続先の具体的なアドレスは運用上の理由で公開しません。\n"
        );
        for dest in safety_dests {
            let _ = writeln!(s, "### {}\n", dest.display_name);
            let _ = writeln!(
                s,
                "- 区分: {}",
                if dest.operator_controlled {
                    "運営者が管理する基盤内の送信（第三者への外部送信ではない）"
                } else {
                    "第三者への外部送信"
                }
            );
            let _ = writeln!(s, "- 目的: {}", dest.purpose);
            let _ = writeln!(s, "- 送信するデータ: {}\n", dest.data_categories);
        }
    }

    // 無効化により送信されないものを明示（analytics: false 等の検証可能性）。
    let mut not_sent: Vec<ExternalDestination> = Vec::new();
    let active = external_destinations(config);
    for dest in [
        ExternalDestination::Cloudflare,
        ExternalDestination::ObjectStorage,
        ExternalDestination::PushProvider,
        ExternalDestination::AnalyticsProvider,
        ExternalDestination::CrashReportProvider,
        ExternalDestination::DedicatedIrohRelay,
        ExternalDestination::PublicRelay,
    ] {
        if !active.contains(&dest) {
            not_sent.push(dest);
        }
    }
    if !not_sent.is_empty() {
        let _ = writeln!(s, "## 送信していない外部送信先（無効な機能）\n");
        for dest in not_sent {
            let _ = writeln!(
                s,
                "- {}: 該当機能が無効のため送信しません。",
                dest.display_name()
            );
        }
        s.push('\n');
    }
    s.push_str(&planned_section(config));
    s
}

fn gen_abuse_policy(config: &ResolvedConfig) -> String {
    let mut s = header(
        config,
        "Abuse ポリシー",
        Some(LegalDocumentKind::AbusePolicy),
    );
    let _ = writeln!(s, "\n## 連絡先\n");
    let _ = writeln!(s, "- abuse 連絡先: {}\n", config.contact());
    let _ = writeln!(s, "## 対応範囲\n");
    let _ = writeln!(
        s,
        "本ノードは、本ノードが実際に関与した補助機能（index / moderation / cache / relay 等のうち有効なもの）の\
         範囲でのみ abuse 対応を行います。kukuri network 全体の中央通報窓口ではありません。\n"
    );
    if config.enabled(Capability::ReportEndpoint) {
        let _ = writeln!(
            s,
            "通報エンドポイント `POST {}` を提供します。通報は本ノードが関与した対象に限定され、\
             reporter の identity / social graph は保持しません。上記連絡先も引き続き窓口とします。\n",
            config.report_endpoint()
        );
        let _ = writeln!(
            s,
            "通報本体は {} 日、任意連絡先は {} 日保持し、期限切れ後は通常読取から除外します。\n",
            config.raw.retention.report_days, config.raw.retention.report_contact_days
        );
    }
    s.push_str(&planned_section(config));
    s
}

fn gen_rights_infringement_policy(config: &ResolvedConfig) -> String {
    let mut s = header(
        config,
        "権利侵害申出ポリシー",
        Some(LegalDocumentKind::RightsInfringement),
    );
    let _ = writeln!(s, "\n## 申出前に確認する対応範囲\n");
    let _ = writeln!(
        s,
        "本ノードが実行できるのは、本ノード自身が提供する索引・検索・発見・推薦・moderation・\
         blob cache のうち有効な機能に対する node-local な措置だけです。対象と本ノードの関与を\
         確認できない場合は追加情報をお願いし、authority 外の申出は範囲外として回答します。\n"
    );
    let _ = writeln!(s, "## 本ノードでは実行できないこと\n");
    for item in [
        "他の Community Node の索引・cache の削除",
        "第三者端末または source peer のデータ削除",
        "author-owned replica にある投稿正本の削除",
        "Direct P2P の遮断",
        "暗号化 relay packet の内容検査または遮断",
        "既に取得されたデータの回収",
    ] {
        let _ = writeln!(s, "- {item}");
    }
    let _ = writeln!(
        s,
        "\n申出の受付は権利侵害の認定や希望する措置を保証するものではありません。申出画面では、\
         この範囲を版付きで表示し、明示的な同意がなければ送信できません。\n"
    );
    let _ = writeln!(s, "## 受付情報と証拠\n");
    let _ = writeln!(
        s,
        "申出人区分、氏名・連絡先、代理権、権利根拠、対象、侵害態様、許諾していない旨、\
         希望する node-local 措置を受け取ります。証拠は URL・hash・外部識別子だけを受け取り、\
         ファイル upload や対象コンテンツの複製は行いません。申出情報は local-only とし、\
         公開 status に申出人情報・operator・内部メモを表示しません。\n"
    );
    let _ = writeln!(s, "## 応答と追跡\n");
    let _ = writeln!(
        s,
        "初回応答は {} 日以内を運用目標とします。これは法定期限ではなく、回答時期や措置を保証しません。\
         受付時に発行される参照 ID と一度だけ表示される追跡 secret で、公開可能な状態を確認・取下げできます。\n",
        config
            .raw
            .manifest
            .rights_request_initial_response_target_days
    );
    if config.enabled(Capability::RightsRequestEndpoint) {
        let _ = writeln!(s, "- 申出画面: {}", config.rights_request_url());
    } else {
        let _ = writeln!(
            s,
            "本ノードでは現在、専用の権利侵害申出受付を有効にしていません。"
        );
    }
    let _ = writeln!(
        s,
        "\n申出本体は未解決 {} 日、措置済み {} 日、却下・範囲外・取下げ {} 日を既定とします。\
         連絡先・本人確認・証拠参照は本体から分離して暗号化し、各区分の期限で削除します。\
         有効な法的手続に基づく案件限定 legal hold 中は対象区分の物理削除だけを停止します。",
        config.raw.retention.rights_request_active_days,
        config.raw.retention.rights_request_resolved_days,
        config.raw.retention.rights_request_rejected_days
    );
    s.push_str(&planned_section(config));
    s
}

fn gen_data_retention(config: &ResolvedConfig) -> String {
    let mut s = header(
        config,
        "データ保持ポリシー",
        Some(LegalDocumentKind::DataRetention),
    );
    let _ = writeln!(s, "\n## 保持期間\n");
    let _ = writeln!(
        s,
        "- 接続ログ: {} 日",
        config.raw.retention.connection_logs_days
    );
    let _ = writeln!(
        s,
        "- モデレーションログ: {} 日\n",
        config.raw.retention.moderation_logs_days
    );
    for (label, days) in retention_rows(config) {
        let _ = writeln!(s, "- {label}: {days} 日");
    }
    let _ = writeln!(
        s,
        "\n期限切れは通常読取時にも除外し、起動時・定期 cleanup で物理削除します。\
         backup から復元した場合も API 公開前に同じ cleanup を適用します。案件・データ区分を限定した\
         active legal hold は物理削除だけを停止し、期限切れデータを通常画面へ再表示しません。\n"
    );

    // データ区分と保存先（#617。索引・モデレーション・信頼・関係の各系統が有効な場合）。
    if index_stack_active(config) {
        let _ = writeln!(s, "## データ区分と保存先\n");
        let _ = writeln!(s, "| データ | 保存先 | 性質 |");
        let _ = writeln!(s, "|---|---|---|");
        let _ = writeln!(
            s,
            "| 認証・同意・通報 | Postgres | 管理系の永続データ（同意は撤回まで、通報は保持方針に従う） |"
        );
        let _ = writeln!(
            s,
            "| 走査判定（scan verdict） | Postgres | fail-closed な索引真実源が参照する判定記録 |"
        );
        let _ = writeln!(
            s,
            "| 署名付き moderation event / risk signal | Postgres | 保持期間・失効・訂正再発行・申し立ての対象 |"
        );
        let _ = writeln!(
            s,
            "| 索引の真実源（index truth） | Postgres | 許可判定のみを持つ authoritative な索引記録 |"
        );
        let _ = writeln!(
            s,
            "| 検索投影 | ArcadeDB | 真実源から再構築可能な派生データ（バックアップ対象外） |"
        );
        let _ = writeln!(
            s,
            "| relation graph | ArcadeDB | 公開トピックでの 2 者間のアクションから定期解析で再構築可能な node-local advisory（バックアップ対象外） |"
        );
        let _ = writeln!(
            s,
            "| ランデブー / presence | Valkey | TTL 付きの揮発データ（短期で自動失効） |"
        );
        let _ = writeln!(
            s,
            "| 生メディア | （保存しない） | 走査時の一時取得のみで恒久保存しない |"
        );
        let _ = writeln!(
            s,
            "| indexer のレプリカ同期状態 | ローカル永続 volume | 同期復元用であり content の canonical store ではない |"
        );
        s.push('\n');
        let _ = writeln!(s, "## 削除・再構築・バックアップ\n");
        let _ = writeln!(
            s,
            "- 索引項目は、対象トピックから外れた時点・判定が許可以外へ変わった時点で削除される。"
        );
        let _ = writeln!(
            s,
            "- risk signal は失効（expires_at）と半減期減衰の対象で、申し立ての認容で寄与から除外される。"
        );
        let _ = writeln!(
            s,
            "- ArcadeDB（検索投影・relation graph）は失われても真実源とレプリカから再構築できる。"
        );
        let _ = writeln!(
            s,
            "- バックアップ対象は Postgres のみ。ArcadeDB・Valkey・一時取得メディアはバックアップ対象外。\n"
        );
    }

    let _ = writeln!(s, "## capability 別の保持への影響（運用中）\n");
    for cap in available_enabled(config) {
        let m = cap.meta();
        let _ = writeln!(
            s,
            "- {}: {}",
            m.display_name,
            m.policy_descriptor().retention_text()
        );
    }
    s.push_str(&planned_section(config));
    s
}

fn retention_rows(config: &ResolvedConfig) -> [(&'static str, u32); 12] {
    let r = &config.raw.retention;
    [
        ("通報本体", r.report_days),
        ("通報者連絡先", r.report_contact_days),
        ("未解決の権利侵害申出", r.rights_request_active_days),
        ("措置済みの権利侵害申出", r.rights_request_resolved_days),
        (
            "却下・範囲外・取下げの権利侵害申出",
            r.rights_request_rejected_days,
        ),
        ("申出者連絡先", r.rights_request_contact_days),
        ("本人・代理権確認情報", r.rights_request_identity_days),
        ("証拠参照", r.rights_request_evidence_days),
        ("判断・通知履歴", r.rights_request_history_days),
        ("operator audit", r.operator_audit_days),
        ("signed moderation event", r.moderation_event_days),
        ("risk signal", r.risk_signal_days),
    ]
}

fn gen_prior_consultation_email(config: &ResolvedConfig) -> String {
    let s_cfg = &config.raw.server;
    let mut s = header(config, "事前相談メールテンプレート", None);
    let _ = writeln!(s, "\n## 件名\n");
    let _ = writeln!(
        s,
        "電気通信事業の届出要否に関する事前相談（{}）\n",
        s_cfg.domain
    );
    let _ = writeln!(s, "## 本文（ドラフト）\n");
    let _ = writeln!(s, "```text");
    let _ = writeln!(s, "ご担当者様");
    s.push('\n');
    let _ = writeln!(
        s,
        "{operator} と申します。P2P network の補助層として動作する community node の",
        operator = s_cfg.operator_name
    );
    let _ = writeln!(
        s,
        "運営に関し、電気通信事業の届出要否について事前相談させていただきたくご連絡しました。"
    );
    s.push('\n');
    let _ = writeln!(s, "■ サービス概要");
    let _ = writeln!(s, "- ドメイン: {}", s_cfg.domain);
    let _ = writeln!(
        s,
        "- インフラ: {} / 回線非設置（クラウド / VPS 利用）",
        s_cfg
            .cloud_provider
            .clone()
            .unwrap_or_else(|| "クラウド".to_string())
    );
    let _ = writeln!(
        s,
        "- 役割: ユーザーの identity / profile / social graph を所有しない補助ノード"
    );
    s.push('\n');
    let _ = writeln!(s, "■ 有効な補助機能");
    for cap in available_enabled(config) {
        let _ = writeln!(s, "- {}", cap.meta().display_name);
    }
    if config.enabled(Capability::IrohRelay) || config.enabled(Capability::TrafficRelayFallback) {
        s.push('\n');
        let _ = writeln!(s, "■ relay について");
        let _ = writeln!(
            s,
            "暗号化済み traffic の relay 中継が発生し得ます（signaling only ではありません）。"
        );
    }
    s.push('\n');
    let _ = writeln!(
        s,
        "上記構成における届出要否についてご教示いただけますと幸いです。"
    );
    let _ = writeln!(s, "```\n");
    s
}

/// capability 別リスクと推奨対応ガイド（#359）。
///
/// 個人・小規模運営を discourage しない。各 capability の性質・責任範囲・リスク・推奨対応を
/// 示し、限定された責任範囲で現実的に運用できるようにする。有効 capability を実践ガイドとして、
/// 無効 capability を「引き受けていない責務」として記述する。
fn gen_capability_risk_and_practices(config: &ResolvedConfig) -> String {
    let mut s = header(
        config,
        "Capability 別リスクと推奨対応ガイド（ドラフト）",
        None,
    );

    let _ = writeln!(
        s,
        "\nkukuri は、コミュニティ基盤の運営を企業だけが担うものとは考えない。\
         このガイドは、個人・小規模グループが各 capability の性質を理解し、\
         限定された責任範囲で現実的に運用するための実践的なガイドである。\n"
    );
    let _ = writeln!(
        s,
        "各 capability は authority scope（責任を主張する範囲）と responsibility boundary\
         （引き受けない範囲）を持つ。これらは `docs/architecture/p2p-first-community-node-responsibility-boundary.md`\
         の責任境界と整合する。\n"
    );

    let _ = writeln!(s, "## 有効化している capability\n");
    let enabled = config.enabled_capabilities();
    for cap in &enabled {
        write_capability_risk_section(&mut s, *cap);
    }

    // 無効 capability は「引き受けていない責務」として一覧する。
    let disabled = config.disabled_capabilities();
    if !disabled.is_empty() {
        let _ = writeln!(s, "## 引き受けていない責務（無効な capability）\n");
        let _ = writeln!(
            s,
            "以下の capability は無効であり、本ノードはこれらに関する責務を引き受けていない。\n"
        );
        for cap in disabled {
            let m = cap.meta();
            let _ = writeln!(
                s,
                "- **{}**: {}",
                m.display_name,
                m.policy_descriptor().purpose.label()
            );
        }
        s.push('\n');
    }

    s
}

/// 1 capability 分のリスク・推奨対応セクションを書き出す。
fn write_capability_risk_section(s: &mut String, cap: Capability) {
    let m = cap.meta();
    let descriptor = m.policy_descriptor();
    let rp = cap.risk_practices();
    let availability = match cap.availability() {
        Availability::Available => "提供中（Phase A）",
        Availability::Planned => "計画中・未提供（Phase B）",
    };

    let _ = writeln!(s, "### {}（{}）\n", m.display_name, availability);
    let _ = writeln!(s, "- 機能: {}", descriptor.purpose.label());
    let _ = writeln!(s, "- 取り扱うデータ: {}", descriptor.data_classes_text());
    let _ = writeln!(s, "- user の期待: {}", rp.user_expectation);
    let _ = writeln!(s, "- authority scope: {}", rp.authority_scope);
    let _ = writeln!(s, "- 引き受けない範囲: {}", rp.responsibility_boundary);
    let _ = writeln!(s, "- 保持への影響: {}", descriptor.retention_text());
    let _ = writeln!(
        s,
        "- 削除・訂正・停止等の請求経路: {}",
        descriptor.rights_request_paths_text()
    );

    let _ = writeln!(s, "- 想定リスク:");
    for risk in rp.risks {
        let _ = writeln!(s, "  - {risk}");
    }
    let _ = writeln!(s, "- 推奨対応:");
    for practice in rp.recommended_practices {
        let _ = writeln!(s, "  - {practice}");
    }
    let _ = writeln!(s, "- 小規模運営の tips: {}", rp.small_scale_tips);
    let _ = writeln!(s, "- scope を狭める / 無効化: {}", rp.how_to_reduce);
    s.push('\n');
}

// ---------------------------------------------------------------------------
// 集約
// ---------------------------------------------------------------------------

/// すべての生成文書を filename 昇順で返す。
pub fn generate_all(config: &ResolvedConfig) -> Vec<GeneratedFile> {
    let mut files = vec![
        GeneratedFile {
            filename: MANIFEST_FILE.to_string(),
            content: render_manifest(config),
        },
        GeneratedFile {
            filename: "network-diagram.md".to_string(),
            content: gen_network_diagram(config),
        },
        GeneratedFile {
            filename: "telecom-notification-draft.md".to_string(),
            content: gen_telecom_notification(config),
        },
        GeneratedFile {
            filename: "service-description-draft.md".to_string(),
            content: gen_service_description(config),
        },
        GeneratedFile {
            filename: "terms.md".to_string(),
            content: gen_terms(config),
        },
        GeneratedFile {
            filename: "privacy-policy.md".to_string(),
            content: gen_privacy(config),
        },
        GeneratedFile {
            filename: "external-transmission-notice.md".to_string(),
            content: gen_external_transmission(config),
        },
        GeneratedFile {
            filename: "abuse-policy.md".to_string(),
            content: gen_abuse_policy(config),
        },
        GeneratedFile {
            filename: "rights-infringement-policy.md".to_string(),
            content: gen_rights_infringement_policy(config),
        },
        GeneratedFile {
            filename: "moderation-policy.md".to_string(),
            content: crate::docs_moderation_policy::gen_moderation_policy(config),
        },
        GeneratedFile {
            filename: "data-retention-policy.md".to_string(),
            content: gen_data_retention(config),
        },
        GeneratedFile {
            filename: "prior-consultation-email.md".to_string(),
            content: gen_prior_consultation_email(config),
        },
        GeneratedFile {
            filename: "capability-risk-and-practices.md".to_string(),
            content: gen_capability_risk_and_practices(config),
        },
    ];
    files.extend(crate::docs_trust_observation_sharing::generated_file(
        config,
    ));
    files.sort_by(|a, b| a.filename.cmp(&b.filename));
    files
}
