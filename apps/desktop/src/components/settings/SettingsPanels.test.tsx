import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import i18n from '@/i18n';
import type { DesktopLogsView } from '@/lib/desktopLogs';
import { developerLogFixtureSnapshot } from '@/mocks/api/developerLogs';

import { AppearancePanel } from './AppearancePanel';
import { CommunityNodePanel } from './CommunityNodePanel';
import { ConnectivityPanel } from './ConnectivityPanel';
import { DeveloperLogViewer } from './DeveloperLogViewer';
import { DeveloperPanel } from './DeveloperPanel';
import { DiscoveryPanel } from './DiscoveryPanel';
import { ReactionsPanel } from './ReactionsPanel';
import { SafetyPanel } from './SafetyPanel';
import {
  createAppearancePanelFixture,
  createCommunityNodePanelFixture,
  createConnectivityPanelFixture,
  createDiscoveryPanelFixture,
} from './fixtures';
import { SettingsActionRow } from './SettingsActionRow';
import { SettingsDiagnosticList } from './SettingsDiagnosticList';
import { SettingsMetricGrid } from './SettingsMetricGrid';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function installCropperMocks() {
  vi.spyOn(URL, 'createObjectURL').mockImplementation(() => 'blob:crop-preview');
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
  vi.stubGlobal(
    'Image',
    class {
      naturalWidth = 320;
      naturalHeight = 240;
      onload: null | (() => void) = null;
      onerror: null | (() => void) = null;

      set src(_value: string) {
        queueMicrotask(() => {
          this.onload?.();
        });
      }
    }
  );
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
    drawImage: vi.fn(),
  } as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation((callback) => {
    callback(new Blob([Uint8Array.from([1, 2, 3, 4])], { type: 'image/png' }));
  });
}

test('appearance panel switches the selected theme immediately', async () => {
  const user = userEvent.setup();
  const onThemeChange = vi.fn();
  const appearancePanelFixture = createAppearancePanelFixture();

  render(
    <AppearancePanel
      view={appearancePanelFixture}
      onThemeChange={onThemeChange}
      onLocaleChange={() => {}}
    />
  );

  await user.click(screen.getByRole('radio', { name: /Light/i }));
  expect(onThemeChange).toHaveBeenCalledWith('light');
  expect(screen.getByRole('radio', { name: /Dark/i })).toHaveAttribute('aria-checked', 'true');
});

test('appearance panel removes redundant explanatory copy and duplicate section titles', () => {
  const appearancePanelFixture = createAppearancePanelFixture();

  render(
    <AppearancePanel
      view={appearancePanelFixture}
      onThemeChange={() => {}}
      onLocaleChange={() => {}}
    />
  );

  expect(screen.queryByRole('heading', { name: 'Appearance' })).not.toBeInTheDocument();
  expect(screen.queryByText('dark theme selected')).not.toBeInTheDocument();
  expect(
    screen.queryByText('Theme changes apply immediately on this device and stay local to this desktop.')
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText(
      'Language changes apply immediately on this device and stay local to this desktop.'
    )
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText('High-contrast solid surfaces for low-light work.')
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText('Brighter solid surfaces for daytime readability.')
  ).not.toBeInTheDocument();
  expect(screen.getByRole('radiogroup', { name: 'Theme mode' })).toBeInTheDocument();
  expect(screen.getByLabelText('Language')).toBeInTheDocument();
});

test('connectivity panel renders loading and topic detail states', async () => {
  const user = userEvent.setup();
  const onImportPeer = vi.fn();
  const connectivityPanelFixture = createConnectivityPanelFixture();
  const loadPeers = vi.fn(async (_query: unknown, cursor: string | null) =>
    cursor ? { peer_ids: ['peer-c'], next_cursor: null } : { peer_ids: ['peer-a', 'peer-b'], next_cursor: 'peer-b' });

  render(
    <ConnectivityPanel
      view={{
        ...connectivityPanelFixture,
        status: 'loading',
        summaryLabel: 'loading',
      }}
      onPeerTicketInputChange={() => {}}
      onImportPeer={onImportPeer}
      loadPeers={loadPeers}
    />
  );

  expect(screen.getByText('Loading connectivity diagnostics…')).toBeInTheDocument();
  expect(screen.getByText('Topic Connectivity Detail')).toBeInTheDocument();
  // #1221 R2-D: peer の一覧は、詳細を開くまで読まない。開いたら 1 ページ目を読み、続きは「さらに読み込む」で読む。
  expect(loadPeers).not.toHaveBeenCalled();
  await user.click(screen.getAllByText('Technical diagnostic details')[2]);
  expect(await screen.findByText('The initial topic join timed out. (Diagnostic details: topic join pending: timed out waiting for initial topic join)')).toBeInTheDocument();
  await waitFor(() => expect(loadPeers).toHaveBeenCalledTimes(4));
  expect(loadPeers).toHaveBeenCalledWith({ kind: 'connected', topic: 'kukuri:topic:relay' }, null);
  expect(loadPeers).toHaveBeenCalledWith({ kind: 'missing', topic: 'kukuri:topic:relay' }, null);
  const loadMore = await screen.findAllByRole('button', { name: 'Load more' });
  expect(loadMore).toHaveLength(4);
  await user.click(loadMore[0]);
  expect(loadPeers).toHaveBeenLastCalledWith({ kind: 'connected', topic: 'kukuri:topic:relay' }, 'peer-b');
  expect(await screen.findByText('peer-a, peer-b, peer-c')).toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: 'Load more' })).toHaveLength(3);

  await user.click(screen.getByRole('button', { name: 'Import Peer' }));
  expect(onImportPeer).toHaveBeenCalledTimes(1);
});

