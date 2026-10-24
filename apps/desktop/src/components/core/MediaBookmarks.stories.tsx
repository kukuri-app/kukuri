import type { Meta, StoryObj } from '@storybook/react-vite';
import { useMemo } from 'react';
import type { AttachmentView, BookmarkedMediaView } from '@/lib/api';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { STORY_VIDEO_POSTER_PREVIEW } from '@/components/storyFixtures';
import { MediaAssetActions } from '@/shell/page/MediaAssetActions';
import { MediaBookmarkPicker } from '@/shell/page/MediaBookmarkPicker';
import { MediaBookmarksContext } from './mediaBookmarksContext';

const source = { topic_id: 'kukuri:topic:general', object_id: 'source-post', author_pubkey: 'a'.repeat(64), created_at: 1_700_000_000_000 };
const asset: AttachmentView = { hash: 'b'.repeat(64), mime: 'image/png', role: 'image_original', bytes: 1024, status: 'Available' };
const saved: BookmarkedMediaView = { bookmarked_from_object_id: source.object_id, hash: asset.hash, source, attachments: [asset], content_labels: [], bookmarked_at: 1 };

function MediaBookmarksStory({ state }: { state: 'ready' | 'empty' | 'loading' | 'error' | 'gated' }) {
  const store = useMemo(() => { const store = createDesktopShellStore(); store.getState().setField('timelineAdvisoryLookup', { active: false, settled: {} }); return store; }, []);
  const api = useMemo(() => ({ ...createDesktopMockApi(),
    getBookmarkedMedia: async () => state === 'ready' ? saved : null,
    bookmarkMedia: async () => saved,
    listBookmarkedMedia: async () => {
      if (state === 'loading') return new Promise<never>(() => undefined);
      if (state === 'error') throw new Error('unavailable');
      return { items: state === 'empty' ? [] : [{ ...saved, content_labels: state === 'gated' ? ['adult'] : [] }], newer_cursor: null, older_cursor: null };
    },
  }), [state]);
  return <DesktopShellStoreContext.Provider value={store}><MediaBookmarksContext.Provider value={{ api }}>
    <div className='grid max-w-lg gap-4'>
      <div className='media-frame'><MediaAssetActions attachment={asset} target={{ topic: source.topic_id, objectId: source.object_id, source, channelRef: { kind: 'public' } }}>
        <img className='media-preview' src={STORY_VIDEO_POSTER_PREVIEW} alt='Media attachment' />
      </MediaAssetActions></div>
      <MediaBookmarkPicker onSelect={() => undefined} />
    </div>
  </MediaBookmarksContext.Provider></DesktopShellStoreContext.Provider>;
}

const meta = { title: 'Core/MediaBookmarks', component: MediaBookmarksStory, args: { state: 'ready' } } satisfies Meta<typeof MediaBookmarksStory>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Ready: Story = {};
export const Empty: Story = { args: { state: 'empty' } };
export const Loading: Story = { args: { state: 'loading' } };
export const Failed: Story = { args: { state: 'error' } };
export const Gated: Story = { args: { state: 'gated' } };
