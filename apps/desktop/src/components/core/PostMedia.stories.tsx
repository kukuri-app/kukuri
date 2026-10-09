import type { Meta, StoryObj } from '@storybook/react-vite';

import {
  STORY_IMAGE_MEDIA,
  STORY_VIDEO_PLAYABLE_MEDIA,
  STORY_VIDEO_POSTER_MEDIA,
} from '@/components/storyFixtures';

import { PostMedia } from './PostMedia';

const meta = {
  title: 'Core/PostMedia',
  component: PostMedia,
  render: (args) => (
    <div style={{ width: 'min(32rem, calc(100vw - 2rem))' }}>
      <PostMedia {...args} />
    </div>
  ),
} satisfies Meta<typeof PostMedia>;

export default meta;

type Story = StoryObj<typeof meta>;

export const ImageReady: Story = {
  args: {
    media: STORY_IMAGE_MEDIA,
  },
};

const GRID_IMAGES = ['00b3a4', 'f59d62', '6c8cff', 'e05d8b', '9bc53d'].map((fill, index) => ({
  hash: `story-grid-${index}`,
  src: `data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><rect width="640" height="360" fill="%23${fill}"/></svg>`,
  mime: 'image/png',
}));

// #1690: 3 枚は左の 1 枚と右の 2 段、5 枚以上は先頭 4 枚と「+N」。
export const ImageGridThree: Story = {
  args: {
    media: { ...STORY_IMAGE_MEDIA, imageGalleryItems: GRID_IMAGES.slice(0, 3) },
  },
};

export const ImageGridWithMore: Story = {
  args: {
    media: { ...STORY_IMAGE_MEDIA, extraAttachmentCount: 1, imageGalleryItems: GRID_IMAGES },
  },
};

export const VideoPosterOnly: Story = {
  args: {
    media: STORY_VIDEO_POSTER_MEDIA,
  },
};

export const VideoPlayable: Story = {
  args: {
    media: STORY_VIDEO_PLAYABLE_MEDIA,
  },
};
