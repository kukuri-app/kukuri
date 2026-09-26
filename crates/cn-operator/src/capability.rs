//! community node の capability モデル。
//!
//! #352 の operator docs generator は、各機能を boolean トグルとしてだけでなく、
//! 説明責任・外部送信・保持影響を持つ capability として扱う。
//!
//! ここで重要なのは `Availability` による Phase A / Phase B の分離である。
//!
//! - `Availability::Available` (Phase A): 現行の community node 実装が実際に提供できる、
//!   またはデプロイ構成として確定できる capability。生成文書では「運用中」として開示してよい。
//! - `Availability::Planned` (Phase B): 将来追加され、現行実装に存在しない capability。
//!   config 上は宣言できるが、生成文書では
//!   「計画中・この配布物では未提供」として扱い、運用中の外部送信・データ取扱い開示には載せない。
//!
//! これは `docs/architecture/p2p-first-community-node-responsibility-boundary.md` の
//! 「node manifest が宣言した capability / authority scope が責任範囲の上限」という方針に従い、
//! 「宣言」と「実際に実行可能」を分離して、実体のない開示を生成しないためのガードである。

use std::fmt;

/// capability が現行配布物で実行可能か（Phase A）、設計のみ（Phase B）か。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Phase A: 現行実装またはデプロイ構成として提供可能。運用中として開示してよい。
    Available,
    /// Phase B: 設計・spec のみ。生成文書では「計画中・未提供」として扱う。
    Planned,
}

impl Availability {
    pub fn is_planned(self) -> bool {
        matches!(self, Availability::Planned)
    }

    /// 文書中の日本語ラベル。
    pub fn label_ja(self) -> &'static str {
        match self {
            Availability::Available => "提供中",
            Availability::Planned => "計画中（この配布物では未提供）",
        }
    }
}

/// community node が提供し得る capability。
///
/// `ALL` の並び順が生成文書・manifest の決定論的な出力順序になる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    // --- Phase A: 現行実装 / デプロイ構成 ---
    AuthConsent,
    BootstrapAssist,
    TopicRendezvous,
    IrohRelay,
    TrafficRelayFallback,
    BlobCache,
    PrivateMessageStorage,
    Analytics,
    CrashReport,
    CloudflareProxy,
    PushNotification,
    // Operational capabilities (availability is decided by `availability()`).
    CommunityIndex,
    Moderation,
    CommunityLocalTrust,
    ReportEndpoint,
    RightsRequestEndpoint,
    TesterFeedback,
    DomeHosting,
}

impl Capability {
    /// 決定論的な出力順序を与える全 capability。
    pub const ALL: [Capability; 18] = [
        Capability::AuthConsent,
        Capability::BootstrapAssist,
        Capability::TopicRendezvous,
        Capability::IrohRelay,
        Capability::TrafficRelayFallback,
        Capability::BlobCache,
        Capability::PrivateMessageStorage,
        Capability::Analytics,
        Capability::CrashReport,
        Capability::CloudflareProxy,
        Capability::PushNotification,
        Capability::CommunityIndex,
        Capability::Moderation,
        Capability::CommunityLocalTrust,
        Capability::ReportEndpoint,
        Capability::RightsRequestEndpoint,
        Capability::TesterFeedback,
        Capability::DomeHosting,
    ];

