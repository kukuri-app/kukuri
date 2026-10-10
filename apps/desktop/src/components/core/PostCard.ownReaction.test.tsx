import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import { PostCard } from './PostCard';
import { createView } from './PostCard.testHelpers';

// 自作は同じアカウントの別の端末に届いていないことがあるので、自分のリアクションもこの端末の自作に無ければ保存できる。
test('own custom reaction can be saved when this device does not hold it', async () => {
  const user = userEvent.setup();
  const onBookmarkCustomReaction = vi.fn();
  const ownAsset = {
    asset_id: 'own-asset',
    owner_pubkey: 'a'.repeat(64),
    blob_hash: 'blob-own',
    search_key: 'own-parrot',
    mime: 'image/png',
    bytes: 128,
    width: 128,
    height: 128,
  };
  const view = createView({
    post: {
      ...createView().post,
      reaction_summary: [
        {
          reaction_key_kind: 'custom_asset',
          normalized_reaction_key: 'custom_asset:own-asset',
          emoji: null,
          custom_asset: ownAsset,
          count: 1,
        },
      ],
    },
  });
  const card = (ownedReactionAssets: (typeof ownAsset)[]) => (
    <PostCard
      view={view}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      localAuthorPubkey={'a'.repeat(64)}
      ownedReactionAssets={ownedReactionAssets}
      onBookmarkCustomReaction={onBookmarkCustomReaction}
    />
  );
  const { rerender } = render(card([]));
  const chip = screen.getByRole('button', { name: `${ownAsset.search_key} 1` });

  fireEvent.contextMenu(chip);
  await user.click(screen.getByRole('menuitem', { name: 'Save to use it myself' }));
  expect(onBookmarkCustomReaction).toHaveBeenCalledWith(ownAsset);

  rerender(card([{ ...ownAsset, asset_id: 'content-id' }]));
  fireEvent.contextMenu(chip);
  expect(screen.getByRole('menuitem', { name: 'Save to use it myself' })).toBeDisabled();
});
