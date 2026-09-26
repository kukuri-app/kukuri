//! capability が法務文書へ与える構造化された事実。
//!
//! prose は renderer の責務とし、データ分類・処理・利用条件・保持参照・外部送信・
//! 請求経路・safety action・効果範囲はこの descriptor を単一の入力元にする。

use serde::Serialize;

use crate::{Capability, ExternalDestination};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyPurpose {
    AuthenticateAndRecordConsent,
    BootstrapPeerDiscovery,
    MatchTopicPeers,
    AssistEncryptedRelay,
    RelayFallbackTraffic,
    CacheAttachments,
    StoreEncryptedPrivateMessages,
    AnalyzeUsage,
    DiagnoseCrashes,
    ProtectAndDeliverHttpTraffic,
    DeliverPushNotifications,
    IndexSearchAndRecommend,
    ScanAndModerate,
    ComputeNodeLocalTrust,
    IntakeReports,
    IntakeRightsRequests,
    CollectTesterFeedback,
    HostDomeSessions,
}

impl PolicyPurpose {
    pub const fn label(self) -> &'static str {
        match self {
            Self::AuthenticateAndRecordConsent => "利用者を認証し、Node固有文書への同意を記録する",
            Self::BootstrapPeerDiscovery => "P2P networkへ到達するためのseed peer情報を提供する",
            Self::MatchTopicPeers => "同一topicを購読するpeer間の接続成立を補助する",
            Self::AssistEncryptedRelay => "NAT越えと暗号化済み通信のrelayを補助する",
            Self::RelayFallbackTraffic => "直接経路が成立しない場合に暗号化済みtrafficを中継する",
            Self::CacheAttachments => "添付メディアの配信と可用性を一時cacheで補助する",
            Self::StoreEncryptedPrivateMessages => {
                "offline配送のため暗号化済みprivate messageを一時保管する"
            }
            Self::AnalyzeUsage => "サービス改善のため利用状況を分析する",
            Self::DiagnoseCrashes => "不具合の検出と修正のため診断情報を収集する",
            Self::ProtectAndDeliverHttpTraffic => "proxy・CDN・WAFによりHTTP配信を補助し保護する",
            Self::DeliverPushNotifications => "登録deviceへpush通知を配送する",
            Self::IndexSearchAndRecommend => {
                "許可された公開contentの検索・発見・おすすめを補助する"
            }
            Self::ScanAndModerate => "安全性走査とNode localなmoderationを実施する",
            Self::ComputeNodeLocalTrust => {
                "公開情報からNode localなtrust・relation advisoryを算出する"
            }
            Self::IntakeReports => "本Nodeが関与した対象への通報を受け付けて審査する",
            Self::IntakeRightsRequests => "権利侵害、削除、訂正、停止等の申出を受け付けて追跡する",
            Self::CollectTesterFeedback => "品質改善のためtester feedbackを受け付ける",
            Self::HostDomeSessions => "owner署名leaseの範囲でDome sessionをhostする",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRightsRequestPath {
    OperatorContact,
    ConsentWithdrawal,
    DisconnectNode,
    UnregisterDevice,
    ContentWithdrawalOrTransmissionPrevention,
    RightsRequestEndpoint,
    ModerationAppeal,
    RelationOptOut,
}

impl PolicyRightsRequestPath {
    pub const fn label(self) -> &'static str {
        match self {
            Self::OperatorContact => "operatorの公開連絡先",
            Self::ConsentWithdrawal => "Node同意の撤回",
            Self::DisconnectNode => "Node接続の解除・利用停止",
            Self::UnregisterDevice => "通知deviceの登録解除",
            Self::ContentWithdrawalOrTransmissionPrevention => "投稿撤回またはNodeへの送信防止請求",
            Self::RightsRequestEndpoint => "権利侵害申出endpoint",
            Self::ModerationAppeal => "moderation判定への異議申立て",
            Self::RelationOptOut => "relation表示のopt-out",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDataClass {
    AuthenticationAndConsent,
    ConnectivityMetadata,
    TopicPresence,
    EncryptedTransitTraffic,
    CachedAttachment,
    EncryptedPrivateMessage,
    UsageAnalytics,
    CrashDiagnostics,
    HttpTraffic,
    PushDelivery,
    PublicContentAndMetadata,
    SafetyAssessment,
    CommunityRelation,
    UserReport,
    RightsClaim,
    TesterFeedback,
    DomeSession,
}

impl PolicyDataClass {
    pub const fn label(self) -> &'static str {
        match self {
            Self::AuthenticationAndConsent => "公開鍵・認証情報・同意記録",
            Self::ConnectivityMetadata => "接続先・接続元・到達性メタデータ",
            Self::TopicPresence => "topic presence と一時的な接続ヒント",
            Self::EncryptedTransitTraffic => "暗号化済みの中継 traffic",
            Self::CachedAttachment => "一時キャッシュされた添付メディアと CID",
            Self::EncryptedPrivateMessage => "暗号化されたプライベートメッセージ",
            Self::UsageAnalytics => "利用状況の統計・イベント",
            Self::CrashDiagnostics => "クラッシュ診断・スタックトレース",
            Self::HttpTraffic => "HTTP request/response と接続元 IP",
            Self::PushDelivery => "device token と通知 payload",
            Self::PublicContentAndMetadata => "公開 topic の本文・metadata・再構築可能な索引",
            Self::SafetyAssessment => "scan verdict・moderation event・risk signal・異議状態",
            Self::CommunityRelation => {
                "公開 topic での 2 者間のアクション由来の関係・node-local trust"
            }
            Self::UserReport => "通報対象・理由・補足・任意の連絡先",
            Self::RightsClaim => "申出人情報・権利根拠・対象・証拠参照",
            Self::TesterFeedback => "自由記述 feedback・client version・OS",
            Self::DomeSession => "Hosting Lease・manifest・input・ephemeral state",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyProcessing {
    Authenticate,
    MatchPeers,
    Relay,
    Cache,
    TemporarilyStore,
    Analyze,
    DeliverNotification,
    IndexAndRecommend,
    SafetyScanAndModerate,
    ComputeAdvisory,
    IntakeAndReview,
    HostSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyUsageCondition {
    ExplicitNodeConsent,
    ConnectionAttempt,
    EnabledFeature,
    SupportedPublicTopic,
    UserSubmission,
    OwnerSignedLease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRetentionRef {
    ShortTtl,
    UntilConsentWithdrawal,
    ConnectionLogsDays,
    TemporaryCache,
    ProviderPolicy,
    UntilUnregistered,
    UntilDelivered,
    IndexEligibility,
    ModerationLogsDays,
    ModerationEventDays,
    RiskSignalDays,
    ReportDays,
    RightsRequestLifecycle,
    TesterFeedbackDays,
    LeaseOrSessionEnd,
}

impl PolicyRetentionRef {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ShortTtl => "短期 TTL で失効",
            Self::UntilConsentWithdrawal => {
                "同意撤回まで（公開済み文書・同意履歴は監査履歴として保持）"
            }
            Self::ConnectionLogsDays => "retention.connection_logs_days",
            Self::TemporaryCache => "一時キャッシュ（恒久保存なし）",
            Self::ProviderPolicy => "operator が契約する外部 provider の保持方針",
            Self::UntilUnregistered => "登録解除まで",
            Self::UntilDelivered => "配送完了まで",
            Self::IndexEligibility => "対象 topic・許可判定の条件を満たす間",
            Self::ModerationLogsDays => "retention.moderation_logs_days",
            Self::ModerationEventDays => "retention.moderation_event_days",
            Self::RiskSignalDays => "retention.risk_signal_days または明示された失効まで",
            Self::ReportDays => "retention.report_days / report_contact_days",
            Self::RightsRequestLifecycle => "retention.rights_request_*_days",
            Self::TesterFeedbackDays => "retention.tester_feedback_days",
            Self::LeaseOrSessionEnd => "lease close・期限または session 終了まで",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyBillingPath {
    None,
    OperatorProviderContract,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySafetyAction {
    None,
    FailClosedHold,
    ExcludeFromNodeIndex,
    SignedNodeLocalEvent,
    Appeal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffectScope {
    ThisNode,
    ConnectionOnly,
    SupportedPublicTopics,
    NodeLocalAdvisory,
    SubmittedRequest,
    OwnerSignedLease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct CapabilityPolicyDescriptor {
    pub purpose: PolicyPurpose,
    pub data_classes: &'static [PolicyDataClass],
    pub processing: &'static [PolicyProcessing],
    pub usage_condition: PolicyUsageCondition,
    pub retention: &'static [PolicyRetentionRef],
    pub external_destinations: &'static [ExternalDestination],
    pub billing_path: PolicyBillingPath,
    pub safety_actions: &'static [PolicySafetyAction],
    pub effect_scope: PolicyEffectScope,
    pub rights_request_paths: &'static [PolicyRightsRequestPath],
}

impl CapabilityPolicyDescriptor {
    pub fn data_classes_text(self) -> String {
        self.data_classes
            .iter()
            .map(|value| value.label())
            .collect::<Vec<_>>()
            .join("、")
    }

    pub fn retention_text(self) -> String {
        self.retention
            .iter()
            .map(|value| value.label())
            .collect::<Vec<_>>()
            .join("、")
    }

    pub fn rights_request_paths_text(self) -> String {
        self.rights_request_paths
            .iter()
            .map(|value| value.label())
            .collect::<Vec<_>>()
            .join("、")
    }

    pub fn policy_summary_text(self) -> String {
        format!(
            "目的は{}。取扱いデータは{}。保持は{}を参照し、削除・訂正・停止等の申出は{}で受け付けます。",
            self.purpose.label(),
            self.data_classes_text(),
            self.retention_text(),
            self.rights_request_paths_text(),
        )
    }
}

impl Capability {
    pub fn policy_descriptor(self) -> CapabilityPolicyDescriptor {
        use PolicyBillingPath::{None as NoBilling, OperatorProviderContract};
        use PolicyDataClass::*;
        use PolicyEffectScope::*;
        use PolicyProcessing::*;
        use PolicyRetentionRef::*;
        use PolicyRightsRequestPath::*;
        use PolicySafetyAction::*;
        use PolicyUsageCondition::*;

        let purpose = match self {
            Self::AuthConsent => PolicyPurpose::AuthenticateAndRecordConsent,
            Self::BootstrapAssist => PolicyPurpose::BootstrapPeerDiscovery,
            Self::TopicRendezvous => PolicyPurpose::MatchTopicPeers,
            Self::IrohRelay => PolicyPurpose::AssistEncryptedRelay,
            Self::TrafficRelayFallback => PolicyPurpose::RelayFallbackTraffic,
            Self::BlobCache => PolicyPurpose::CacheAttachments,
            Self::PrivateMessageStorage => PolicyPurpose::StoreEncryptedPrivateMessages,
            Self::Analytics => PolicyPurpose::AnalyzeUsage,
            Self::CrashReport => PolicyPurpose::DiagnoseCrashes,
            Self::CloudflareProxy => PolicyPurpose::ProtectAndDeliverHttpTraffic,
            Self::PushNotification => PolicyPurpose::DeliverPushNotifications,
            Self::CommunityIndex => PolicyPurpose::IndexSearchAndRecommend,
            Self::Moderation => PolicyPurpose::ScanAndModerate,
            Self::CommunityLocalTrust => PolicyPurpose::ComputeNodeLocalTrust,
            Self::ReportEndpoint => PolicyPurpose::IntakeReports,
            Self::RightsRequestEndpoint => PolicyPurpose::IntakeRightsRequests,
            Self::TesterFeedback => PolicyPurpose::CollectTesterFeedback,
            Self::DomeHosting => PolicyPurpose::HostDomeSessions,
        };

        let common = |data_classes, processing, usage_condition, retention, effect_scope| {
            CapabilityPolicyDescriptor {
                purpose,
                data_classes,
                processing,
                usage_condition,
                retention,
                external_destinations: &[],
                billing_path: NoBilling,
                safety_actions: &[PolicySafetyAction::None],
                effect_scope,
                rights_request_paths: &[OperatorContact],
            }
        };
        match self {
            Self::AuthConsent => CapabilityPolicyDescriptor {
                rights_request_paths: &[ConsentWithdrawal, DisconnectNode, OperatorContact],
                ..common(
                    &[AuthenticationAndConsent],
                    &[Authenticate],
                    ExplicitNodeConsent,
                    &[ShortTtl, UntilConsentWithdrawal],
                    ThisNode,
                )
            },
            Self::BootstrapAssist => CapabilityPolicyDescriptor {
                rights_request_paths: &[DisconnectNode, OperatorContact],
                ..common(
                    &[ConnectivityMetadata],
                    &[MatchPeers],
                    ConnectionAttempt,
                    &[ShortTtl],
                    ConnectionOnly,
                )
            },
            Self::TopicRendezvous => CapabilityPolicyDescriptor {
                rights_request_paths: &[DisconnectNode, OperatorContact],
                ..common(
                    &[TopicPresence],
                    &[MatchPeers],
                    ConnectionAttempt,
                    &[ShortTtl],
                    ConnectionOnly,
                )
            },
            Self::IrohRelay => CapabilityPolicyDescriptor {
                external_destinations: &[ExternalDestination::DedicatedIrohRelay],
                billing_path: OperatorProviderContract,
                ..common(
                    &[ConnectivityMetadata, EncryptedTransitTraffic],
                    &[Relay],
                    ConnectionAttempt,
                    &[ConnectionLogsDays],
                    ConnectionOnly,
                )
            },
            Self::TrafficRelayFallback => CapabilityPolicyDescriptor {
                external_destinations: &[ExternalDestination::PublicRelay],
                billing_path: OperatorProviderContract,
                ..common(
                    &[ConnectivityMetadata, EncryptedTransitTraffic],
                    &[Relay],
                    ConnectionAttempt,
                    &[ConnectionLogsDays],
                    ConnectionOnly,
                )
            },
            Self::BlobCache => CapabilityPolicyDescriptor {
                external_destinations: &[ExternalDestination::ObjectStorage],
                billing_path: OperatorProviderContract,
                ..common(
                    &[CachedAttachment],
                    &[Cache],
                    EnabledFeature,
                    &[TemporaryCache],
                    ThisNode,
                )
            },
            Self::PrivateMessageStorage => common(
                &[EncryptedPrivateMessage],
                &[TemporarilyStore],
                EnabledFeature,
                &[TemporaryCache],
                ThisNode,
            ),
            Self::Analytics => CapabilityPolicyDescriptor {
                external_destinations: &[ExternalDestination::AnalyticsProvider],
                billing_path: OperatorProviderContract,
                ..common(
                    &[UsageAnalytics],
                    &[Analyze],
                    EnabledFeature,
                    &[ProviderPolicy],
                    ThisNode,
                )
            },
            Self::CrashReport => CapabilityPolicyDescriptor {
                external_destinations: &[ExternalDestination::CrashReportProvider],
                billing_path: OperatorProviderContract,
                ..common(
                    &[CrashDiagnostics],
                    &[Analyze],
                    EnabledFeature,
                    &[ProviderPolicy],
                    ThisNode,
                )
            },
            Self::CloudflareProxy => CapabilityPolicyDescriptor {
                external_destinations: &[ExternalDestination::Cloudflare],
                billing_path: OperatorProviderContract,
                ..common(
                    &[HttpTraffic],
                    &[Relay, Cache],
                    ConnectionAttempt,
                    &[ProviderPolicy],
                    ConnectionOnly,
                )
            },
            Self::PushNotification => CapabilityPolicyDescriptor {
                external_destinations: &[ExternalDestination::PushProvider],
                billing_path: OperatorProviderContract,
                ..common(
                    &[PushDelivery],
                    &[DeliverNotification],
                    EnabledFeature,
                    &[UntilUnregistered, UntilDelivered],
                    ThisNode,
                )
            },
            Self::CommunityIndex => common(
                &[PublicContentAndMetadata],
                &[IndexAndRecommend],
                SupportedPublicTopic,
                &[IndexEligibility],
                SupportedPublicTopics,
            ),
            Self::Moderation => CapabilityPolicyDescriptor {
                safety_actions: &[
                    FailClosedHold,
                    ExcludeFromNodeIndex,
                    SignedNodeLocalEvent,
                    Appeal,
                ],
                rights_request_paths: &[
                    ContentWithdrawalOrTransmissionPrevention,
                    ModerationAppeal,
                    RightsRequestEndpoint,
                    OperatorContact,
                ],
                ..common(
                    &[SafetyAssessment],
                    &[SafetyScanAndModerate],
                    SupportedPublicTopic,
                    &[ModerationLogsDays, ModerationEventDays, RiskSignalDays],
                    SupportedPublicTopics,
                )
            },
            Self::CommunityLocalTrust => CapabilityPolicyDescriptor {
                safety_actions: &[Appeal],
                rights_request_paths: &[RelationOptOut, ModerationAppeal, OperatorContact],
                ..common(
                    &[CommunityRelation, SafetyAssessment],
                    &[ComputeAdvisory],
                    SupportedPublicTopic,
                    &[RiskSignalDays, IndexEligibility],
                    NodeLocalAdvisory,
                )
            },
            Self::ReportEndpoint => CapabilityPolicyDescriptor {
                rights_request_paths: &[RightsRequestEndpoint, OperatorContact],
                ..common(
                    &[UserReport],
                    &[IntakeAndReview],
                    UserSubmission,
                    &[ReportDays],
                    SubmittedRequest,
                )
            },
            Self::RightsRequestEndpoint => CapabilityPolicyDescriptor {
                rights_request_paths: &[RightsRequestEndpoint, OperatorContact],
                ..common(
                    &[RightsClaim],
                    &[IntakeAndReview],
                    UserSubmission,
                    &[RightsRequestLifecycle],
                    SubmittedRequest,
                )
            },
            Self::TesterFeedback => common(
                &[TesterFeedback],
                &[IntakeAndReview],
                UserSubmission,
                &[TesterFeedbackDays],
                SubmittedRequest,
            ),
            Self::DomeHosting => common(
                &[DomeSession],
                &[HostSession],
                PolicyUsageCondition::OwnerSignedLease,
                &[LeaseOrSessionEnd],
                PolicyEffectScope::OwnerSignedLease,
            ),
        }
    }
}