    /// config / manifest の snake_case キー。
    pub fn key(self) -> &'static str {
        match self {
            Capability::AuthConsent => "auth_consent",
            Capability::BootstrapAssist => "bootstrap_assist",
            Capability::TopicRendezvous => "topic_rendezvous",
            Capability::IrohRelay => "iroh_relay",
            Capability::TrafficRelayFallback => "traffic_relay_fallback",
            Capability::BlobCache => "blob_cache",
            Capability::PrivateMessageStorage => "private_message_storage",
            Capability::Analytics => "analytics",
            Capability::CrashReport => "crash_report",
            Capability::CloudflareProxy => "cloudflare_proxy",
            Capability::PushNotification => "push_notification",
            Capability::CommunityIndex => "community_index",
            Capability::Moderation => "moderation",
            Capability::CommunityLocalTrust => "community_local_trust",
            Capability::ReportEndpoint => "report_endpoint",
            Capability::RightsRequestEndpoint => "rights_request_endpoint",
            Capability::TesterFeedback => "tester_feedback",
            Capability::DomeHosting => "dome_hosting",
        }
    }

    pub fn availability(self) -> Availability {
        // index / moderation / local trust は #616 の実行時準備判定・有効化の関門・
        // 全構成 E2E の完了により提供中へ昇格した（#617）。現時点で計画中の capability は
        // 無いが、将来の Phase B 追加に備えて区分と分離表示の仕組みは残す。
        Availability::Available
    }

    pub fn display_name(self) -> &'static str {
        self.meta().display_name
    }

    /// 文書生成用の静的メタデータ。
    pub fn meta(self) -> CapabilityMeta {
        match self {
            Capability::AuthConsent => CapabilityMeta {
                capability: self,
                display_name: "認証・同意 (auth / consent)",
                purpose: "community node の補助機能を利用する client の認証と、利用規約・ポリシーへの同意取得",
                telecom_note: "認証はノード自身が処理する。回線設備の設置を伴わない。",
                privacy_note: "公開鍵と同意状態を扱う。IP アドレス・接続ログの扱いは接続ログ保持期間に従う。",
                terms_note: "本ノードの補助機能利用には認証と同意が必要である旨を記載する。",
            },
            Capability::BootstrapAssist => CapabilityMeta {
                capability: self,
                display_name: "ブートストラップ補助 (bootstrap assist)",
                purpose: "新規 client が P2P network へ最初に到達するための seed peer 情報の提供",
                telecom_note: "接続ヒントの中継のみ。実データの伝送経路を恒久的に保持しない。",
                privacy_note: "一時的な接続情報を扱う。長期保存はしない。",
                terms_note: "P2P 接続を補助する目的であり、通信内容を保持しない旨を記載する。",
            },
            Capability::TopicRendezvous => CapabilityMeta {
                capability: self,
                display_name: "トピックランデブー (topic rendezvous)",
                purpose: "同一 topic を購読する client 同士の Relay Supported P2P 接続成立を補助する",
                telecom_note: "presence の一時的な突き合わせのみ。実データ伝送の恒久経路を持たない。",
                privacy_note: "どの topic に接続中かの一時情報を扱う。長期保存はしない。",
                terms_note: "topic 接続の補助であり、投稿内容を保持しない旨を記載する。",
            },
            Capability::IrohRelay => CapabilityMeta {
                capability: self,
                display_name: "iroh relay 補助 (iroh relay assist)",
                purpose: "Direct P2P が成立しない場合の hole punching / endpoint assist",
                telecom_note: "iroh relay は単なる signaling ではなく、NAT 越えのために暗号化済み traffic の中継が発生し得る。届出要否は構成と所在地に依存するため事前確認が必要。",
                privacy_note: "中継時に接続元の IP アドレスを観測し得る。接続ログ保持期間に従って扱う。",
                terms_note: "relay 経由の接続補助が行われ得る旨を記載する。",
            },
            Capability::TrafficRelayFallback => CapabilityMeta {
                capability: self,
                display_name: "トラフィック relay フォールバック (traffic relay fallback)",
                purpose: "他のすべての経路が成立しない場合に限り、暗号化済み traffic を relay 経由で疎通させる",
                telecom_note: "暗号化済みであっても traffic relay fallback は実データの伝送経路となり得る。signaling only と混同せず、届出要否を事前確認する。",
                privacy_note: "fallback 時に接続元 IP アドレスやタイミングメタデータを観測し得る。",
                terms_note: "最終手段として暗号化済み通信が relay 経由になり得る旨を記載する。",
            },
            Capability::BlobCache => CapabilityMeta {
                capability: self,
                display_name: "blob / 添付キャッシュ (blob cache)",
                purpose: "添付メディアの配信補助・可用性向上のための一時キャッシュ",
                telecom_note: "メディア配信補助。恒久保存を行わない方針を明記する。",
                privacy_note: "添付メディアを一時的に扱う。保持期間と削除方針を明記する。",
                terms_note: "添付メディアが一時的にキャッシュされ得る旨を記載する。",
            },
            Capability::PrivateMessageStorage => CapabilityMeta {
                capability: self,
                display_name: "プライベートメッセージ保管 (private message storage)",
                purpose: "オフライン配送のためのプライベートメッセージの一時保管",
                telecom_note: "メッセージ保管はノード内処理。回線設備の設置を伴わない。",
                privacy_note: "プライベートメッセージを扱うため、保持期間・暗号化・アクセス制御を明記する。",
                terms_note: "プライベートメッセージが一時保管され得る旨と暗号化方針を記載する。",
            },
            Capability::Analytics => CapabilityMeta {
                capability: self,
                display_name: "アナリティクス (analytics)",
                purpose: "サービス改善のための利用状況分析",
                telecom_note: "分析目的のデータ送信が発生し得る。",
                privacy_note: "利用状況データを第三者の分析プロバイダへ送信し得る旨を明記する。",
                terms_note: "アナリティクス目的のデータ収集が行われ得る旨を記載する。",
            },
            Capability::CrashReport => CapabilityMeta {
                capability: self,
                display_name: "クラッシュレポート (crash reporting)",
                purpose: "不具合の検出と修正のためのクラッシュ情報収集",
                telecom_note: "診断目的のデータ送信が発生し得る。",
                privacy_note: "クラッシュ診断データを第三者プロバイダへ送信し得る旨を明記する。",
                terms_note: "クラッシュレポートが送信され得る旨を記載する。",
            },
            Capability::CloudflareProxy => CapabilityMeta {
                capability: self,
                display_name: "Cloudflare プロキシ / CDN / WAF",
                purpose: "リバースプロキシ・CDN・WAF による配信補助と保護",
                telecom_note: "通信が Cloudflare を経由する。所在地・データ越境の観点を確認する。",
                privacy_note: "リクエストと接続元 IP が Cloudflare を経由する旨を外部送信として明記する。",
                terms_note: "通信が Cloudflare を経由し得る旨を記載する。",
            },
            Capability::PushNotification => CapabilityMeta {
                capability: self,
                display_name: "プッシュ通知 (push notification)",
                purpose: "OS プッシュ通知の配信",
                telecom_note: "プッシュ通知プロバイダ経由の送信が発生する。",
                privacy_note: "デバイストークンと通知内容をプッシュプロバイダへ送信する旨を明記する。",
                terms_note: "プッシュ通知のためにデバイストークンを扱う旨を記載する。",
            },
            Capability::CommunityIndex => CapabilityMeta {
                capability: self,
                display_name: "コミュニティインデックス (community index)",
                purpose: "安全性走査を通過した許可 content のみを対象とする検索・発見・おすすめの補助",
                telecom_note: "索引と検索・発見・おすすめの権限は、本ノードのサポート対象（公開トピック）\
                    内に限定される。本ノードは content の真実源ではない。",
                privacy_note: "公開トピックへ公開された投稿のみを対象とし、走査済みの許可 content のみを\
                    索引する。生メディアを保持しない。",
                terms_note: "本ノードが索引した content の範囲についてのみ責任を負い、検索・発見・おすすめは\
                    本ノードの authority scope 内に限定される旨を記載する。",
            },
            Capability::Moderation => CapabilityMeta {
                capability: self,
                display_name: "モデレーション (moderation)",
                purpose: "既知一致照合（Project Arachnid Shield）と分類器（OpenAI 互換の視覚言語モデル）に\
                    よる走査で、critical safety risk を本ノードの索引・発見・おすすめから排除する。\
                    走査は fail-closed（走査失敗・プロバイダ不達・メディア不達は許可へ落とさず保留）",
                telecom_note: "moderation は node-local の判断であり、本ノードの authority scope 内に\
                    限定される。network 全体への命令ではない。",
                privacy_note: "走査のためメディアを一時取得する（恒久保存しない）。プロバイダの Match Data \
                    を保存・配布・AI 入力に使わない。プロバイダへの送信内容は外部送信ポリシーに明記する。",
                terms_note: "moderation event は本ノードの authority scope 内でのみ意味を持ち、判定への\
                    申し立て（異議）の導線を提供する旨を記載する。",
            },
            Capability::CommunityLocalTrust => CapabilityMeta {
                capability: self,
                display_name: "コミュニティローカル信頼・関係 (community-local trust / relation)",
                purpose: "閲覧者に固定された node-local advisory としての信頼・関係の読み取り提供\
                    （trust と relation の双方を含む）",
                telecom_note: "信頼・関係は node-local advisory であり、本ノードの authority scope 内に\
                    限定される。canonical な identity / social graph を変更しない。",
                privacy_note: "関係の入力は公開トピックでの 2 者間のアクション（返信・repost・引用・リアクション）と\
                    公開のフォローだけで、プライベートチャンネル由来の信号は使わない。関係表示の離脱（opt-out）は可逆で、信頼値に影響しない。",
                terms_note: "trust signal は network-wide command ではなく optional な入力であり、\
                    cross-cluster content を自動抑制しない旨を記載する。",
            },
            Capability::ReportEndpoint => CapabilityMeta {
                capability: self,
                display_name: "通報エンドポイント (report endpoint)",
                purpose: "本ノードが関与した対象に対する通報の受付（POST /v1/report）",
                telecom_note: "通報受付はノードの authority scope 内に限定される。",
                privacy_note: "reporter の identity / social graph は保持せず、明示入力された連絡先のみ任意保存する。",
                terms_note: "通報は本ノードが関与した対象に限定され、中央通報窓口ではない。",
            },
            Capability::RightsRequestEndpoint => CapabilityMeta {
                capability: self,
                display_name: "権利侵害申出エンドポイント (rights request endpoint)",
                purpose: "本ノードの対応範囲を事前確認した権利者等から、権利侵害申出を受け付けて追跡可能にする",
                telecom_note: "措置は本ノードの索引・moderation・cache 等の authority scope 内に限定される。",
                privacy_note: "申出人情報と権利主張は local-only とし、公開 status へ PII や内部判断を出さない。",
                terms_note: "申請前に可能・不可能な措置を提示し、版付きの明示同意を必須にする。",
            },
            Capability::TesterFeedback => CapabilityMeta {
                capability: self,
                display_name: "テスターフィードバック受付 (tester feedback)",
                purpose: "テスターの利用経験レポートの受付・蓄積(POST /v1/tester-feedback)。品質観点の発見と自動テスト化の元データにする",
                telecom_note: "フィードバック受付はノード内処理。回線設備の設置を伴わない。",
                privacy_note: "送信者の identity / social graph は保持しない。自由記述と自動付与された client version / OS のみ保存する。",
                terms_note: "テスターフィードバックが本ノードへ送信・保存され得る旨を記載する。",
            },
            Capability::DomeHosting => CapabilityMeta {
                capability: self,
                display_name: "Dome ホスティング (dome hosting)",
                purpose: "owner不在時も単一のauthoritative hostとしてDome sessionを継続する",
                telecom_note: "Dome participantとのsession trafficを終端する。帯域と同時session数を制限する。",
                privacy_note: "participant inputは処理後に破棄し、raw inputや認証tokenをlogへ出さない。",
                terms_note: "Community NodeはDomeのcanonical ownerではなく、owner署名leaseの範囲だけをhostする。",
            },
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// 文書生成に使う capability の静的メタデータ。
#[derive(Clone, Copy, Debug)]
pub struct CapabilityMeta {
    pub capability: Capability,
    pub display_name: &'static str,
    pub purpose: &'static str,
    pub telecom_note: &'static str,
    pub privacy_note: &'static str,
    pub terms_note: &'static str,
}

impl CapabilityMeta {
    /// データ分類・処理・保持等の法務上の事実は prose field ではなく、この型付き
    /// descriptor を参照する。既存 note は各文書向けの説明文だけに利用する。
    pub fn policy_descriptor(self) -> crate::policy_descriptor::CapabilityPolicyDescriptor {
        self.capability.policy_descriptor()
    }
}

/// 外部送信表示で列挙し得る送信先。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalDestination {
    CommunityServer,
    DedicatedIrohRelay,
    PublicRelay,
    Cloudflare,
    ObjectStorage,
    PushProvider,
    AnalyticsProvider,
    CrashReportProvider,
}

impl ExternalDestination {
    pub fn display_name(self) -> &'static str {
        match self {
            ExternalDestination::CommunityServer => "コミュニティサーバー本体",
            ExternalDestination::DedicatedIrohRelay => "専用 iroh relay",
            ExternalDestination::PublicRelay => "n0.computer 等のパブリック relay",
            ExternalDestination::Cloudflare => "Cloudflare (プロキシ / CDN / WAF)",
            ExternalDestination::ObjectStorage => "オブジェクトストレージ",
            ExternalDestination::PushProvider => "プッシュ通知プロバイダ",
            ExternalDestination::AnalyticsProvider => "アナリティクスプロバイダ",
            ExternalDestination::CrashReportProvider => "クラッシュレポートプロバイダ",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ExternalDestination::CommunityServer => {
                "client からの接続を受け、補助機能を提供する本ノード。"
            }
            ExternalDestination::DedicatedIrohRelay => {
                "NAT traversal / hole punching を補助する専用 relay。暗号化済み traffic の中継が発生し得る。"
            }
            ExternalDestination::PublicRelay => {
                "他経路が成立しない場合の fallback として、暗号化済み traffic が経由し得るパブリック relay。"
            }
            ExternalDestination::Cloudflare => {
                "リバースプロキシ / CDN / WAF。HTTP リクエストと接続元 IP が経由する。"
            }
            ExternalDestination::ObjectStorage => "添付メディアの一時キャッシュ配信先。",
            ExternalDestination::PushProvider => "デバイストークンと通知内容の送信先。",
            ExternalDestination::AnalyticsProvider => "利用状況データの送信先。",
            ExternalDestination::CrashReportProvider => "クラッシュ診断データの送信先。",
        }
    }
}
