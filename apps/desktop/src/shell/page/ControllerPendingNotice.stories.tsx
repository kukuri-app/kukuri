import { useEffect } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';

import { CONTROLLER_PENDING, explainControllerPending } from '@/lib/api/invoke/dispatch';
import { InvokeError } from '@/lib/api/invoke/error';
import { ControllerPendingNotice } from './ControllerPendingNotice';

// #1220 AC-3a: 共有リンクの作成・書き込みが、新しいアクセスの配布を別の端末が行うために保留を返したとき。
function PendingStory() {
  useEffect(() => {
    explainControllerPending(new InvokeError(CONTROLLER_PENDING, CONTROLLER_PENDING));
  }, []);
  return <ControllerPendingNotice />;
}

const meta = { title: 'Shell/ControllerPendingNotice', component: PendingStory, parameters: { layout: 'fullscreen' } } satisfies Meta<typeof PendingStory>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Pending: Story = {};