test('discovery panel keeps env-locked seed editor read-only', () => {
  const discoveryPanelFixture = createDiscoveryPanelFixture();
  render(
    <DiscoveryPanel
      view={{
        ...discoveryPanelFixture,
        envLocked: true,
        seedPeersMessage: 'Environment overrides discovery seeds; editing is disabled.',
      }}
      saveDisabled
      resetDisabled
      onSeedPeersChange={() => {}}
      onSave={() => {}}
      onReset={() => {}}
      onPublicBlobDiscoveryChange={async () => {}}
    />
  );

  expect(screen.getByLabelText('Seed Peers')).toHaveAttribute('readonly');
  expect(
    screen.getByText('Environment overrides discovery seeds; editing is disabled.')
  ).toBeInTheDocument();
});

test('community node panel renders ready and error states', async () => {
  const user = userEvent.setup();
  const onFetchConsents = vi.fn();
  const onAcceptConsents = vi.fn();
  const communityNodePanelFixture = createCommunityNodePanelFixture();

  render(
    <CommunityNodePanel
      view={{
        ...communityNodePanelFixture,
        panelError: 'failed to update community nodes',
      }}
      saveDisabled={false}
      resetDisabled={false}
      clearDisabled={false}
      onAddNode={() => {}}
      onNodeBaseUrlChange={() => {}}
      onRemoveNode={() => {}}
      onSaveNodes={() => {}}
      onReset={() => {}}
      onClearNodes={() => {}}
      onAuthenticate={() => {}}
      onSubmitInviteCode={async () => {}}
      onFetchConsents={onFetchConsents}
      onAcceptConsents={onAcceptConsents}
      onWithdrawConsents={() => {}}
      onRefresh={() => {}}
      onClearToken={() => {}}
    />
  );

  expect(screen.getByText('failed to update community nodes')).toBeInTheDocument();
  expect(screen.getByDisplayValue('https://api.kukuri.app')).toBeInTheDocument();

  await user.click(screen.getAllByRole('button', { name: 'Consents' })[0]);
  expect(onFetchConsents).toHaveBeenCalledWith('https://api.kukuri.app', 'en');

  // 既に全て同意済みのノードでは Accept が無効化され、誤受諾を防ぐ。
  const consentDialog = await screen.findByRole('dialog');
  expect(within(consentDialog).getByRole('button', { name: 'All accepted' })).toBeDisabled();
  expect(onAcceptConsents).not.toHaveBeenCalled();
});

