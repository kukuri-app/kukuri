import { create } from 'zustand';
import { isChangingAccount } from '@/lib/accountSession';
import { suspendColumnDraftPersistence } from '@/shell/columnDraftPersistence';

export type PwaShellState = {
  ready: boolean;
  available: boolean;
  checking: boolean;
  applying: boolean;
  error: 'prepare' | 'apply' | null;
  checkedAt: number | null;
};
export const usePwaShellStore = create<PwaShellState>(() => ({
  ready: false, available: false, checking: false, applying: false, error: null, checkedAt: null,
}));
let registration: ServiceWorkerRegistration | null = null;
let connect = async () => {};
const version = () => document.querySelector<HTMLMetaElement>('meta[name="kukuri-shell-build"]')?.content;

async function request(worker: ServiceWorker, type: string): Promise<string> {
  const channel = new MessageChannel();
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { channel.port1.close(); reject(new Error('Shell worker did not respond')); }, 5000);
    channel.port1.onmessage = event => { clearTimeout(timeout); channel.port1.close(); resolve(event.data); };
    worker.postMessage({ type, version: version() }, [channel.port2]);
  });
}

async function refresh() {
  if (!registration?.active) return;
  await request(registration.active, 'hello');
  const current = await request(registration.active, 'version');
  usePwaShellStore.setState({ ready: true, available: Boolean(registration.waiting) || current !== version() });
}

export function startPwaShell() {
  if (!('serviceWorker' in navigator) || !version()) return () => undefined;
  let disposed = false;
  let stopCandidate: () => void = () => undefined;
  const safeRefresh = () => { void refresh().catch(() => {
    if (!disposed) usePwaShellStore.setState({ error: 'prepare' });
  }); };
  const watch = () => {
    stopCandidate();
    const candidate = registration?.installing;
    if (!candidate) return;
    const changed = () => {
      if (candidate.state === 'installed') { stopCandidate(); safeRefresh(); }
      if (candidate.state === 'redundant') { stopCandidate(); usePwaShellStore.setState({ error: 'prepare', checking: false }); safeRefresh(); }
    };
    candidate.addEventListener('statechange', changed);
    stopCandidate = () => { candidate.removeEventListener('statechange', changed); };
    changed();
  };
  const online = () => { void checkPwaShell(); };
  const visible = () => { if (document.visibilityState === 'visible') online(); };
  navigator.serviceWorker.addEventListener('controllerchange', safeRefresh);
  window.addEventListener('online', online);
  document.addEventListener('visibilitychange', visible);
  connect = async () => {
    const value = await navigator.serviceWorker.getRegistration() ?? await navigator.serviceWorker.register('/sw.js', { updateViaCache: 'none' });
    if (disposed) return;
    registration = value;
    registration.addEventListener('updatefound', watch);
    watch();
    await navigator.serviceWorker.ready;
    if (!disposed) { safeRefresh(); if (navigator.onLine) online(); }
  };
  void connect().catch(() => { if (!disposed) usePwaShellStore.setState({ error: 'prepare' }); });
  return () => {
    disposed = true; stopCandidate();
    registration?.removeEventListener('updatefound', watch);
    navigator.serviceWorker.removeEventListener('controllerchange', safeRefresh);
    window.removeEventListener('online', online);
    document.removeEventListener('visibilitychange', visible);
    registration = null;
  };
}

export async function checkPwaShell() {
  if (usePwaShellStore.getState().checking || usePwaShellStore.getState().applying) return;
  usePwaShellStore.setState({ checking: true, error: null });
  try {
    if (!registration) await connect();
    await registration!.update();
    await refresh();
  } catch { usePwaShellStore.setState({ error: 'prepare' }); }
  finally { usePwaShellStore.setState({ checking: false, checkedAt: Date.now() }); }
}

export async function applyPwaShell() {
  if (!registration || !usePwaShellStore.getState().available || usePwaShellStore.getState().applying) return;
  usePwaShellStore.setState({ applying: true, error: null });
  try {
    if (isChangingAccount()) throw new Error('Account transition is active');
    suspendColumnDraftPersistence(localStorage)();
    await request(registration.active!, 'hello');
    if (registration.waiting) {
      await new Promise<void>((resolve, reject) => {
        const changed = () => { clearTimeout(timeout); resolve(); };
        const timeout = setTimeout(() => {
          navigator.serviceWorker.removeEventListener('controllerchange', changed);
          reject(new Error('Shell update did not activate'));
        }, 10_000);
        navigator.serviceWorker.addEventListener('controllerchange', changed, { once: true });
        registration!.waiting!.postMessage({ type: 'apply' });
      });
    }
    if (isChangingAccount()) throw new Error('Account transition is active');
    suspendColumnDraftPersistence(localStorage)();
    location.reload();
  } catch { usePwaShellStore.setState({ error: 'apply' }); }
  finally { usePwaShellStore.setState({ applying: false }); }
}
