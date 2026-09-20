import { type CommunityNodeAdmissionRejectionCode } from '@/lib/api';
import { type DesktopTheme } from '@/lib/theme';
import { type SupportedLocale } from '@/i18n';
import { type BookmarkedCustomReactionView, type CustomReactionAssetView } from '@/lib/api';

import type { ConnectivityGuidance } from '@/shell/connectivityGuidance';

export type SettingsPanelStatus = 'loading' | 'ready' | 'error';

export type SettingsMetricView = {
  label: string;
  value: string;
  tone?: 'default' | 'accent' | 'warning' | 'danger';
};

export type SettingsDiagnosticItemView = {
  label: string;
  value: string;
  tone?: 'default' | 'danger';
  monospace?: boolean;
};

export type ConnectivityTopicDetailView = {
  guidance?: ConnectivityGuidance;
  topic: string;
  summary: string;
  lastReceivedLabel: string;
  expectedPeerCount: number | null;
  missingPeerCount: number | null;
  statusDetail: string;
  connectedPeersLabel: string;
  relayAssistedPeersLabel: string;
  configuredPeersLabel: string;
  missingPeersLabel: string;
  lastError?: string | null;
};

export type ConnectivityPanelView = {
  guidance?: ConnectivityGuidance;
  status: SettingsPanelStatus;
  summaryLabel: string;
  panelError?: string | null;
  metrics: SettingsMetricView[];
  diagnostics: SettingsDiagnosticItemView[];
  localPeerTicket: string;
  peerTicketInput: string;
  topics: ConnectivityTopicDetailView[];
};

export type DiscoveryPanelView = {
  guidance?: ConnectivityGuidance;
  status: SettingsPanelStatus;
  summaryLabel: string;
  panelError?: string | null;
  metrics: SettingsMetricView[];
  diagnostics: SettingsDiagnosticItemView[];
  seedPeersInput: string;
  seedPeersMessage?: string;
  seedPeersMessageTone?: 'default' | 'danger';
  envLocked: boolean;
};

// public manifest (#356) 由来の依存度 / capability scope / authority scope 表示。
export type CommunityNodeDependencyView = {
  // role / origin / manifest status / capability scope / authority scope を行として表示する。
  diagnostics: SettingsDiagnosticItemView[];
  // identity / profile / social graph が node-owned ではない等、常に表示する責任境界の説明。
  boundaryNotes: string[];
  // manifest fetch が失敗した場合のエラー（client は default node へ fallback しない）。
  manifestError?: string | null;
};

// per-node consent ダイアログ（#384）で表示する 1 ポリシー分の行。
export type CommunityNodeConsentPolicyView = {
  policySlug: string;
  title: string;
  body: string;
  policyVersion: number;
  effectiveDate?: string | null;
  language?: string | null;
  policySnapshotRevision?: string | null;
  authoritativeLanguage?: string | null;
  referenceTranslation: boolean;
  fallback: boolean;
  required: boolean;
  /// #1192: 公開 policy カタログの `policy_kind`。slug ではなくこれで文書の役割を判別する。
  policyKind?: string | null;
  acceptedAtLabel: string | null;
  // 版が上がって再同意が必要な「更新」状態か。
  updated: boolean;
  previouslyAcceptedVersion: number | null;
};

// per-node consent ダイアログ全体の表示状態(#857: 提示は認証不要の公開カタログ、
// 受諾状態はローカル同意記録から導く)。
export type CommunityNodeConsentView = {
  // 公開 policy カタログを取得できているか（未取得なら本文表示前に取得を促す）。
  loaded: boolean;
  loading: boolean;
  // 取得失敗（オフライン等）。ダイアログ内で再試行できる。
  loadError: string | null;
  // 同意が撤回済みか。
  withdrawn: boolean;
  // 過去に受諾したローカル同意記録があるか（撤回済みは除く）。
  hasLocalConsent: boolean;
  allRequiredAccepted: boolean;
  // 更新による未同意（再同意要求）が 1 つでもあるか。
  hasPendingUpdate: boolean;
  policies: CommunityNodeConsentPolicyView[];
};

export type CommunityNodeEntryView = {
  id: string;
  baseUrl: string;
  // 公開manifest由来。Dome hosting等、暗号学的Node IDが必要な機能で自由入力を避ける。
  nodeId?: string | null;
  nodeName?: string | null;
  saved: boolean;
  diagnostics: SettingsDiagnosticItemView[];
  dependency: CommunityNodeDependencyView;
  consent: CommunityNodeConsentView;
  // 距離利用停止の読込・設定・解除ができる利用可否(認証・必須同意・通信・提供中能力。#705)。
  distanceOptoutEligible: boolean;
  // #1056: この node の content advisory(成人向け表現の推定)を採用するか。
  contentAdvisoryEnabled?: boolean;
  inviteCodeSaved: boolean;
  admissionRejectionCode?: CommunityNodeAdmissionRejectionCode | null;
  lastError?: string | null;
};

export type CommunityNodePanelView = {
  status: SettingsPanelStatus;
  summaryLabel: string;
  panelError?: string | null;
  editorMessage?: string;
  editorMessageTone?: 'default' | 'danger';
  nodes: CommunityNodeEntryView[];
};

export type AppearanceOptionView = {
  value: DesktopTheme;
  label: string;
  description: string;
};

export type AppearancePanelView = {
  selectedTheme: DesktopTheme;
  selectedLocale: SupportedLocale;
  options: AppearanceOptionView[];
};

export type ReactionsPanelView = {
  status: SettingsPanelStatus;
  summaryLabel: string;
  panelError?: string | null;
  ownedAssets: CustomReactionAssetView[];
  bookmarkedAssets: BookmarkedCustomReactionView[];
};
