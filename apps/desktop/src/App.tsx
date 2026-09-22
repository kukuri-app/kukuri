import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { HashRouter } from 'react-router-dom';

import { ConsentGateView } from '@/components/ConsentGateView';
import { WindowClosePrompt } from '@/components/WindowClosePrompt';
import { normalizeSupportedLocale } from '@/i18n';
import { changeDesktopLocale } from '@/i18n/changeLocale';
import { Button } from '@/components/ui/button';
import { Notice } from '@/components/ui/notice';
import { DesktopShellPage } from '@/shell/DesktopShellPage';
import { reconcileAccountDrafts } from '@/lib/accountSession';
import {
  type AppProps,
  DesktopShellStoreContext,
  createDesktopShellStore,
} from '@/shell/store';
import {
  type AgeAttestationStatus,
  type AppConsentDocumentStatus,
  type DesktopStartupErrorView,
  type DesktopStartupStatus,
  acceptAppConsents,
  applyPendingDeviceRestoreFrontendState,
  getDesktopStartupStatus,
} from '@/lib/api';
import { isBridgeUnavailableError } from '@/lib/api/invoke/error';
import {
  type DesktopTheme,
  readDesktopTheme,
  writeDesktopTheme,
} from '@/lib/theme';
import { copyTextToClipboard } from '@/lib/utils';
import {
  WORKSPACE_LAYOUT_STORAGE_KEY,
  startWorkspaceLayoutPersistence,
} from '@/shell/workspacePersistence';
import {
  initialHashForRestoredWorkspace,
  isDefaultStartupHash,
} from '@/shell/routing/initialWorkspaceRoute';
import { startColumnDraftPersistence } from '@/shell/columnDraftPersistence';
import { startCommunityIndexNodePreferencePersistence } from '@/shell/communityIndexNodePreference';

type StartupGateState = { status: 'checking' } | DesktopStartupStatus;

export function App(props: AppProps) {
  const { i18n } = useTranslation();
  const locale = normalizeSupportedLocale(i18n.resolvedLanguage ?? i18n.language);
  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);
  const [store] = useState(() => {
    const createdStore = createDesktopShellStore({
      workspaceStorage: window.localStorage,
      draftStorage: window.localStorage,
      communityIndexPreferenceStorage: window.localStorage,
    });
    // Issue #765 T4: hash の無い cold start では、復元した active Column の canonical target を
    // 初期 route として仕込み、既存の deep link 機構に focus 復元を委ねる。
    // 明示的な deep link(hash あり)と、保存 layout が無い初回起動では何もしない。
    if (
      isDefaultStartupHash(window.location.hash) &&
      window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY) !== null
    ) {
      const restoredHash = initialHashForRestoredWorkspace(
        createdStore.getState().workspaceState
      );
      if (restoredHash) {
        window.history.replaceState(null, '', restoredHash);
      }
    }
    return createdStore;
  });
  const [theme, setTheme] = useState<DesktopTheme>(() => readDesktopTheme());
  const [startupGate, setStartupGate] = useState<StartupGateState>(() =>
    props.api ? { status: 'ready' } : { status: 'checking' }
  );

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    if (startupGate.status === 'ready') writeDesktopTheme(theme);
  }, [startupGate.status, theme]);

  useEffect(() => {
    if (startupGate.status !== 'ready') return;
    return startWorkspaceLayoutPersistence(store, window.localStorage);
  }, [startupGate.status, store]);

  useEffect(() => {
    if (startupGate.status !== 'ready') return;
    return startColumnDraftPersistence(store, window.localStorage);
  }, [startupGate.status, store]);

  useEffect(() => {
    if (startupGate.status !== 'ready') return;
    return startCommunityIndexNodePreferencePersistence(store, window.localStorage);
  }, [startupGate.status, store]);

  useEffect(() => {
    if (props.api) {
      setStartupGate({ status: 'ready' });
      return;
    }

    let active = true;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    const loadStartupStatus = () => {
      getDesktopStartupStatus()
        .then(async (status: DesktopStartupStatus) => {
          if (!active) {
            return;
          }
          if (status.status === 'ready') {
            if (await reconcileAccountDrafts()) { window.location.reload(); return; }
            const applied = await applyPendingDeviceRestoreFrontendState();
            if (!active) return;
            if (applied) {
              window.location.reload();
              return;
            }
          }
          setStartupGate(status);
          if (status.status === 'initializing') {
            retryTimer = setTimeout(loadStartupStatus, 100);
          }
        })
        .catch((error: unknown) => {
          if (!active) {
            return;
          }
          if (isBridgeUnavailableError(error)) {
            // Tauri ブリッジ不在(ブラウザ/mock モード)— 文言非依存の code 判定(WP-C3)。
            setStartupGate({ status: 'ready' });
            return;
          }
          setStartupGate({
            status: 'failed',
            error: {
              kind: 'unknown',
              message: 'kukuri could not finish desktop startup.',
              detail: error instanceof Error ? error.message : String(error),
              db_path: null,
            },
          });
        });
    };
    loadStartupStatus();

    return () => {
      active = false;
      if (retryTimer !== null) {
        clearTimeout(retryTimer);
      }
    };
  }, [props.api]);

  if (startupGate.status === 'checking' || startupGate.status === 'initializing') {
    return (
      <>
        <StartupStatusScreen status='checking' />
        <WindowClosePrompt />
      </>
    );
  }

  if (startupGate.status === 'consent_required') {
    return <>
      <ConsentGate
          documents={startupGate.documents}
          ageAttestation={startupGate.age_attestation}
          onAccepted={setStartupGate}
        />
      <WindowClosePrompt />
    </>;
  }

  if (startupGate.status === 'failed') {
    return (
      <>
        <StartupStatusScreen status='failed' error={startupGate.error} />
        <WindowClosePrompt />
      </>
    );
  }

  return (
    <DesktopShellStoreContext.Provider value={store}>
      <HashRouter>
        <DesktopShellPage {...props} theme={theme} onThemeChange={setTheme} />
      </HashRouter>
      <WindowClosePrompt />
    </DesktopShellStoreContext.Provider>
  );
}

