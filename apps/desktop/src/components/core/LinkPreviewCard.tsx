import { useRef, type MouseEvent } from 'react';

import {
  fetchLinkPreview,
  readLinkPreviewRecord,
  type LinkPreviewFetcher,
} from '@/lib/api';
import { firstExternalUrl } from '@/lib/externalUrls';
import { isTauriRuntime } from '@/lib/releaseReadiness';
import { useExternalLinkOpener } from '@/lib/useExternalLinkOpener';

import { useLinkPreview } from './useLinkPreview';

function safePreviewImageData(value: string | null): string | null {
  return value && /^data:image\/(?:png|jpeg|gif|webp);base64,/i.test(value) ? value : null;
}

export function LinkPreviewCard({
  content,
  objectId,
  enabled,
  fetcher,
}: {
  content: string;
  objectId: string;
  enabled: boolean;
  fetcher?: LinkPreviewFetcher;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const externalLink = useExternalLinkOpener();
  const url = enabled ? firstExternalUrl(content) : null;
  // Web は外部 URL を取得せず、投稿者が書いた record を読む(ADR 0051 §7)。
  const resolver = fetcher ?? (isTauriRuntime() ? fetchLinkPreview : readLinkPreviewRecord);
  const state = useLinkPreview({
    enabled,
    fetcher: resolver,
    objectId,
    targetRef: containerRef,
    url,
  });
  const imageData =
    state.status === 'available' ? safePreviewImageData(state.preview.image_data_url) : null;

  const activate = (event: MouseEvent<HTMLAnchorElement>) => {
    event.stopPropagation();
    externalLink.linkProps.onClick(event);
  };
  const activateAux = (event: MouseEvent<HTMLAnchorElement>) => {
    event.stopPropagation();
    externalLink.linkProps.onAuxClick(event);
  };

  return (
    <div ref={containerRef} className='link-preview-slot' data-state={state.status}>
      {state.status === 'available' ? (
        <a
          className='link-preview-card'
          href={url ?? state.preview.url}
          target='_blank'
          rel='noopener noreferrer'
          aria-label={`${state.preview.title} — ${state.preview.source_label}`}
          aria-disabled={externalLink.pending || undefined}
          onClick={activate}
          onAuxClick={activateAux}
          onKeyDown={(event) => event.stopPropagation()}
        >
          {imageData ? (
            <img
              className='link-preview-image'
              src={imageData}
              alt=''
              aria-hidden='true'
            />
          ) : null}
          <span className='link-preview-copy'>
            <span className='link-preview-source'>{state.preview.source_label}</span>
            <strong className='link-preview-title'>{state.preview.title}</strong>
            {state.preview.description ? (
              <span className='link-preview-description'>{state.preview.description}</span>
            ) : null}
          </span>
        </a>
      ) : null}
    </div>
  );
}
