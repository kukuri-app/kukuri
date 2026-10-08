import { ChangeEvent, FormEvent, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import {
  ContextActionMenu,
  contextActionMenuPositionFromKeyboard,
  contextActionMenuPositionFromPointer,
  type ContextActionMenuPosition,
} from '@/components/ui/context-action-menu';
import { ImageCropDialog } from '@/components/ui/ImageCropDialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Notice } from '@/components/ui/notice';
import {
  type CustomReactionAssetView,
  type CustomReactionCropRect,
  type CustomReactionSetView,
} from '@/lib/api';
import { copyTextToClipboard } from '@/lib/utils';

import { type ReactionsPanelView } from './types';

type ReactionsPanelProps = {
  view: ReactionsPanelView;
  creating: boolean;
  mediaObjectUrls?: Record<string, string | null>;
  onCreateAsset: (file: File, cropRect: CustomReactionCropRect, searchKey: string) => void;
  onRemoveBookmark: (assetId: string) => Promise<void>;
  onListSets: () => Promise<CustomReactionSetView[]>;
  onCreateSet: (name: string, assets: CustomReactionAssetView[]) => Promise<CustomReactionSetView>;
};

// #1232 AC-4: 1 つのセットに入れられるリアクションの数と、作ったセットの一覧の件数（core・app-api と同じ）。
const REACTION_SET_MAX_ITEMS = 100;

export function ReactionsPanel({
  view,
  creating,
  mediaObjectUrls = {},
  onCreateAsset,
  onRemoveBookmark,
  onListSets,
  onCreateSet,
}: ReactionsPanelProps) {
  const { t } = useTranslation(['settings', 'common']);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const fileButtonRef = useRef<HTMLButtonElement>(null);
  const fileStatusId = useId();
  const [draftFile, setDraftFile] = useState<File | null>(null);
  const [draftPreviewUrl, setDraftPreviewUrl] = useState<string | null>(null);
  const [draftCrop, setDraftCrop] = useState<CustomReactionCropRect | null>(null);
  const [draftSearchKey, setDraftSearchKey] = useState('');
  const [draftError, setDraftError] = useState<string | null>(null);
  const [cropDialogOpen, setCropDialogOpen] = useState(false);
  const [cropDialogFile, setCropDialogFile] = useState<File | null>(null);
  const [selectedAssetIds, setSelectedAssetIds] = useState<Set<string>>(new Set());
  const [removingBookmarks, setRemovingBookmarks] = useState(false);
  const [savedMenuPosition, setSavedMenuPosition] = useState<ContextActionMenuPosition | null>(
    null
  );
  const [savedMenuAssetId, setSavedMenuAssetId] = useState<string | null>(null);
  const [reactionSets, setReactionSets] = useState<CustomReactionSetView[]>([]);
  const [reactionSetName, setReactionSetName] = useState('');
  const [reactionSetPending, setReactionSetPending] = useState(false);
  const [reactionSetError, setReactionSetError] = useState<string | null>(null);

  useEffect(() => {
    const input = fileInputRef.current;
    const restoreFocus = () => fileButtonRef.current?.focus();
    input?.addEventListener('cancel', restoreFocus);
    return () => input?.removeEventListener('cancel', restoreFocus);
  }, []);

  useEffect(() => {
    return () => {
      if (draftPreviewUrl) {
        URL.revokeObjectURL(draftPreviewUrl);
      }
    };
  }, [draftPreviewUrl]);

  useEffect(() => {
    onListSets().then(setReactionSets, () => setReactionSetError(t('reactions.setsLoadFailed')));
  }, [onListSets, t]);

  useEffect(() => {
    const validAssetIds = new Set(
      [...view.ownedAssets, ...view.bookmarkedAssets].map((asset) => asset.asset_id)
    );
    setSelectedAssetIds((current) => {
      const next = new Set([...current].filter((assetId) => validAssetIds.has(assetId)));
      return next.size === current.size ? current : next;
    });
    if (savedMenuAssetId && !validAssetIds.has(savedMenuAssetId)) {
      setSavedMenuAssetId(null);
      setSavedMenuPosition(null);
    }
  }, [savedMenuAssetId, view.bookmarkedAssets, view.ownedAssets]);

  const handleDraftFileChange = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0] ?? null;
    // The accepted crop is the draft; clearing the input permits the same source again.
    event.target.value = '';
    if (creating) return;
    if (!file) {
      return;
    }
    setCropDialogFile(file);
    setCropDialogOpen(true);
    setDraftError(null);
  };

  const selectedSavedIds = view.bookmarkedAssets
    .map((asset) => asset.asset_id)
    .filter((assetId) => selectedAssetIds.has(assetId));
  const allSavedSelected =
    view.bookmarkedAssets.length > 0 && selectedSavedIds.length === view.bookmarkedAssets.length;
  const savedMenuAsset =
    view.bookmarkedAssets.find((asset) => asset.asset_id === savedMenuAssetId) ?? null;
  // 自作と保存済みで同じ ID（同じ画像＋検索名）は 1 件として数える。
  const selectedAssets = [
    ...new Map(
      [...view.ownedAssets, ...view.bookmarkedAssets]
        .filter((asset) => selectedAssetIds.has(asset.asset_id))
        .map((asset) => [asset.asset_id, asset])
    ).values(),
  ];

  const handleToggleAssets = (assetIds: string[], checked: boolean) => {
    setSelectedAssetIds((current) => {
      const next = new Set(current);
      for (const assetId of assetIds) {
        if (checked) {
          next.add(assetId);
        } else {
          next.delete(assetId);
        }
      }
      return next;
    });
  };

  const handleCreateSet = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setReactionSetPending(true);
    setReactionSetError(null);
    try {
      const created = await onCreateSet(reactionSetName.trim(), selectedAssets);
      setReactionSets((current) => [created, ...current].slice(0, REACTION_SET_MAX_ITEMS));
      setReactionSetName('');
      setSelectedAssetIds(new Set());
    } catch {
      setReactionSetError(t('reactions.setCreateFailed'));
    } finally {
      setReactionSetPending(false);
    }
  };

  const handleRemoveSavedAssets = async (assetIds: string[]) => {
    if (assetIds.length === 0) {
      return;
    }
    setRemovingBookmarks(true);
    try {
      await Promise.all(assetIds.map((assetId) => onRemoveBookmark(assetId)));
    } finally {
      setRemovingBookmarks(false);
    }
  };

  const savedMenuItems = savedMenuAsset
    ? [
        {
          id: 'copy-hash',
          label: t('common:actions.copyHash'),
          onSelect: async () => {
            await copyTextToClipboard(savedMenuAsset.blob_hash);
          },
        },
        {
          id: 'clear',
          label: t('common:actions.clear'),
          tone: 'danger' as const,
          onSelect: async () => {
            await handleRemoveSavedAssets([savedMenuAsset.asset_id]);
          },
        },
      ]
    : [];

  return (
    <div className='shell-main-stack reactions-panel'>
      <section className='shell-main-stack'>
        <div>
          <h4>{t('reactions.myCustomReactions')}</h4>
          <small>{t('reactions.myCustomReactionsHint')}</small>
        </div>
        <div className='space-y-2'>
          <span>{t('reactions.uploadLabel')}</span>
          <div className='flex min-w-0 flex-wrap items-center gap-2'>
            <Button
              ref={fileButtonRef}
              type='button'
              variant='secondary'
              disabled={creating}
              aria-describedby={fileStatusId}
              onClick={() => fileInputRef.current?.click()}
            >
              {t('common:composer.chooseFiles')}
            </Button>
            <span id={fileStatusId} role='status' className='min-w-0 break-all text-sm text-muted-foreground'>
              {draftFile ? draftFile.name : t('common:composer.noFilesSelected')}
            </span>
          </div>
          <Input
            ref={fileInputRef}
            hidden
            className='hidden'
            type='file'
            accept='image/*,.gif'
            aria-label={t('reactions.uploadLabel')}
            disabled={creating}
            onChange={handleDraftFileChange}
          />
        </div>
        {draftError ? <Notice tone='destructive'>{draftError}</Notice> : null}
        {draftFile && draftPreviewUrl && draftCrop ? (
          <div className='reactions-editor-grid'>
            <div className='shell-main-stack'>
              <div className='reactions-preview-card'>
                <div
                  className='reactions-preview-thumb'
                  role='img'
                  style={{ backgroundImage: `url(${draftPreviewUrl})` }}
                  aria-label={t('reactions.preview')}
                />
                <small>{t('reactions.previewHint')}</small>
              </div>
              <Button
                variant='secondary'
                type='button'
                onClick={() => {
                  setCropDialogFile(draftFile);
                  setCropDialogOpen(true);
                }}
              >
                {t('reactions.editCrop')}
              </Button>
            </div>
            <div className='shell-main-stack'>
              <Label>
                <span>{t('reactions.searchKeyLabel')}</span>
                <Input
                  value={draftSearchKey}
                  placeholder={t('reactions.searchKeyPlaceholder')}
                  onChange={(event) => {
                    setDraftSearchKey(event.target.value);
                    if (draftError === t('reactions.searchKeyRequired')) {
                      setDraftError(null);
                    }
                  }}
                />
              </Label>
              <div className='post-actions-inline'>
                <Button
                  type='button'
                  disabled={creating}
                  onClick={() => {
                    const normalizedSearchKey = draftSearchKey.trim();
                    if (!normalizedSearchKey) {
                      setDraftError(t('reactions.searchKeyRequired'));
                      return;
                    }
                    onCreateAsset(draftFile, draftCrop, normalizedSearchKey);
                  }}
                >
                  {t('common:actions.save')}
                </Button>
                <Button
                  variant='secondary'
                  type='button'
                  disabled={creating}
                  onClick={() => {
                    if (draftPreviewUrl) {
                      URL.revokeObjectURL(draftPreviewUrl);
                    }
                    setDraftFile(null);
                    setDraftPreviewUrl(null);
                    setDraftCrop(null);
                    setDraftSearchKey('');
                    setDraftError(null);
                  }}
                >
                  {t('common:actions.cancel', { defaultValue: 'Cancel' })}
                </Button>
              </div>
            </div>
          </div>
        ) : null}
        <div className='reactions-asset-grid'>
          {view.ownedAssets.map((asset) => (
            <div key={asset.asset_id} className='reactions-asset-card relative'>
              <label className='reactions-saved-checkbox'>
                <input
                  type='checkbox'
                  aria-label={t('reactions.selectReaction', { key: asset.search_key })}
                  checked={selectedAssetIds.has(asset.asset_id)}
                  onChange={(event) => handleToggleAssets([asset.asset_id], event.target.checked)}
                />
              </label>
              {typeof mediaObjectUrls[asset.blob_hash] === 'string' ? (
                <img
                  className='reactions-asset-thumb'
                  src={mediaObjectUrls[asset.blob_hash] ?? undefined}
                  alt={asset.search_key}
                  data-asset-id={asset.asset_id}
                />
              ) : (
                <div className='reactions-asset-thumb reactions-asset-placeholder'>
                  {asset.search_key.slice(0, 2)}
                </div>
              )}
              <strong>{asset.search_key}</strong>
              <small>{asset.mime}</small>
            </div>
          ))}
          {view.ownedAssets.length === 0 ? <p className='empty-state'>{t('reactions.noOwnedAssets')}</p> : null}
        </div>
      </section>

      <section className='shell-main-stack'>
        <div className='reactions-saved-header'>
          <div>
            <h4>{t('reactions.savedReactions')}</h4>
            <small>{t('reactions.savedReactionsHint')}</small>
          </div>
          {view.bookmarkedAssets.length > 0 ? (
            <div className='reactions-saved-toolbar'>
              <label className='reactions-saved-select-all'>
                <input
                  type='checkbox'
                  checked={allSavedSelected}
                  onChange={(event) =>
                    handleToggleAssets(
                      view.bookmarkedAssets.map((asset) => asset.asset_id),
                      event.target.checked
                    )
                  }
                />
                <span>{t('reactions.selectAll')}</span>
              </label>
              <Button
                variant='secondary'
                type='button'
                disabled={selectedSavedIds.length === 0 || removingBookmarks}
                onClick={() => void handleRemoveSavedAssets(selectedSavedIds)}
              >
                {t('reactions.clearSelected')}
              </Button>
            </div>
          ) : null}
        </div>
        <div className='reactions-saved-list'>
          {view.bookmarkedAssets.map((asset) => {
            const previewUrl =
              typeof mediaObjectUrls[asset.blob_hash] === 'string'
                ? mediaObjectUrls[asset.blob_hash]
                : null;
            const isSelected = selectedAssetIds.has(asset.asset_id);
            return (
              <article
                key={asset.asset_id}
                className={`reactions-saved-tile${isSelected ? ' reactions-saved-tile-selected' : ''}`}
                aria-label={asset.search_key}
                tabIndex={0}
                onContextMenu={(event) => {
                  setSavedMenuAssetId(asset.asset_id);
                  setSavedMenuPosition(contextActionMenuPositionFromPointer(event));
                }}
                onKeyDown={(event) => {
                  const position = contextActionMenuPositionFromKeyboard(event);
                  if (position) {
                    setSavedMenuAssetId(asset.asset_id);
                    setSavedMenuPosition(position);
                  }
                }}
              >
                <label className='reactions-saved-checkbox'>
                  <input
                    type='checkbox'
                    aria-label={t('reactions.selectReaction', { key: asset.search_key })}
                    checked={isSelected}
                    onChange={(event) => handleToggleAssets([asset.asset_id], event.target.checked)}
                  />
                </label>
                {previewUrl ? (
                  <img className='reactions-asset-thumb' src={previewUrl} alt={asset.search_key} />
                ) : (
                  <div className='reactions-asset-thumb reactions-asset-placeholder' aria-hidden='true'>
                    {asset.search_key.slice(0, 2)}
                  </div>
                )}
              </article>
            );
          })}
          {view.bookmarkedAssets.length === 0 ? (
            <p className='empty-state'>{t('reactions.noSavedAssets')}</p>
          ) : null}
        </div>
      </section>

      <section className='shell-main-stack'>
        <div>
          <h4>{t('reactions.sets')}</h4>
          <small>{t('reactions.setsHint', { max: REACTION_SET_MAX_ITEMS })}</small>
        </div>
        <form
          className='flex min-w-0 flex-wrap items-end gap-2'
          onSubmit={(event) => void handleCreateSet(event)}
        >
          <Label className='min-w-[12rem] flex-1'>
            <span>{t('reactions.setName')}</span>
            <Input
              value={reactionSetName}
              maxLength={64}
              onChange={(event) => setReactionSetName(event.target.value)}
            />
          </Label>
          <Button
            type='submit'
            disabled={
              reactionSetPending ||
              !reactionSetName.trim() ||
              selectedAssets.length === 0 ||
              selectedAssets.length > REACTION_SET_MAX_ITEMS
            }
          >
            {t('reactions.createSet', { count: selectedAssets.length })}
          </Button>
        </form>
        {selectedAssets.length > REACTION_SET_MAX_ITEMS ? (
          <Notice tone='warning'>
            {t('reactions.setTooLarge', { max: REACTION_SET_MAX_ITEMS, count: selectedAssets.length })}
          </Notice>
        ) : null}
        {reactionSetError ? <Notice tone='destructive'>{reactionSetError}</Notice> : null}
        <h5>{t('reactions.createdSets', { max: REACTION_SET_MAX_ITEMS })}</h5>
        <ul className='grid gap-2'>
          {reactionSets.map((set) => (
            <li key={set.set_hash} className='reactions-saved-header'>
              <div className='min-w-0'>
                <strong className='block break-all'>{set.name}</strong>
                <small>{t('reactions.setItemCount', { count: set.item_count })}</small>
              </div>
              <Button
                variant='secondary'
                type='button'
                aria-label={t('reactions.copySetLinkOf', { name: set.name })}
                onClick={() => void copyTextToClipboard(`kukuri:reaction-set:${set.set_hash}`)}
              >
                {t('reactions.copySetLink')}
              </Button>
            </li>
          ))}
        </ul>
        {reactionSets.length === 0 ? <p className='empty-state'>{t('reactions.noSets')}</p> : null}
      </section>

      {view.panelError ? <Notice tone='destructive'>{view.panelError}</Notice> : null}

      <ImageCropDialog
        open={cropDialogOpen}
        file={cropDialogFile}
        title={t('reactions.cropTitle')}
        description={t('reactions.cropDescription')}
        confirmLabel={t('common:actions.save')}
        onOpenChange={(open) => {
          setCropDialogOpen(open);
          if (!open) {
            setCropDialogFile(null);
          }
        }}
        onConfirm={async ({ file, cropRect, croppedFile }) => {
          if (draftPreviewUrl) {
            URL.revokeObjectURL(draftPreviewUrl);
          }
          setDraftFile(file);
          setDraftCrop(cropRect);
          setDraftPreviewUrl(URL.createObjectURL(croppedFile));
          setCropDialogOpen(false);
          setCropDialogFile(null);
        }}
      />
      <ContextActionMenu
        open={savedMenuAsset !== null}
        position={savedMenuPosition}
        items={savedMenuItems}
        onClose={() => {
          setSavedMenuAssetId(null);
          setSavedMenuPosition(null);
        }}
      />
    </div>
  );
}
