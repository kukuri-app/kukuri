import { type FormEvent, useContext, useState } from 'react';
import { Radio } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Dialog, DialogBody, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import { channelComposeTarget, messageFromError } from '@/shell/presentation';
import type { ColumnScope } from '@/shell/slices/workspace';

import { LiveActionsContext } from './liveActions';
import { LiveThumbnailField } from './LiveThumbnailField';

/** 投稿先を固定して開始する。開く・閉じるだけでは通常の投稿の下書きを変えない。 */
export function LiveStartButton({ scope, audienceLabel, disabled }: {
  scope: ColumnScope;
  audienceLabel: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation('live');
  const actions = useContext(LiveActionsContext);
  const [open, setOpen] = useState(false);
  if (!actions) return null;
  return <>
    <Button variant='secondary' type='button' disabled={disabled} onClick={() => setOpen(true)}>
      <Radio className='size-4' aria-hidden='true' />{t('actions.start')}
    </Button>
    {open ? <LiveStartDialog scope={scope} audienceLabel={audienceLabel} onClose={() => setOpen(false)} /> : null}
  </>;
}

function LiveStartDialog({ scope, audienceLabel, onClose }: {
  scope: ColumnScope;
  audienceLabel: string;
  onClose: () => void;
}) {
  const { t } = useTranslation(['live', 'common']);
  const actions = useContext(LiveActionsContext)!;
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [content, setContent] = useState('');
  const [pending, setPending] = useState(false);
  const [createdId, setCreatedId] = useState<string | null>(null);
  const [thumbnail, setThumbnail] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (pending || !title.trim()) return;
    setPending(true);
    setError(null);
    try {
      const id = createdId ?? await actions.api.createLiveSession(scope.topicId, title.trim(), description.trim(), channelComposeTarget(scope.channelId), content.trim());
      setCreatedId(id);
      if (thumbnail) await actions.api.setLiveThumbnail({ topic: scope.topicId, session_id: id, data_url: thumbnail, custom: true });
      await actions.refresh?.(scope.topicId);
      onClose();
    } catch (error) {
      setError(messageFromError(error, t('live:errors.failedCreate')));
      setPending(false);
    }
  };
  return <Dialog open onOpenChange={(open) => { if (!open && !pending) onClose(); }}>
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{t('live:actions.start')}</DialogTitle>
        <DialogDescription>{t('common:labels.audience')}: {audienceLabel}</DialogDescription>
      </DialogHeader>
      <DialogBody>
        <form className='composer composer-compact' onSubmit={(event) => void submit(event)} aria-busy={pending}>
          <Label><span>{t('live:fields.title')}</span><Input value={title} required disabled={pending}
            placeholder={t('live:fields.placeholders.title')} onChange={(event) => setTitle(event.target.value)} /></Label>
          <Label><span>{t('live:fields.description')}</span><Textarea value={description} disabled={pending}
            placeholder={t('live:fields.placeholders.description')} onChange={(event) => setDescription(event.target.value)} /></Label>
          <Label><span>{t('live:fields.startPost')}</span><Textarea value={content} disabled={pending}
            placeholder={t('live:fields.placeholders.startPost')} onChange={(event) => setContent(event.target.value)} /></Label>
          {error ? <p className='error error-inline' role='alert'>{error}</p> : null}
          <LiveThumbnailField value={thumbnail} onChange={setThumbnail} disabled={pending} />
          <Button type='submit' disabled={pending || !title.trim()}>{t('live:actions.start')}</Button>
        </form>
      </DialogBody>
    </DialogContent>
  </Dialog>;
}
