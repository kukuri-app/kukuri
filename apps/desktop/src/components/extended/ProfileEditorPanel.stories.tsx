import { useState, type FormEvent } from 'react';

import type { Meta, StoryObj } from '@storybook/react-vite';

import { ProfileEditorPanel } from './ProfileEditorPanel';
import type { ProfileEditorFields } from './types';

const meta = {
  title: 'Extended/ProfileEditorPanel',
  component: ProfileEditorPanel,
} satisfies Meta<typeof ProfileEditorPanel>;

export default meta;

type Story = StoryObj<typeof meta>;

const LOCAL_PUBKEY = '79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798';

const STORY_ARGS = {
  authorLabel: 'Local Author',
  status: 'ready',
  saving: false,
  dirty: false,
  error: null,
  fields: {
    displayName: 'Local Author',
    name: 'local-author',
    about: 'Maintains shell UI migration work.',
    nip05: 'local-author@example.com',
  },
  localPubkey: LOCAL_PUBKEY,
  picturePreviewSrc: 'data:image/png;base64,AA==',
  hasPicture: true,
  pictureInputKey: 0,
  onFieldChange: () => undefined,
  onPictureSelect: () => undefined,
  onPictureClear: () => undefined,
  onSave: (event: FormEvent<HTMLFormElement>) => event.preventDefault(),
  onReset: () => undefined,
} satisfies React.ComponentProps<typeof ProfileEditorPanel>;

function ProfileStory({
  status = 'ready',
  error = null,
  nip05 = 'local-author@example.com',
}: {
  status?: 'loading' | 'ready' | 'error';
  error?: string | null;
  nip05?: string;
}) {
  const [fields, setFields] = useState<ProfileEditorFields>({
    displayName: 'Local Author',
    name: 'local-author',
    about: 'Maintains shell UI migration work.',
    nip05,
  });

  return (
    <ProfileEditorPanel
      authorLabel='Local Author'
      status={status}
      saving={false}
      dirty={false}
      error={error}
      fields={fields}
      localPubkey={LOCAL_PUBKEY}
      picturePreviewSrc='data:image/png;base64,AA=='
      hasPicture={true}
      pictureInputKey={0}
      onFieldChange={(field, value) => setFields((current) => ({ ...current, [field]: value }))}
      onPictureSelect={() => undefined}
      onPictureClear={() => undefined}
      onSave={(event) => event.preventDefault()}
      onReset={() => undefined}
    />
  );
}

export const Ready: Story = {
  args: STORY_ARGS,
  render: (args) => <ProfileStory status={args.status} error={args.error} />,
};

export const ErrorState: Story = {
  args: {
    ...STORY_ARGS,
    status: 'error',
    error: 'profile sync failed',
  },
  render: (args) => <ProfileStory status={args.status} error={args.error} />,
};

// #1670: 形に合わない NIP-05 の識別子は理由を示し、保存できない。
export const InvalidNip05: Story = {
  args: { ...STORY_ARGS, dirty: true },
  render: () => <ProfileStory nip05='Local Author@localhost' />,
};