test('community node consent dialog shows policy body, version, and update notice', async () => {
  const user = userEvent.setup();
  const onFetchConsents = vi.fn();
  const onAcceptConsents = vi.fn();
  const communityNodePanelFixture = createCommunityNodePanelFixture();
  const fixtureWithUpdate = {
    ...communityNodePanelFixture,
    nodes: communityNodePanelFixture.nodes.map((node, index) =>
      index === 0
        ? {
            ...node,
            consent: {
              loaded: true,
              loading: false,
              loadError: null,
              withdrawn: false,
              hasLocalConsent: true,
              allRequiredAccepted: false,
              hasPendingUpdate: true,
              policies: [
                {
                  policySlug: 'terms_of_service',
                  title: 'Terms of Service',
                  body: 'You must follow the community node terms of service.',
                  policyVersion: 2,
                  referenceTranslation: false,
                  fallback: false,
                  required: true,
                  acceptedAtLabel: null,
                  updated: true,
                  previouslyAcceptedVersion: 1,
                },
              ],
            },
          }
        : node
    ),
  };

  render(
    <CommunityNodePanel
      view={fixtureWithUpdate}
      saveDisabled={false}
      resetDisabled={false}
      clearDisabled={false}
      onAddNode={() => {}}
      onNodeBaseUrlChange={() => {}}
      onRemoveNode={() => {}}
      onSaveNodes={() => {}}
      onReset={() => {}}
      onClearNodes={() => {}}
      onAuthenticate={() => {}}
      onSubmitInviteCode={async () => {}}
      onFetchConsents={onFetchConsents}
      onAcceptConsents={onAcceptConsents}
      onWithdrawConsents={() => {}}
      onRefresh={() => {}}
      onClearToken={() => {}}
    />
  );

  await user.click(screen.getAllByRole('button', { name: 'Consents' })[0]);

  const consentDialog = await screen.findByRole('dialog');
  await user.click(within(consentDialog).getByRole('button', { name: 'Terms of Service' }));
  expect(
    within(consentDialog).getByText('You must follow the community node terms of service.')
  ).toBeInTheDocument();
  expect(within(consentDialog).getByText('v2')).toBeInTheDocument();
  expect(
    within(consentDialog).getByText('This node updated its policies. Review the changes and accept again to keep connecting.')
  ).toBeInTheDocument();
  expect(within(consentDialog).getByText('Updated from v1 to v2.')).toBeInTheDocument();

  await user.click(within(consentDialog).getByRole('button', { name: 'Accept' }));
  expect(onAcceptConsents).toHaveBeenCalledWith('https://api.kukuri.app', [
    {
      policy_slug: 'terms_of_service',
      policy_version: 2,
      policy_snapshot_revision: null,
    },
  ], 'en');
});

test('community node refresh opens consent dialog without protected refresh when consent is missing', async () => {
  const user = userEvent.setup();
  const onFetchConsents = vi.fn().mockResolvedValue(undefined);
  const onRefresh = vi.fn().mockResolvedValue(false);
  const communityNodePanelFixture = createCommunityNodePanelFixture();
  const view = {
    ...communityNodePanelFixture,
    nodes: communityNodePanelFixture.nodes.map((node, index) =>
      index === 0
        ? {
            ...node,
            consent: {
              ...node.consent,
              hasLocalConsent: false,
              allRequiredAccepted: false,
            },
          }
        : node
    ),
  };

  render(
    <CommunityNodePanel
      view={view}
      saveDisabled={false}
      resetDisabled={false}
      clearDisabled={false}
      onAddNode={() => {}}
      onNodeBaseUrlChange={() => {}}
      onRemoveNode={() => {}}
      onSaveNodes={() => {}}
      onReset={() => {}}
      onClearNodes={() => {}}
      onAuthenticate={() => {}}
      onSubmitInviteCode={async () => {}}
      onFetchConsents={onFetchConsents}
      onAcceptConsents={() => {}}
      onWithdrawConsents={() => {}}
      onRefresh={onRefresh}
      onClearToken={() => {}}
    />
  );

  await user.click(screen.getAllByRole('button', { name: 'Refresh' })[0]);

  expect(onRefresh).not.toHaveBeenCalled();
  expect(onFetchConsents).toHaveBeenCalledWith('https://api.kukuri.app', 'en');
  expect(await screen.findByRole('dialog')).toBeInTheDocument();
});

test('community node refresh opens consent dialog when runtime detects a policy update', async () => {
  const user = userEvent.setup();
  const onFetchConsents = vi.fn().mockResolvedValue(undefined);
  const onRefresh = vi.fn().mockResolvedValue(true);
  const communityNodePanelFixture = createCommunityNodePanelFixture();

  render(
    <CommunityNodePanel
      view={communityNodePanelFixture}
      saveDisabled={false}
      resetDisabled={false}
      clearDisabled={false}
      onAddNode={() => {}}
      onNodeBaseUrlChange={() => {}}
      onRemoveNode={() => {}}
      onSaveNodes={() => {}}
      onReset={() => {}}
      onClearNodes={() => {}}
      onAuthenticate={() => {}}
      onSubmitInviteCode={async () => {}}
      onFetchConsents={onFetchConsents}
      onAcceptConsents={() => {}}
      onWithdrawConsents={() => {}}
      onRefresh={onRefresh}
      onClearToken={() => {}}
    />
  );

  await user.click(screen.getAllByRole('button', { name: 'Refresh' })[0]);

  expect(onRefresh).toHaveBeenCalledWith('https://api.kukuri.app');
  expect(onFetchConsents).toHaveBeenCalledWith('https://api.kukuri.app', 'en');
  expect(await screen.findByRole('dialog')).toBeInTheDocument();
});

