import { render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

// Web の build（ADR 0060 §6、#1220 AC-6）: 更新を確かめず、DHT を使わず、配信元から取得する。
vi.mock('@/lib/webRuntime', () => ({
  IS_WEB_RUNTIME: true,
  invokeWebRuntime: vi.fn(),
  listenWebRuntimeEvents: () => () => undefined,
}));
vi.mock('@/lib/api/invoke/desktop', () => ({ invokeDesktop: vi.fn().mockResolvedValue('available') }));

import i18n from '@/i18n';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { ReleasePanel } from './ReleasePanel';

afterEach(() => i18n.changeLanguage('en'));

test.each(['en', 'ja', 'zh-CN'])('the web client describes the transmissions of the web build in %s', async (locale) => {
  await i18n.changeLanguage(locale);
  render(
    <DesktopShellStoreContext.Provider value={createDesktopShellStore()}>
      <ReleasePanel showDiagnostics={false} />
    </DesktopShellStoreContext.Provider>
  );
  const shown = (key: string) => screen.queryByText(i18n.t(`settings:release.${key}`));
  expect(shown('webSummary')).toBeInTheDocument();
  expect(shown('externalTransmission.webOriginDestination')).toBeInTheDocument();
  expect(shown('externalTransmission.webP2pDestination')).toBeInTheDocument();
  expect(shown('externalTransmission.webP2pRetention')).toBeInTheDocument();
  for (const native of ['summary', 'externalTransmission.updateDestination', 'externalTransmission.p2pDestination', 'externalTransmission.p2pRetention']) {
    expect(shown(native)).not.toBeInTheDocument();
  }
});
