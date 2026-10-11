import type { Meta, StoryObj } from '@storybook/react-vite';

import { PwaInstallView } from './PwaInstallNotice';

const meta = {
  title: 'Settings/PwaInstallNotice', component: PwaInstallView,
  args: { phase: 'unavailable', onInstall: () => undefined },
} satisfies Meta<typeof PwaInstallView>;
export default meta;
type Story = StoryObj<typeof meta>;
export const BrowserMenu: Story = {};
export const Available: Story = { args: { phase: 'available' } };
export const Prompting: Story = { args: { phase: 'prompting' } };
export const Installed: Story = { args: { phase: 'installed' } };
export const Failed: Story = { args: { phase: 'failed' } };
