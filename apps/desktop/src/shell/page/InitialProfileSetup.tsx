import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogBody, DialogFooter } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Notice } from '@/components/ui/notice';
import { ProfileEditorPanel } from '@/components/extended/ProfileEditorPanel';
import { getProfileSetupRequired, listAccounts, saveInitialProfile } from '@/lib/api/identity';
import { fileToCreateAttachment } from '@/lib/attachments';
import type { ProfileInput } from '@/lib/api';
import { useDesktopShellStore, useDesktopShellStoreApi } from '@/shell/store';

type AccountAccess = { listAccounts: typeof listAccounts; getProfileSetupRequired: typeof getProfileSetupRequired; saveInitialProfile: typeof saveInitialProfile };

export function InitialProfileSetup({ ready, nodeFailed, onSkipNode, accountAccess, onRequired, onRestore }: {
  ready: boolean; nodeFailed: boolean; onSkipNode: () => void;
  accountAccess?: AccountAccess;
  onRequired?: (required: boolean) => void;
  // Web だけ（#1217 AC-5）: サイトデータが消えた後に、以前のアカウントを戻す入口。押すとこの dialog を閉じる。
  onRestore?: () => void;
}) {
  const { t } = useTranslation('shell');
  const store = useDesktopShellStoreApi();
  const { author, localProfile } = useDesktopShellStore(useShallow((s) => ({ author: s.syncStatus.local_author_pubkey, localProfile: s.localProfile })));
  const [accountId, setAccountId] = useState<string | null>(null);
  const [required, setRequired] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [fields, setFields] = useState<ProfileInput>({ name: '', display_name: '', about: '', clear_picture: false });
  const [picture, setPicture] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [canOpen, setCanOpen] = useState(false);
  const dirty = useRef(false);
  const busy = useRef(false);
  useEffect(() => {
    let current = true;
    setRequired(false); setDismissed(false); setAccountId(null); setError(null); onRequired?.(false);
    void (accountAccess?.listAccounts ?? listAccounts)().then(async (snapshot) => {
      const account = snapshot.accounts.find((a) => a.id === snapshot.active_account_id && a.pubkey === author);
      if (!account) return;
      const needsSetup = await (accountAccess?.getProfileSetupRequired ?? getProfileSetupRequired)(account.id);
      if (current) { setAccountId(account.id); setRequired(needsSetup); onRequired?.(needsSetup); }
    }).catch(() => { if (current) setError(t('accountMenu.loadFailed')); });
    return () => { current = false; };
  }, [author, attempt, t, accountAccess, onRequired]);
  useEffect(() => () => { if (picture) URL.revokeObjectURL(picture); }, [picture]);
  useEffect(() => {
    if (localProfile && !dirty.current) setFields({ name: localProfile.name ?? '', display_name: localProfile.display_name ?? '', about: localProfile.about ?? '', clear_picture: false });
  }, [localProfile]);
  useEffect(() => {
    if (!ready || !required || dismissed) { setCanOpen(false); return; }
    const tryOpen = () => {
      const other = [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')].some((element) => !element.hasAttribute('data-initial-profile') && !element.closest('[aria-hidden="true"], [hidden], [data-state="closed"]'));
      if (!other) setCanOpen(true);
    };
    tryOpen();
    const observer = new MutationObserver(tryOpen);
    observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['aria-hidden', 'hidden', 'data-state'] });
    return () => observer.disconnect();
  }, [ready, required, dismissed]);
  const isCurrent = () => store.getState().syncStatus.local_author_pubkey === author;
  const save = async () => {
    if (!accountId || busy.current || !isCurrent()) return;
    busy.current = true; setSaving(true); setError(null);
    try {
      const profile = await (accountAccess?.saveInitialProfile ?? saveInitialProfile)({ account_id: accountId, profile: { name: fields.name ?? null, display_name: fields.display_name ?? null, about: fields.about ?? null, picture_upload: fields.picture_upload ?? null, clear_picture: fields.clear_picture ?? false } });
      if (!isCurrent()) return;
      store.getState().patchState({ localProfile: profile, profileDraft: { name: profile.name ?? '', display_name: profile.display_name ?? '', about: profile.about ?? '', clear_picture: false }, profileDirty: false });
      setRequired(false);
    } catch { if (isCurrent()) setError(t('accountMenu.actionFailed')); }
    finally { busy.current = false; if (isCurrent()) setSaving(false); }
  };
  if (!required || dismissed) return error ? <Notice tone='destructive'>{error}<Button onClick={() => setAttempt((n) => n + 1)}>{t('accountMenu.retry')}</Button></Notice> : null;
  if (!ready) return nodeFailed ? <Notice>{t('initialProfile.nodeUnavailable')}<Button onClick={onSkipNode}>{t('initialProfile.skipNode')}</Button></Notice> : null;
  return <Dialog open={canOpen} onOpenChange={(open) => { if (!open && !busy.current) setDismissed(true); }}>
    <DialogContent data-initial-profile className='w-[min(36rem,94vw)] max-h-[90vh] overflow-hidden flex flex-col' onInteractOutside={(event) => event.preventDefault()}>
      <DialogHeader><DialogTitle>{t('initialProfile.title')}</DialogTitle><DialogDescription>{t('initialProfile.description')}</DialogDescription></DialogHeader>
      <DialogBody className='min-h-0 overflow-y-auto'>
        {onRestore ? <Notice className='mb-4 space-y-2'>
          <p>{t('initialProfile.restore.description')}</p>
          <Button variant='secondary' onClick={() => { setDismissed(true); onRestore(); }}>{t('initialProfile.restore.action')}</Button>
        </Notice> : null}
        <ProfileEditorPanel hideActions authorLabel={localProfile?.display_name || t('accountMenu.unknown')} status='ready' saving={saving} dirty
          error={null} fields={{ displayName: fields.display_name ?? '', name: fields.name ?? '', about: fields.about ?? '' }}
          picturePreviewSrc={picture} hasPicture={Boolean(picture)} pictureInputKey={0}
          onFieldChange={(field, value) => { dirty.current = true; setFields((f) => ({ ...f, [field === 'displayName' ? 'display_name' : field]: value })); }}
          onPictureSelect={(event) => {
            const file = event.target.files?.[0];
            if (!file) return;
            void fileToCreateAttachment(file, 'profile_avatar').then((upload) => {
              if (!isCurrent()) return;
              setFields((f) => ({ ...f, picture_upload: upload, clear_picture: false })); setPicture(URL.createObjectURL(file));
            }).catch(() => { if (isCurrent()) setError(t('accountMenu.actionFailed')); });
          }}
          onPictureClear={() => { setPicture(null); setFields((f) => ({ ...f, picture_upload: null, clear_picture: true })); }}
          onSave={(event) => { event.preventDefault(); void save(); }}
          onReset={() => { setPicture(null); setFields({ name: '', display_name: '', about: '', clear_picture: false }); }} />
        {error ? <Notice tone='destructive'>{error}</Notice> : null}
      </DialogBody>
      <DialogFooter><Button variant='secondary' disabled={saving} onClick={() => setDismissed(true)}>{t('initialProfile.later')}</Button><Button disabled={saving} onClick={() => void save()}>{t(saving ? 'accountMenu.pending' : 'initialProfile.save')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>;
}