test('community node consent dialog disables accept when latest policy fetch fails', async () => {
  const user = userEvent.setup();
  const onFetchConsents = vi.fn().mockRejectedValue(new Error('offline'));
  const onAcceptConsents = vi.fn();
  const communityNodePanelFixture = createCommunityNodePanelFixture();
  const fixtureWithPendingConsent = {
    ...communityNodePanelFixture,
    nodes: communityNodePanelFixture.nodes.map((node, index) =>
      index === 0
        ? {
            ...node,
            consent: {
              ...node.consent,
              allRequiredAccepted: false,
            },
          }
        : node
    ),
  };

  render(
    <CommunityNodePanel
      view={fixtureWithPendingConsent}
      saveDisabled={false}
      resetDisabled={false}
      clearDisabled={false}
      onAddNode={() => {}}
      onNodeBaseUrlChange={() => {}}
      onRemoveNode={() => {}}
      onSaveNodes={() => {}}
      onReset={() => {}}
      onClearNodes={() => {}}
      onAuthenticate={() => {}}
      onSubmitInviteCode={async () => {}}
      onFetchConsents={onFetchConsents}
      onAcceptConsents={onAcceptConsents}
      onWithdrawConsents={() => {}}
      onRefresh={() => {}}
      onClearToken={() => {}}
    />
  );

  await user.click(screen.getAllByRole('button', { name: 'Consents' })[0]);

  const consentDialog = await screen.findByRole('dialog');
  // #857: 取得失敗(オフライン等)は失敗通知と再試行ボタンを出し、受諾は無効化する。
  expect(
    within(consentDialog).getByText(
      'Could not load the policies (you may be offline). Retry when the node is reachable.'
    )
  ).toBeInTheDocument();
  expect(within(consentDialog).getByRole('button', { name: 'Retry' })).toBeInTheDocument();
  expect(
    within(consentDialog).queryByText('You must follow the community node terms of service.')
  ).not.toBeInTheDocument();
  expect(within(consentDialog).getByRole('button', { name: 'Accept' })).toBeDisabled();
});

test('settings panels avoid the legacy grid classname collision', () => {
  const appearancePanelFixture = createAppearancePanelFixture();
  const connectivityPanelFixture = createConnectivityPanelFixture();
  const discoveryPanelFixture = createDiscoveryPanelFixture();
  const communityNodePanelFixture = createCommunityNodePanelFixture();

  const { container } = render(
    <div>
      <AppearancePanel
        view={appearancePanelFixture}
        onThemeChange={() => {}}
        onLocaleChange={() => {}}
      />
      <ConnectivityPanel
        view={connectivityPanelFixture}
        onPeerTicketInputChange={() => {}}
        onImportPeer={() => {}}
      />
      <DiscoveryPanel
        view={discoveryPanelFixture}
        saveDisabled={false}
        resetDisabled={false}
        onSeedPeersChange={() => {}}
        onSave={() => {}}
        onReset={() => {}}
        onPublicBlobDiscoveryChange={async () => {}}
      />
      <CommunityNodePanel
        view={communityNodePanelFixture}
        saveDisabled={false}
        resetDisabled={false}
        clearDisabled={false}
        onAddNode={() => {}}
        onNodeBaseUrlChange={() => {}}
        onRemoveNode={() => {}}
        onSaveNodes={() => {}}
        onReset={() => {}}
        onClearNodes={() => {}}
        onAuthenticate={() => {}}
        onSubmitInviteCode={async () => {}}
        onFetchConsents={() => {}}
        onAcceptConsents={() => {}}
        onWithdrawConsents={() => {}}
        onRefresh={() => {}}
        onClearToken={() => {}}
      />
      <SettingsMetricGrid items={connectivityPanelFixture.metrics} />
      <SettingsDiagnosticList items={discoveryPanelFixture.diagnostics} columns={2} />
      <SettingsActionRow>
        <button type='button'>Action</button>
      </SettingsActionRow>
    </div>
  );

  expect(container.querySelector('.grid')).toBeNull();
});

