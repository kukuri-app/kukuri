import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Image } from 'lucide-react';
import { liveThumbnailDataUrl } from '@/components/extended/liveMedia';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';

export function LiveThumbnailField({ value, onChange, disabled }: {
  value: string | null;
  onChange: (value: string | null) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation('live');
  const [error, setError] = useState(false);
  return <div className='live-thumbnail-field'>
    <Label><span>{t('thumbnail.choose')}</span><input type='file' accept='image/*' disabled={disabled}
      onChange={(event) => {
        const file = event.target.files?.[0];
        event.target.value = '';
        if (!file) return;
        setError(false);
        void createImageBitmap(file).then((bitmap) => {
          try { onChange(liveThumbnailDataUrl(bitmap, bitmap.width, bitmap.height)); }
          finally { bitmap.close(); }
        }).catch(() => setError(true));
      }} /></Label>
    {value ? <><img src={value} alt={t('thumbnail.label')} /><Button type='button' variant='ghost' disabled={disabled}
      onClick={() => onChange(null)}>{t('thumbnail.remove')}</Button></> : <p>{t('thumbnail.automatic')}</p>}
    {error ? <p role='alert'>{t('thumbnail.failed')}</p> : null}
  </div>;
}

export function LiveThumbnailImage({ url }: { url: string | null | undefined }) {
  const { t } = useTranslation('live');
  return <div className='live-thumbnail-image'>
    {url ? <img src={url} alt='' /> : <span><Image aria-hidden='true' /><span>{t('thumbnail.waiting')}</span></span>}
  </div>;
}
