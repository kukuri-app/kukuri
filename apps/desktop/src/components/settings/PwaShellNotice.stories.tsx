import type { Meta, StoryObj } from '@storybook/react-vite';
import { PwaShellView } from './PwaShellNotice';

const state = { ready: true, available: false, checking: false, applying: false, error: null, checkedAt: null };
const meta = { title: 'Settings/PwaShellNotice', component: PwaShellView,
  args: { state, onCheck: () => {}, onApply: () => {} } } satisfies Meta<typeof PwaShellView>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Preparing: Story = { args: { state: { ...state, ready: false } } };
export const Ready: Story = {};
export const Checking: Story = { args: { state: { ...state, checking: true } } };
export const Available: Story = { args: { state: { ...state, available: true } } };
export const Applying: Story = { args: { state: { ...state, available: true, applying: true } } };
export const PreparationFailed: Story = { args: { state: { ...state, error: 'prepare' } } };
export const ApplyFailed: Story = { args: { state: { ...state, available: true, error: 'apply' } } };