test('reactions panel renders icon-only saved assets and supports single or bulk clear', async () => {
  const user = userEvent.setup();
  const onRemoveBookmark = vi.fn().mockResolvedValue(undefined);
  const clipboardWriteText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });

  render(
    <ReactionsPanel
      view={{
        status: 'ready',
        summaryLabel: 'ready',
        ownedAssets: [
          {
            asset_id: 'asset-owned',
            owner_pubkey: 'a'.repeat(64),
            blob_hash: 'blob-owned',
            search_key: 'party-parrot',
            mime: 'image/png',
            bytes: 128,
            width: 128,
            height: 128,
          },
        ],
        bookmarkedAssets: [
          {
            asset_id: 'asset-saved',
            owner_pubkey: 'b'.repeat(64),
            blob_hash: 'blob-saved',
            search_key: 'saved-cat',
            mime: 'image/gif',
            bytes: 256,
            width: 128,
            height: 128,
          },
          {
            asset_id: 'asset-wave',
            owner_pubkey: 'c'.repeat(64),
            blob_hash: 'blob-wave',
            search_key: 'wave-dog',
            mime: 'image/png',
            bytes: 144,
            width: 128,
            height: 128,
          },
        ],
      }}
      creating={false}
      mediaObjectUrls={{
        'blob-owned': 'https://example.com/owned.png',
        'blob-saved': 'https://example.com/saved.gif',
        'blob-wave': 'https://example.com/wave.png',
      }}
      onCreateAsset={() => {}}
      onRemoveBookmark={onRemoveBookmark}
      onListSets={async () => []}
      onCreateSet={vi.fn()}
    />
  );

  expect(screen.getByText('My custom reactions')).toBeInTheDocument();
  expect(screen.getByText('party-parrot')).toBeInTheDocument();
  expect(screen.getByAltText('party-parrot')).toHaveAttribute(
    'src',
    'https://example.com/owned.png'
  );
  expect(screen.queryByText('asset-owned')).not.toBeInTheDocument();
  expect(screen.getByRole('img', { name: 'saved-cat' })).toHaveAttribute(
    'src',
    'https://example.com/saved.gif'
  );
  expect(screen.getByRole('img', { name: 'wave-dog' })).toHaveAttribute(
    'src',
    'https://example.com/wave.png'
  );
  expect(screen.queryByText('asset-saved')).not.toBeInTheDocument();
  expect(screen.queryByText('blob-saved')).not.toBeInTheDocument();
  expect(screen.queryByText('saved-cat')).not.toBeInTheDocument();

  fireEvent.contextMenu(screen.getByRole('img', { name: 'saved-cat' }));
  await user.click(screen.getByRole('menuitem', { name: 'Copy hash' }));
  expect(clipboardWriteText).toHaveBeenLastCalledWith('blob-saved');

  const savedTile = screen.getByRole('img', { name: 'saved-cat' }).closest('article');
  if (!(savedTile instanceof HTMLElement)) throw new Error('saved reaction tile not found');
  savedTile.focus();
  fireEvent.keyDown(savedTile, { key: 'F10', shiftKey: true });
  await user.click(screen.getByRole('menuitem', { name: 'Clear' }));
  expect(onRemoveBookmark).toHaveBeenCalledWith('asset-saved');

  onRemoveBookmark.mockClear();

  await user.click(screen.getByRole('checkbox', { name: 'Select all' }));
  await user.click(screen.getByRole('button', { name: 'Clear selected' }));
  await waitFor(() => {
    expect(onRemoveBookmark).toHaveBeenCalledTimes(2);
  });
  expect(onRemoveBookmark).toHaveBeenCalledWith('asset-saved');
  expect(onRemoveBookmark).toHaveBeenCalledWith('asset-wave');
});

test('connectivity panel shows the summary state only once in the sync status card', () => {
  const connectivityPanelFixture = createConnectivityPanelFixture();

  render(
    <ConnectivityPanel
      view={{
        ...connectivityPanelFixture,
        summaryLabel: 'waiting',
      }}
      onPeerTicketInputChange={() => {}}
      onImportPeer={() => {}}
    />
  );

  expect(screen.getAllByText('waiting')).toHaveLength(1);
});

test('reactions panel crops an uploaded image before creating a custom asset', async () => {
  installCropperMocks();
  const user = userEvent.setup();
  const onCreateAsset = vi.fn();

  render(
    <ReactionsPanel
      view={{
        status: 'ready',
        summaryLabel: 'ready',
        ownedAssets: [],
        bookmarkedAssets: [],
      }}
      creating={false}
      onCreateAsset={onCreateAsset}
      onRemoveBookmark={async () => {}}
      onListSets={async () => []}
      onCreateSet={vi.fn()}
    />
  );

  await user.upload(
    screen.getByLabelText('Upload image'),
    new File([Uint8Array.from([9, 8, 7, 6])], 'party.png', { type: 'image/png' })
  );

  const cropDialog = await screen.findByRole('dialog', { name: 'Crop reaction image' });
  expect(within(cropDialog).getByRole('slider')).toBeInTheDocument();

  await user.click(within(cropDialog).getByRole('button', { name: 'Save' }));

  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Crop reaction image' })).not.toBeInTheDocument();
  });

  await user.type(screen.getByLabelText('Search key'), 'party');
  await user.click(screen.getAllByRole('button', { name: 'Save' })[0]);

  expect(onCreateAsset).toHaveBeenCalledWith(
    expect.objectContaining({ name: 'party.png' }),
    expect.objectContaining({
      size: expect.any(Number),
      x: expect.any(Number),
      y: expect.any(Number),
    }),
    'party'
  );
});

