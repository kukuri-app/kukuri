import { useShallow } from 'zustand/react/shallow';
import { useRef, useState } from 'react';
import { firstUnconsentedCommunityNode } from '@/lib/api/communityNodeAvailability';
import { InitialProfileSetup } from './InitialProfileSetup';
import type { DesktopApi } from '@/lib/api';
import { communityIndexNodeLabel } from '@/lib/api/communityIndex';
import { communityIndexAvailability, type CommunityNodeAvailability } from '@/lib/api/communityNodeAvailability';
import { CommunityNodeConsentDialog } from '@/components/settings/CommunityNodeConsentDialog';
import { CommunityNodeOnboardingDialog } from '@/components/settings/CommunityNodeOnboardingDialog';
import { CommunityIndexAvailabilityNotice } from '@/components/core/CommunityIndexAvailabilityNotice';
import { useCommunityNodeConsentFlow, type AcceptCommunityNodeConsents } from '@/shell/actions/useCommunityNodeConsentFlow';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { useCommunityNodeOnboarding, useCommunityNodeReconsent } from './useCommunityNodeOnboarding';

export function CommunityNodeOnboarding({ api, onAccept, onOpenSettings, onRetry }: {
  api: DesktopApi;
  onAccept: AcceptCommunityNodeConsents;
  onOpenSettings: () => void;
  onRetry: (availability: CommunityNodeAvailability) => Promise<void>;
}) {
  const intro = useCommunityNodeOnboarding();
  const [resolvedFor, setResolvedFor] = useState<string | null>(null);
  const [profileRequired, setProfileRequired] = useState(false);
  const initialReview = useRef(false);
  const setPreference = useDesktopShellFieldSetter('communityIndexNodePreference');
  const state = useDesktopShellStore(useShallow((s) => ({
    config: s.communityNodeConfig, statuses: s.communityNodeStatuses, manifests: s.communityNodeManifests,
    preference: s.communityIndexNodePreference, configLoaded: s.communityNodeConfigLoaded,
    statusesLoaded: s.communityNodeStatusesLoaded,
    statusError: Boolean(s.communityNodeConfigError || s.communityNodeStatusError),
    author: s.syncStatus.local_author_pubkey,
  })));
  const consent = useCommunityNodeConsentFlow({
    api, configuredBaseUrls: state.config.nodes.map((node) => node.base_url),
    statuses: state.statuses, acceptConsents: onAccept,
    onAccepted: () => { if (initialReview.current) { initialReview.current = false; setResolvedFor(state.author); } },
    onDismiss: () => { if (initialReview.current) { initialReview.current = false; if (profileRequired) intro.resume(); } },
  });
  const baseUrl = intro.baseUrl;
  useCommunityNodeReconsent(consent.open, Boolean(baseUrl || consent.dialog));
  const availability = communityIndexAvailability(state);
  const loaded = state.configLoaded && state.statusesLoaded && !state.statusError;
  const nodeReady = resolvedFor === state.author || (loaded && !firstUnconsentedCommunityNode(state.config, state.statuses, true));
  return <>
    {state.statusError ? <CommunityIndexAvailabilityNotice
      availability={availability} onRetry={() => onRetry(availability)} onReviewPolicies={consent.open}
      onOpenSettings={onOpenSettings} onAutomatic={() => setPreference({ mode: 'auto' })}
    /> : null}
    {baseUrl ? <CommunityNodeOnboardingDialog
      baseUrl={baseUrl}
      nodeLabel={state.manifests[baseUrl]?.status === 'ok'
        ? communityIndexNodeLabel(baseUrl, state.manifests[baseUrl]) : new URL(baseUrl).host}
      onDismiss={() => { intro.dismiss(); setResolvedFor(state.author); }}
      onReview={() => { initialReview.current = true; consent.open(baseUrl, intro.handOff()); }}
      onOpenSettings={() => { intro.handOff(); onOpenSettings(); }}
      onCloseAutoFocus={intro.restoreFocus}
    /> : null}
    {consent.dialog ? <CommunityNodeConsentDialog {...consent.dialog} /> : null}
    <InitialProfileSetup key={state.author} onRequired={setProfileRequired} ready={nodeReady && !baseUrl && !consent.dialog} nodeFailed={state.statusError} onSkipNode={() => setResolvedFor(state.author)} />
  </>;
}
