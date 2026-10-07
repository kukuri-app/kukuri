import { useState, type ComponentProps } from 'react';

import type { Meta, StoryObj } from '@storybook/react-vite';

import { DiscoveryPanel } from './DiscoveryPanel';
import { discoveryPanelFixture } from './fixtures';
import { SettingsStoryFrame } from './SettingsStoryFrame';

type DiscoveryStoryProps = {
  args: ComponentProps<typeof DiscoveryPanel>;
  width?: 'wide' | 'narrow';
};

function DiscoveryPanelStory({
  args,
  width = 'wide',
}: DiscoveryStoryProps) {
  const [seedPeersInput, setSeedPeersInput] = useState(args.view.seedPeersInput);
  const [publicBlobDiscovery, setPublicBlobDiscovery] = useState(args.view.publicBlobDiscovery);

  return (
    <SettingsStoryFrame width={width}>
      <div>
        <DiscoveryPanel
          {...args}
          view={{ ...args.view, seedPeersInput, publicBlobDiscovery }}
          onSeedPeersChange={setSeedPeersInput}
          onSave={() => {}}
          onReset={() => setSeedPeersInput(args.view.seedPeersInput)}
          // 切替中の表示を確かめられるよう、backend の組み直しの代わりに 1 秒待つ。
          onPublicBlobDiscoveryChange={async (enabled) => {
            await new Promise((resolve) => setTimeout(resolve, 1000));
            setPublicBlobDiscovery(enabled);
          }}
        />
      </div>
    </SettingsStoryFrame>
  );
}

const meta = {
  title: 'Settings/DiscoveryPanel',
  component: DiscoveryPanel,
  render: (args) => <DiscoveryPanelStory args={args} />,
  args: {
    view: discoveryPanelFixture,
    saveDisabled: false,
    resetDisabled: false,
    onSeedPeersChange: () => {},
    onSave: () => {},
    onReset: () => {},
    onPublicBlobDiscoveryChange: async () => {},
  },
} satisfies Meta<typeof DiscoveryPanel>;

export default meta;

type Story = StoryObj<typeof meta>;

// 公開コンテンツの発見はオン（既定）。
export const Ready: Story = {};

export const PublicBlobDiscoveryOff: Story = {
  args: {
    view: { ...discoveryPanelFixture, publicBlobDiscovery: false },
  },
};

// 環境変数で発見の設定を固定した起動。シードと公開コンテンツの発見を変えられない。
export const NarrowLocked: Story = {
  args: {
    view: {
      ...discoveryPanelFixture,
      envLocked: true,
      seedPeersMessage: 'Environment overrides discovery seeds; editing is disabled.',
    },
    saveDisabled: true,
    resetDisabled: true,
  },
  render: (args) => <DiscoveryPanelStory args={args} width='narrow' />,
};

export const Loading: Story = {
  args: {
    view: {
      ...discoveryPanelFixture,
      status: 'loading',
      summaryLabel: 'loading',
    },
  },
};