test('reaction file selection and crop cancellation preserve the accepted draft and permit reselection', async () => {
  installCropperMocks();
  const user = userEvent.setup();
  const create = vi.fn();
  const props = {
    view: { status: 'ready' as const, summaryLabel: '', ownedAssets: [], bookmarkedAssets: [] },
    creating: false, onCreateAsset: create, onRemoveBookmark: async () => {},
    onListSets: async () => [], onCreateSet: vi.fn(),
  };
  const view = render(<ReactionsPanel {...props} />);
  const file = new File(['image'], '選択した画像.png', { type: 'image/png' });
  const input = screen.getByLabelText('Upload image');
  await user.upload(input, file);
  let crop = await screen.findByRole('dialog', { name: 'Crop reaction image' });
  await user.click(within(crop).getByRole('button', { name: 'Cancel' }));
  expect(screen.getByText('No files selected')).toBeVisible();
  await user.upload(input, file);
  crop = await screen.findByRole('dialog', { name: 'Crop reaction image' });
  await user.click(within(crop).getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.getByText(file.name)).toBeVisible();
  await user.type(screen.getByLabelText('Search key'), 'preserved');
  fireEvent(input, new Event('cancel'));
  expect(screen.getByText(file.name)).toBeVisible();
  await user.upload(input, new File(['new'], 'replacement.gif', { type: 'image/gif' }));
  crop = await screen.findByRole('dialog', { name: 'Crop reaction image' });
  await user.click(within(crop).getByRole('button', { name: 'Cancel' }));
  expect(screen.getByText(file.name)).toBeVisible();
  expect(screen.getByLabelText('Search key')).toHaveValue('preserved');
  expect(create).not.toHaveBeenCalled();
  await act(() => i18n.changeLanguage('ja'));
  expect(screen.getByText(file.name)).toBeVisible();
  await user.click(screen.getByRole('button', { name: '切り抜きを編集' }));
  const japaneseCrop = await screen.findByRole('dialog', { name: 'リアクション画像の切り抜き' });
  expect(within(japaneseCrop).getByText('ドラッグして位置を動かし、拡大率を調整して表示する正方形の範囲を選択します。')).toBeVisible();
  await user.click(within(japaneseCrop).getByRole('button', { name: 'キャンセル' }));
  expect(screen.getByLabelText('検索キーワード')).toHaveValue('preserved');
  view.rerender(<ReactionsPanel {...props} creating />);
  expect(screen.getByRole('button', { name: 'ファイルを選択' })).toBeDisabled();
  expect(input).toBeDisabled();
});

