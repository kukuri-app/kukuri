import type { Page } from '@playwright/test';

// 実際のAppを起動する。productionのmock分岐は変更せず、試験内だけでIPCを置き換える。
export async function seedAppConsent(page: Page, {
  locale = 'en', theme = 'dark', attestedVersion = null, failOnce = false,
  systemLocales = ['en-US'],
}: {
  locale?: string | null; theme?: string; attestedVersion?: number | null; failOnce?: boolean;
  systemLocales?: string[];
} = {}) {
  await page.addInitScript(({ locale, theme, attestedVersion, failOnce, systemLocales }) => {
    // nullはfresh contextの言語をseedしない。reload時も実際の保存値を保持する。
    if (locale !== null) localStorage.setItem('kukuri.desktop.locale', locale);
    Object.defineProperty(window, 'isTauri', { configurable: true, value: true });
    localStorage.setItem('kukuri.desktop.theme', theme);
    let desktopApi = window.__KUKURI_DESKTOP__;
    let ready = false;
    let attempts = 0;
    const calls: { command: string; args?: Record<string, unknown> }[] = [];
    Object.defineProperty(window, '__appConsentCalls', { configurable: true, value: calls });
    Object.defineProperty(window, '__KUKURI_DESKTOP__', {
      configurable: true,
      get: () => ready ? desktopApi : undefined,
      set: (api: typeof desktopApi) => { desktopApi = api; },
    });
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      configurable: true,
      value: { invoke: async (command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args });
        if (command === 'get_system_locales') return systemLocales;
        if (command === 'get_desktop_startup_status') return {
          status: 'consent_required',
          documents: ['terms', 'privacy'].map((slug) => ({
            slug, currentVersion: 9, acceptedVersion: attestedVersion === null ? null : 8,
            acceptedAt: null, acceptedLanguage: null, acceptedAppVersion: null,
            effectiveDate: '2026-10-07', authoritativeLanguage: 'ja', materialChange: true,
            controllerName: 'Preview Distributor', contact: 'privacy@example.test',
          })),
          age_attestation: { currentVersion: 1, attestedVersion, attestedAt: null },
        };
        if (command === 'accept_app_consents') {
          attempts += 1;
          await new Promise((resolve) => setTimeout(resolve, 250));
          if (failOnce && attempts === 1) throw new Error('Consent fixture: storage unavailable');
          ready = true;
          return { status: 'ready' };
        }
        if (command === 'get_pending_device_restore_frontend_state') return null;
        throw new Error(`Unexpected consent fixture IPC: ${command}`);
      } },
    });
  }, { locale, theme, attestedVersion, failOnce, systemLocales });
}

export async function appConsentCalls(page: Page) {
  return page.evaluate(() => (window as unknown as {
    __appConsentCalls: { command: string; args?: Record<string, unknown> }[];
  }).__appConsentCalls.filter((call) => call.command === 'accept_app_consents'));
}