function ConsentGate({
  documents,
  ageAttestation,
  onAccepted,
}: {
  documents: AppConsentDocumentStatus[];
  ageAttestation: AgeAttestationStatus;
  onAccepted: (status: DesktopStartupStatus) => void;
}) {
  const { i18n } = useTranslation('legal');
  const [accepting, setAccepting] = useState(false);
  const acceptInFlight = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [declined, setDeclined] = useState(false);
  const [ageAttested, setAgeAttested] = useState(false);
  const [localeSaveFailed, setLocaleSaveFailed] = useState(false);
  // #857: 文書単位判定 — どれか 1 つでも旧版で同意済みなら「更新」通知を出す。
  const updated = documents.some(
    (document) =>
      document.acceptedVersion !== null && document.acceptedVersion < document.currentVersion
  );
  // #858: 現行版で申告済みなら(文書更新の再同意時)チェックを再要求しない。
  const attestationRequired =
    ageAttestation.attestedVersion === null ||
    ageAttestation.attestedVersion < ageAttestation.currentVersion;

  async function handleAccept() {
    if (acceptInFlight.current || (attestationRequired && !ageAttested)) return;
    acceptInFlight.current = true;
    setAccepting(true);
    setError(null);
    try {
      const nextStatus = await acceptAppConsents(
        documents.map((document) => ({
          slug: document.slug,
          version: document.currentVersion,
        })),
        i18n.resolvedLanguage ?? i18n.language,
        attestationRequired && ageAttested
      );
      if (
        nextStatus.status === 'ready' &&
        (await applyPendingDeviceRestoreFrontendState())
      ) {
        window.location.reload();
        return;
      }
      onAccepted(nextStatus);
    } catch (acceptError) {
      setError(acceptError instanceof Error ? acceptError.message : String(acceptError));
    } finally {
      acceptInFlight.current = false;
      setAccepting(false);
    }
  }

  return (
    <ConsentGateView
      documents={documents}
      updated={updated}
      attestationRequired={attestationRequired}
      ageAttested={ageAttested}
      accepting={accepting}
      error={error}
      declined={declined}
      locale={normalizeSupportedLocale(i18n.resolvedLanguage ?? i18n.language)}
      localeSaveFailed={localeSaveFailed}
      onLocaleChange={(locale) => {
        if (acceptInFlight.current) return;
        setLocaleSaveFailed(!changeDesktopLocale(locale));
      }}
      onAgeAttestedChange={setAgeAttested}
      onAccept={() => void handleAccept()}
      onDecline={() => setDeclined(true)}
    />
  );
}

function StartupStatusScreen({
  status,
  error,
}: {
  status: 'checking' | 'failed';
  error?: DesktopStartupErrorView;
}) {
  const { t } = useTranslation(['common']);
  const detail = error
    ? [
        `kind: ${error.kind}`,
        `db_path: ${error.db_path ?? 'unknown'}`,
        '',
        error.detail,
      ].join('\n')
    : '';

  return (
    <main className='startup-error-screen'>
      <section className='startup-error-panel' aria-live='polite'>
        {status === 'checking' ? (
          <Notice>{t('startup.checking')}</Notice>
        ) : (
          <>
            <Notice tone='destructive'>
              <strong>{t('startup.title')}</strong>
              <span>{t('startup.description')}</span>
            </Notice>
            <div className='startup-error-actions'>
              <Button type='button' onClick={() => window.location.reload()}>
                {t('actions.retry')}
              </Button>
              <Button
                type='button'
                variant='secondary'
                onClick={() => void copyTextToClipboard(detail)}
              >
                {t('startup.copyDetails')}
              </Button>
            </div>
            <dl className='startup-error-summary'>
              <div>
                <dt>{t('startup.kind')}</dt>
                <dd>{t(`startup.kinds.${error?.kind ?? 'unknown'}`)}</dd>
              </div>
              <div>
                <dt>{t('startup.dbPath')}</dt>
                <dd>{error?.db_path ?? t('fallbacks.unknown')}</dd>
              </div>
            </dl>
            <textarea
              className='startup-error-detail'
              value={detail}
              readOnly
              aria-label={t('startup.detailLabel')}
            />
          </>
        )}
      </section>
    </main>
  );
}