test('connectivity panel hides diagnostics but keeps ticket import when showDiagnostics is false', () => {
  const connectivityPanelFixture = createConnectivityPanelFixture();

  render(
    <ConnectivityPanel
      view={connectivityPanelFixture}
      onPeerTicketInputChange={() => {}}
      onImportPeer={() => {}}
      showDiagnostics={false}
    />
  );

  expect(screen.getByText('Your Ticket')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Import Peer' })).toBeInTheDocument();
  expect(screen.queryByText('Connected peers and assistance candidates')).not.toBeInTheDocument();
  expect(screen.queryByText('Connected Peers')).not.toBeInTheDocument();
  expect(screen.queryByText('peer-a, peer-b')).not.toBeInTheDocument();
});

test('discovery panel hides diagnostics but keeps seed editor when showDiagnostics is false', () => {
  const discoveryPanelFixture = createDiscoveryPanelFixture();

  render(
    <DiscoveryPanel
      view={discoveryPanelFixture}
      saveDisabled={false}
      resetDisabled={false}
      onSeedPeersChange={() => {}}
      onSave={() => {}}
      onReset={() => {}}
      onPublicBlobDiscoveryChange={async () => {}}
      showDiagnostics={false}
    />
  );

  expect(screen.getByLabelText('Seed Peers')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Save Seeds' })).toBeInTheDocument();
  expect(screen.queryByText('Local Endpoint ID')).not.toBeInTheDocument();
  expect(screen.queryByText('local-endpoint-a')).not.toBeInTheDocument();
});

test('community node panel hides diagnostics but keeps node editing when showDiagnostics is false', () => {
  const communityNodePanelFixture = createCommunityNodePanelFixture();

  render(
    <CommunityNodePanel
      view={communityNodePanelFixture}
      saveDisabled={false}
      resetDisabled={false}
      clearDisabled={false}
      onAddNode={() => {}}
      onNodeBaseUrlChange={() => {}}
      onRemoveNode={() => {}}
      onSaveNodes={() => {}}
      onReset={() => {}}
      onClearNodes={() => {}}
      onAuthenticate={() => {}}
      onSubmitInviteCode={async () => {}}
      onFetchConsents={() => {}}
      onAcceptConsents={() => {}}
      onWithdrawConsents={() => {}}
      onRefresh={() => {}}
      onClearToken={() => {}}
      showDiagnostics={false}
    />
  );

  expect(screen.getByDisplayValue('https://api.kukuri.app')).toBeInTheDocument();
  expect(screen.getAllByRole('button', { name: 'Authenticate' }).length).toBeGreaterThan(0);
  expect(screen.queryByText('Session Phase')).not.toBeInTheDocument();
  expect(screen.queryByText('Session Activation')).not.toBeInTheDocument();
});

// #1550: 「Add Node」は欄の label で包まない。読み上げ名は button 自身の文字で、欄の文字を押してもノードは足されない。
test('community node add button keeps its own name and ignores clicks on the field text', async () => {
  const user = userEvent.setup();
  const onAddNode = vi.fn();

  render(
    <CommunityNodePanel
      view={createCommunityNodePanelFixture()}
      saveDisabled={false}
      resetDisabled={false}
      clearDisabled={false}
      onAddNode={onAddNode}
      onNodeBaseUrlChange={() => {}}
      onRemoveNode={() => {}}
      onSaveNodes={() => {}}
      onReset={() => {}}
      onClearNodes={() => {}}
      onAuthenticate={() => {}}
      onSubmitInviteCode={async () => {}}
      onFetchConsents={() => {}}
      onAcceptConsents={() => {}}
      onWithdrawConsents={() => {}}
      onRefresh={() => {}}
      onClearToken={() => {}}
    />
  );

  for (const text of [
    'Configured Nodes',
    'Each community node has a base URL and requires separate consent before use.',
    'Save nodes before authenticating.',
  ]) {
    await user.click(screen.getByText(text));
  }
  expect(onAddNode).not.toHaveBeenCalled();

  await user.click(screen.getByRole('button', { name: 'Add Node' }));
  expect(onAddNode).toHaveBeenCalledTimes(1);
});

test('developer panel toggle reports the requested developer mode state', async () => {
  const user = userEvent.setup();
  const onDeveloperModeChange = vi.fn();

  render(
    <DeveloperPanel developerModeEnabled={false} onDeveloperModeChange={onDeveloperModeChange} onOpenDiagnostics={vi.fn()} />
  );

  const toggle = screen.getByRole('checkbox', { name: 'Enable developer mode' });
  expect(toggle).not.toBeChecked();

  await user.click(toggle);
  expect(onDeveloperModeChange).toHaveBeenCalledWith(true);
});

test('developer panel shortcuts follow the current controlled mode', async () => {
  const user = userEvent.setup();
  const onOpenDiagnostics = vi.fn();
  const props = { onDeveloperModeChange: vi.fn(), onOpenDiagnostics };
  const { rerender } = render(<DeveloperPanel {...props} developerModeEnabled />);
  expect(screen.getByRole('status')).toHaveTextContent('Developer mode is on.');
  for (const [name, section] of [
    ['Connection diagnostics', 'connectivity'],
    ['Discovery diagnostics', 'discovery'],
    ['Community node diagnostics', 'community-node'],
  ]) {
    await user.click(screen.getByRole('button', { name }));
    expect(onOpenDiagnostics).toHaveBeenLastCalledWith(section);
  }
  rerender(<DeveloperPanel {...props} developerModeEnabled={false} />);
  expect(screen.getByRole('status')).toHaveTextContent('Developer mode is off.');
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

// #961: セーフティ設定はミュートとブロックの違いを説明し、一覧への導線を持つ。
test('safety panel explains mute versus block and opens each list', async () => {
  const user = userEvent.setup();
  const onOpenMutedUsers = vi.fn();
  const onOpenBlockedUsers = vi.fn();

  render(
    <SafetyPanel
      adultContentEnabled={false}
      onAdultContentEnabledChange={vi.fn()}
      onOpenMutedUsers={onOpenMutedUsers}
      onOpenBlockedUsers={onOpenBlockedUsers}
    />
  );

  expect(screen.getByText(/Mute hides a user's posts/)).toBeInTheDocument();
  expect(screen.getByText(/Block is signed with your account/)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Open muted users' }));
  expect(onOpenMutedUsers).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: 'Open blocked users' }));
  expect(onOpenBlockedUsers).toHaveBeenCalledTimes(1);
});

// #962/#978: 開発者モードON時は診断レポートへの導線と、ログビューア(slot)を表示する。OFF時は両方消える。
test('developer panel links to the diagnostic report and shows the log viewer only while enabled', async () => {
  const user = userEvent.setup();
  const onOpenDiagnostics = vi.fn();
  const onRefresh = vi.fn();
  const logs = (
    <DeveloperLogViewer status='ready' view={fixtureLogsView()} errorMessage={null} onRefresh={onRefresh} />
  );
  const props = { onDeveloperModeChange: vi.fn(), onOpenDiagnostics, logs };
  const { rerender } = render(<DeveloperPanel {...props} developerModeEnabled />);
  await user.click(screen.getByRole('button', { name: 'Diagnostic report' }));
  expect(onOpenDiagnostics).toHaveBeenLastCalledWith('release');
  expect(screen.getByRole('heading', { name: 'Logs' })).toBeVisible();
  const region = screen.getByRole('region', { name: 'Recent logs, 12 lines' });
  expect(within(region).getByText(/desktop profile lease acquired/)).toBeVisible();
  expect(within(region).getByText('ERROR')).toBeVisible();
  expect(screen.getByText(/The newest 2,000 lines \/ 1 MiB are kept/)).toBeVisible();
  expect(screen.queryByText(/does not include a log viewer/i)).not.toBeInTheDocument();
  expect(screen.getByRole('link', { name: /Troubleshooting guide/ })).toHaveAttribute(
    'href',
    'https://github.com/kukuri-app/kukuri/blob/main/docs/runbooks/mvp-troubleshooting.md'
  );
  await user.click(screen.getByRole('button', { name: 'Refresh logs' }));
  expect(onRefresh).toHaveBeenCalledTimes(1);
  rerender(<DeveloperPanel {...props} developerModeEnabled={false} />);
  expect(screen.queryByRole('heading', { name: 'Logs' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Diagnostic report' })).not.toBeInTheDocument();
});

// #978: コピー／書き出しは利用者の click でだけ起き、本文は表示中の行と除外規則を含む。
test('developer log viewer copies and exports the shown lines only on request', async () => {
  const user = userEvent.setup();
  const writeText = vi.fn<(text: string) => Promise<void>>(async () => undefined);
  vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } });
  vi.spyOn(URL, 'createObjectURL').mockImplementation(() => 'blob:kukuri/logs');
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
  const downloads: string[] = [];
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
    downloads.push(this.download);
  });

  render(<DeveloperLogViewer status='ready' view={fixtureLogsView()} errorMessage={null} onRefresh={vi.fn()} />);
  expect(writeText).not.toHaveBeenCalled();
  expect(downloads).toEqual([]);

  await user.click(screen.getByRole('button', { name: 'Copy logs' }));
  expect(writeText).toHaveBeenCalledTimes(1);
  const copied = writeText.mock.calls[0]?.[0] ?? '';
  expect(copied).toContain('# kukuri desktop logs');
  expect(copied).toContain('secret keys, auth tokens, DM bodies, and passphrases are never logged');
  expect(copied).toContain('kukuri_desktop_tauri_lib: desktop profile lease acquired');
  expect(screen.getByText('Logs copied.')).toBeVisible();

  await user.click(screen.getByRole('button', { name: 'Export logs' }));
  expect(downloads).toEqual(['kukuri-logs.txt']);
  expect(screen.getByText('Logs exported.')).toBeVisible();
});

test('developer log viewer distinguishes loading, empty, dropped and error states', () => {
  const onRefresh = vi.fn();
  const { rerender } = render(
    <DeveloperLogViewer status='loading' view={null} errorMessage={null} onRefresh={onRefresh} />
  );
  expect(screen.getByText('Loading logs…')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Refresh logs' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Copy logs' })).toBeDisabled();

  rerender(
    <DeveloperLogViewer
      status='ready'
      view={{ ...fixtureLogsView(), entries: [], oldestSeq: null, nextSeq: 1, droppedOlder: false }}
      errorMessage={null}
      onRefresh={onRefresh}
    />
  );
  expect(screen.getByText('No logs yet.')).toBeVisible();
  expect(screen.queryByRole('region', { name: /Recent logs/ })).not.toBeInTheDocument();

  rerender(
    <DeveloperLogViewer
      status='ready'
      view={{ ...fixtureLogsView(), oldestSeq: 2401, nextSeq: 4401, droppedOlder: true, gapSinceLastRefresh: true }}
      errorMessage={null}
      onRefresh={onRefresh}
    />
  );
  expect(screen.getByText(/older lines are no longer available/)).toBeVisible();
  expect(screen.getByText(/lines in between were lost/)).toBeVisible();

  rerender(
    <DeveloperLogViewer status='error' view={null} errorMessage='requires Ready startup state' onRefresh={onRefresh} />
  );
  expect(screen.getByRole('alert')).toHaveTextContent('Logs could not be read.');
  expect(screen.getByRole('alert')).toHaveTextContent('requires Ready startup state');
  expect(screen.getByRole('button', { name: 'Refresh logs' })).toBeEnabled();
});

function fixtureLogsView(): DesktopLogsView {
  const snapshot = developerLogFixtureSnapshot();
  return {
    entries: snapshot.entries,
    oldestSeq: snapshot.oldest_seq,
    nextSeq: snapshot.next_seq,
    maxEntries: snapshot.max_entries,
    maxBytes: snapshot.max_bytes,
    droppedOlder: false,
    gapSinceLastRefresh: false,
  };
}
