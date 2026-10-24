import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';

import { type ComposerDraftMediaView } from './types';

type ComposerDraftPreviewListProps = {
  items: ComposerDraftMediaView[];
  onRemove: (itemId: string) => void;
};

export function ComposerDraftPreviewList({ items, onRemove }: ComposerDraftPreviewListProps) {
  const { t } = useTranslation('common');

  if (items.length === 0) {
    return null;
  }

  return (
    <ul className='draft-attachment-list'>
      {items.map((item) => (
        <li key={item.id} className='draft-attachment-item'>
          <div className='draft-attachment-content'>
            <div className='draft-preview-frame'>
              {item.previewUrl ? <img
                className='draft-preview-image'
                src={item.previewUrl}
                alt={t('composer.draftPreviewAlt', { name: item.sourceName })}
              /> : <span>{t('fallbacks.attachment')}</span>}
            </div>
            <div>
              <strong>{item.sourceName}</strong>
              {item.attachments.map((attachment) => (
                <small key={attachment.key}>
                  {attachment.label} · {attachment.mime} · {attachment.byteSizeLabel}
                </small>
              ))}
            </div>
          </div>
          <Button variant='secondary' type='button' onClick={() => onRemove(item.id)}>
            {t('actions.remove')}
          </Button>
        </li>
      ))}
    </ul>
  );
}
