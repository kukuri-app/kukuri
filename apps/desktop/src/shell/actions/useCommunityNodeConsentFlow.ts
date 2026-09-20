import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { CommunityNodeConsentDocumentRef, CommunityNodeNodeStatus, DesktopApi } from '@/lib/api';
import { communityNodeConsentView } from '@/shell/presentation';
import type { CommunityNodePoliciesEntry } from '@/shell/store';

export type AcceptCommunityNodeConsents = (
  baseUrl: string, documents: CommunityNodeConsentDocumentRef[], language: string
) => Promise<unknown>;

// UIの表示内容を受諾まで固定する。API副作用は既存shell actionへ委譲できる。
export function useCommunityNodeConsentFlow({
  api, configuredBaseUrls, statuses = [], acceptConsents, onAccepted, onDismiss,
}: {
  api: DesktopApi;
  configuredBaseUrls: readonly string[];
  statuses?: readonly CommunityNodeNodeStatus[];
  acceptConsents?: AcceptCommunityNodeConsents;
  onAccepted?: () => void;
  onDismiss?: () => void;
}) {
  const { i18n, t } = useTranslation(['settings', 'common']);
  const language = i18n.resolvedLanguage ?? i18n.language;
  const [baseUrl, setBaseUrl] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<{
    baseUrl: string; language: string; entry: CommunityNodePoliciesEntry;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const accepting = useRef(false);
  const generation = useRef(0);
  const returnFocus = useRef<HTMLElement | null>(null);
  const configured = baseUrl !== null && configuredBaseUrls.includes(baseUrl);

  useEffect(() => {
    if (!baseUrl || !configured) return;
    let active = true;
    const id = ++generation.current;
    setCatalog({ baseUrl, language, entry: { status: 'loading' } });
    setError(null);
    void api.fetchCommunityNodePolicies(baseUrl, language).then((response) => {
      if (active && id === generation.current) setCatalog({
        baseUrl, language, entry: { status: 'ok', policies: response.policies },
      });
    }).catch((cause: unknown) => {
      if (active && id === generation.current) setCatalog({
        baseUrl, language, entry: { status: 'error', error: cause instanceof Error ? cause.message : String(cause) },
      });
    });
    return () => { active = false; };
  }, [api, attempt, baseUrl, configured, language]);

  const close = useCallback(() => {
    if (accepting.current) return;
    generation.current += 1;
    setBaseUrl(null);
    setCatalog(null);
    setError(null);
  }, []);

  useEffect(() => {
    if (baseUrl && !configured) close();
  }, [baseUrl, busy, close, configured]);

  const open = useCallback((target: string, focus?: HTMLElement | null) => {
    if (accepting.current || !configuredBaseUrls.includes(target)) return;
    returnFocus.current = focus === undefined
      ? document.activeElement instanceof HTMLElement ? document.activeElement : null
      : focus;
    generation.current += 1;
    setCatalog(null);
    setError(null);
    setBaseUrl(target);
    setAttempt((value) => value + 1);
  }, [configuredBaseUrls]);

  const entry = catalog?.baseUrl === baseUrl && catalog.language === language
    ? catalog.entry : undefined;
  const view = communityNodeConsentView(
    statuses.find((status) => status.base_url === baseUrl), entry
  );
  async function accept() {
    if (!baseUrl || !configured || entry?.status !== 'ok' || accepting.current) return;
    // #1192: 受諾するのは「表示した文書」だけ。一覧から外した文書(観測提供・権利侵害
    // 申出ポリシー)を同意記録にしない。
    const policies = view.policies;
    if (!policies.length) return;
    const target = baseUrl;
    const id = generation.current;
    accepting.current = true;
    setBusy(true);
    setError(null);
    try {
      // ローカル設定を再確認し、表示後に削除されたNodeを受諾しない。
      const config = await api.getCommunityNodeConfig();
      if (id !== generation.current) return;
      if (!config.nodes.some((node) => node.base_url === target)) {
        setBaseUrl(null);
        setCatalog(null);
        return;
      }
      const documents = policies.map((policy) => ({
        policy_slug: policy.policySlug, policy_version: policy.policyVersion,
        policy_snapshot_revision: policy.policySnapshotRevision ?? null,
      }));
      await (acceptConsents ?? api.acceptCommunityNodeConsents.bind(api))(target, documents, language);
      if (id === generation.current) {
        setBaseUrl(null);
        setCatalog(null);
        onAccepted?.();
      }
    } catch {
      if (id === generation.current) setError(t('settings:communityNode.consent.acceptFailed'));
    } finally {
      accepting.current = false;
      setBusy(false);
    }
  }

  return {
    open, close,
    dialog: baseUrl && configured ? {
      open: true,
      baseUrl,
      consent: view,
      busy,
      error,
      onOpenChange: (nextOpen: boolean) => { if (!nextOpen && !accepting.current) { close(); onDismiss?.(); } },
      onAccept: () => { void accept(); },
      onRetry: () => { if (!accepting.current) setAttempt((value) => value + 1); },
      onCloseAutoFocus: (event: Event) => {
        event.preventDefault();
        if (returnFocus.current?.isConnected) returnFocus.current.focus();
      },
    } : null,
  };
}
