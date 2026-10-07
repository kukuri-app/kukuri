import { useState } from 'react';

import type { Meta, StoryObj } from '@storybook/react-vite';

import { AppearancePanel } from '@/components/settings/AppearancePanel';
import { CommunityNodePanel } from '@/components/settings/CommunityNodePanel';
import { ConnectivityPanel } from '@/components/settings/ConnectivityPanel';
import { DiscoveryPanel } from '@/components/settings/DiscoveryPanel';
import {
  createAppearancePanelFixture,
  createCommunityNodePanelFixture,
  createConnectivityPanelFixture,
  createDiscoveryPanelFixture,
} from '@/components/settings/fixtures';
import type { DesktopTheme } from '@/lib/theme';
import type { SupportedLocale } from '@/i18n';
import type { SettingsSection } from '@/components/shell/types';

import { SettingsDrawer } from './SettingsDrawer';

function SettingsDrawerStory({ initialSection = 'connectivity' }: { initialSection?: SettingsSection }) {
  const appearancePanelFixture = createAppearancePanelFixture();
  const connectivityPanelFixture = createConnectivityPanelFixture();
  const discoveryPanelFixture = createDiscoveryPanelFixture();
  const communityNodePanelFixture = createCommunityNodePanelFixture();
  const [open, setOpen] = useState(true);
  const [activeSection, setActiveSection] = useState<SettingsSection>(initialSection);
  const [theme, setTheme] = useState<DesktopTheme>(appearancePanelFixture.selectedTheme);
  const [locale, setLocale] = useState<SupportedLocale>(appearancePanelFixture.selectedLocale);
  const [peerTicketInput, setPeerTicketInput] = useState(connectivityPanelFixture.peerTicketInput);
  const [seedPeersInput, setSeedPeersInput] = useState(discoveryPanelFixture.seedPeersInput);
  const [communityNodes, setCommunityNodes] = useState(communityNodePanelFixture.nodes);

  const sections = [
    {
      id: 'appearance' as const,
      label: 'Appearance',
      description: 'Local light and dark theme selection.',
      content: (
        <AppearancePanel
          view={{ ...createAppearancePanelFixture(), selectedTheme: theme, selectedLocale: locale }}
          onThemeChange={setTheme}
          onLocaleChange={setLocale}
        />
      ),
    },
    {
      id: 'connectivity' as const,
      label: 'Connectivity',
      description: 'Peer status and topic diagnostics',
      content: (
        <ConnectivityPanel
          view={{ ...connectivityPanelFixture, peerTicketInput }}
          onPeerTicketInputChange={setPeerTicketInput}
          onImportPeer={() => undefined}
        />
      ),
    },
    {
      id: 'discovery' as const,
      label: 'Discovery',
      description: 'Seeded DHT configuration and diagnostics',
      content: (
        <DiscoveryPanel
          view={{ ...discoveryPanelFixture, seedPeersInput }}
          saveDisabled={false}
          resetDisabled={false}
          onSeedPeersChange={setSeedPeersInput}
          onSave={() => undefined}
          onReset={() => setSeedPeersInput(discoveryPanelFixture.seedPeersInput)}
          onPublicBlobDiscoveryChange={async () => undefined}
        />
      ),
    },
    {
      id: 'community-node' as const,
      label: 'Community Node',
      description: 'Auth, consent, and connectivity urls',
      content: (
        <CommunityNodePanel
          view={{ ...communityNodePanelFixture, nodes: communityNodes }}
          saveDisabled={false}
          resetDisabled={false}
          clearDisabled={false}
          onAddNode={() =>
            setCommunityNodes((current) => [
              ...current,
              {
                id: `drawer-node-${current.length + 1}`,
                baseUrl: '',
                saved: false,
                diagnostics: [],
                dependency: { diagnostics: [], boundaryNotes: [] },
                consent: {
                  loaded: false,
                  loading: false,
                  loadError: null,
                  withdrawn: false,
                  hasLocalConsent: false,
                  allRequiredAccepted: false,
                  hasPendingUpdate: false,
                  policies: [],
                },
                distanceOptoutEligible: true,
                inviteCodeSaved: false,
                admissionRejectionCode: null,
                lastError: null,
              },
            ])
          }
          onNodeBaseUrlChange={(id, value) =>
            setCommunityNodes((current) =>
              current.map((node) => (node.id === id ? { ...node, baseUrl: value } : node))
            )
          }
          onRemoveNode={(id) =>
            setCommunityNodes((current) => current.filter((node) => node.id !== id))
          }
          onSaveNodes={() => undefined}
          onReset={() => setCommunityNodes(communityNodePanelFixture.nodes)}
          onClearNodes={() => setCommunityNodes([])}
          onAuthenticate={() => undefined}
          onSubmitInviteCode={async () => undefined}
          onFetchConsents={() => undefined}
          onAcceptConsents={() => undefined}
          onWithdrawConsents={() => {}}
          onRefresh={() => undefined}
          onClearToken={() => undefined}
        />
      ),
    },
  ];

  return (
    <SettingsDrawer
      drawerId='storybook-shell-settings'
      open={open}
      onOpenChange={setOpen}
      activeSection={activeSection}
      onSectionChange={setActiveSection}
      sections={sections}
    />
  );
}

const meta = {
  title: 'Shell/SettingsDrawer',
  component: SettingsDrawer,
  parameters: {
    layout: 'fullscreen',
  },
  args: {
    drawerId: 'storybook-shell-settings',
    open: true,
    onOpenChange: () => undefined,
    activeSection: 'connectivity',
    onSectionChange: () => undefined,
    sections: [],
  },
  render: () => <SettingsDrawerStory />,
} satisfies Meta<typeof SettingsDrawer>;

export default meta;

type Story = StoryObj<typeof meta>;

export const ConnectivityOpen: Story = {};

export const AppearanceOpen: Story = {
  render: () => <SettingsDrawerStory initialSection='appearance' />,
};

export const DiscoveryOpen: Story = {
  render: () => <SettingsDrawerStory initialSection='discovery' />,
};
