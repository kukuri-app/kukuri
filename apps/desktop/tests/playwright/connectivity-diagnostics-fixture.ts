import type { Page } from '@playwright/test';

export async function seedConnectivityDiagnostics(page: Page, locale = 'ja', theme = 'dark', developer = true, initialFailure = false) {
  await page.addInitScript(({ locale, theme, developer, initialFailure }) => {
    localStorage.setItem('kukuri.desktop.locale', locale);
    localStorage.setItem('kukuri.desktop.theme', theme);
    localStorage.setItem('kukuri.desktop.developer-mode', String(developer));
    const control = { fail: initialFailure, delay: 0, live: false, reads: 0, mutations: [] as string[] };
    Object.assign(window, { diagnosticsTest: control });
    let desktopApi = window.__KUKURI_DESKTOP__;
    Object.defineProperty(window, '__KUKURI_DESKTOP__', {
      configurable: true, get: () => desktopApi,
      set: (api: typeof desktopApi) => {
        desktopApi = api;
        if (!api) return;
        // No configured CN: do not trigger unrelated consent or introduction flows.
        void api.setCommunityNodeConfig([]);
        const get = api.getSyncStatus.bind(api);
        api.getSyncStatus = async () => {
          control.reads++;
          if (control.delay) await new Promise(resolve => setTimeout(resolve, control.delay));
          if (control.fail) throw new Error('fixture status read unavailable');
          const s = await get();
          const error = 'topic join pending: timed out waiting for initial topic join';
          return { ...s, connected: control.live, peer_count: control.live ? 1 : 0,
            delivery_state: control.live ? 'Live' : 'DurableRecovering', last_error: error, status_detail: error,
            configured_peer_count: 1, subscribed_topics: ['kukuri:topic:general'], gossip_disabled_topics: ['kukuri:topic:test'],
            topic_diagnostics: [{ ...s.topic_diagnostics[0], topic: 'kukuri:topic:general', joined: control.live,
              peer_count: control.live ? 1 : 0,
              configured_peer_count: 1, missing_peer_count: control.live ? 0 : 1,
              delivery_state: control.live ? 'Live' : 'DurableRecovering',
              active_path: 'direct_p2p', last_error: error, status_detail: error }],
            discovery: { ...s.discovery, connected_peer_count: 0, docs_assist_peer_count: 1, last_discovery_error: error },
          };
        };
        // 詳細の peer の一覧は、開いたときにページで読む(#1221 R2-D)。
        api.listConnectivityPeers = async ({ kind }) => ({
          peer_ids: kind === 'docs_assist' ? ['assist-peer']
            : kind === 'connected' ? (control.live ? ['peer-a'] : [])
            : kind === 'configured' || (kind === 'missing' && !control.live) ? ['candidate-peer'] : [],
          next_cursor: null,
        });
        for (const name of ['authenticateCommunityNode', 'acceptCommunityNodeConsents', 'setDiscoverySeeds', 'importPeerTicket', 'setTopicGossipEnabled'] as const) {
          // Diagnostics and settings navigation must never invoke mutation APIs.
          const original = api[name] as (...args: unknown[]) => unknown;
          Object.assign(api, { [name]: (...args: unknown[]) => { control.mutations.push(name); return original.apply(api, args); } });
        }
      },
    });
  }, { locale, theme, developer, initialFailure });
}
