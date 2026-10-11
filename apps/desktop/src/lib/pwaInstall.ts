import { create } from 'zustand';

type InstallPrompt = Event & {
  prompt(): Promise<void>;
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>;
};
export type PwaInstallPhase = 'unavailable' | 'available' | 'prompting' | 'installed' | 'failed';
export const usePwaInstallStore = create<{ phase: PwaInstallPhase }>(() => ({ phase: 'unavailable' }));
let prompt: InstallPrompt | null = null;

// mainのWeb入口で登録する。設定を開く前に届くChromeのイベントも受ける。
export function startPwaInstall() {
  const display = window.matchMedia('(display-mode: standalone)');
  if (display.matches) usePwaInstallStore.setState({ phase: 'installed' });
  const beforeInstall = (event: Event) => {
    event.preventDefault();
    if (usePwaInstallStore.getState().phase === 'installed') return;
    prompt = event as InstallPrompt;
    usePwaInstallStore.setState({ phase: 'available' });
  };
  const installed = () => {
    prompt = null;
    usePwaInstallStore.setState({ phase: 'installed' });
  };
  const displayChanged = () => { if (display.matches) installed(); };
  window.addEventListener('beforeinstallprompt', beforeInstall);
  window.addEventListener('appinstalled', installed);
  display.addEventListener('change', displayChanged);
  return () => {
    window.removeEventListener('beforeinstallprompt', beforeInstall);
    window.removeEventListener('appinstalled', installed);
    display.removeEventListener('change', displayChanged);
    prompt = null;
  };
}

export async function installPwa() {
  const request = prompt;
  if (!request || usePwaInstallStore.getState().phase !== 'available') return;
  prompt = null; // promptは1回だけ使える。取消後はブラウザの新しいイベントを待つ。
  usePwaInstallStore.setState({ phase: 'prompting' });
  try {
    await request.prompt(); // clickのuser activationの中で呼ぶ。
    await request.userChoice;
    if (usePwaInstallStore.getState().phase !== 'installed') {
      usePwaInstallStore.setState({ phase: 'unavailable' });
    }
  } catch {
    if (usePwaInstallStore.getState().phase !== 'installed') {
      usePwaInstallStore.setState({ phase: 'failed' });
    }
  }
}
