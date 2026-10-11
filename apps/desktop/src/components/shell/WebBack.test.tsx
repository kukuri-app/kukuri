import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { columnDraftKey, type ColumnDraftTarget } from '@/shell/slices/columnDrafts';
import { ColumnComposerFooter } from './ColumnComposerFooter';
import { useWebBackLayer } from '@/lib/useWebBackLayer';
import { useDesktopShellRouting } from '@/shell/useDesktopShellRouting';
import { useDesktopShellStore } from '@/shell/store';
import { createShellHookHarness } from '@/shell/testSupport/renderShellHook';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';

vi.mock('@/lib/webRuntime', () => ({ IS_WEB_RUNTIME: true }));
const target: ColumnDraftTarget = { columnId: 'public', action: 'post', scope: { topicId: 'general', channelId: null } };
beforeEach(() => {
  history.replaceState(null, '', '#/profile');
  history.pushState(null, '', '#/timeline');
});
async function back() {
  await act(async () => { history.back(); await new Promise(resolve => setTimeout(resolve, 50)); });
}
function setup(preventDismiss = false) {
  const store = createDesktopShellStore();
  const submit = vi.fn(async () => undefined);
  function View() {
    const [open, setOpen] = useState(false);
    return <DesktopShellStoreContext.Provider value={store}>
      <ColumnComposerFooter active destinationLabel='general' locale='en' target={target}
        onActivate={() => undefined} onAttachmentSelection={async () => undefined}
        onAttachmentPaste={async () => undefined} onRemoveAttachment={() => undefined} onSubmit={submit} />
      <button onClick={() => setOpen(true)}>Open overlay</button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent hideClose={preventDismiss} onEscapeKeyDown={event => { if (preventDismiss) event.preventDefault(); }}
          aria-describedby={undefined}><DialogTitle>Overlay</DialogTitle></DialogContent>
      </Dialog>
    </DesktopShellStoreContext.Provider>;
  }
  render(<View />);
  return { store, submit, user: userEvent.setup() };
}
it('back closes the composer, retains its draft, then returns to the prior screen', async () => {
  const { store, submit, user } = setup();
  await user.click(screen.getByRole('button', { name: 'Post to general' }));
  await user.type(screen.getByPlaceholderText('Write a post'), 'keep draft');
  await back();
  await waitFor(() => expect(screen.queryByPlaceholderText('Write a post')).not.toBeInTheDocument());
  expect(location.hash).toBe('#/timeline');
  expect(store.getState().columnDraftsByKey[columnDraftKey(target)]).toMatchObject({ content: 'keep draft', expanded: false });
  expect(submit).not.toHaveBeenCalled();
  await back();
  expect(location.hash).toBe('#/profile');
});
it('back closes one overlay before the underlying composer', async () => {
  const { user } = setup();
  await user.click(screen.getByRole('button', { name: 'Post to general' }));
  await user.type(screen.getByPlaceholderText('Write a post'), 'overlay draft');
  await user.click(screen.getByRole('button', { name: 'Open overlay' }));
  await back();
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.getByDisplayValue('overlay draft')).toBeVisible();
  expect(location.hash).toBe('#/timeline');
  await back();
  await waitFor(() => expect(screen.queryByPlaceholderText('Write a post')).not.toBeInTheDocument());
});
it('a confirmation that refuses dismissal remains open on back', async () => {
  const { user, submit } = setup(true);
  await user.click(screen.getByRole('button', { name: 'Open overlay' }));
  await back();
  await back();
  expect(screen.getByRole('dialog')).toBeVisible();
  expect(location.hash).toBe('#/timeline');
  expect(submit).not.toHaveBeenCalled();
});
it('closing the composer normally leaves no extra back step', async () => {
  const { user } = setup();
  await user.click(screen.getByRole('button', { name: 'Post to general' }));
  await user.click(screen.getByRole('button', { name: 'Close' }));
  await waitFor(() => expect(history.state?.kukuriBackLayer).toBeUndefined());
  await back();
  expect(location.hash).toBe('#/profile');
});
it('a dialog handoff leaves no disposed layer in the screen history', async () => {
  function Handoff() {
    const [stage, setStage] = useState('intro');
    return stage === 'intro' ? <Dialog key='intro' open onOpenChange={() => setStage('terms')}>
      <DialogContent aria-describedby={undefined}><DialogTitle>Introduction</DialogTitle>
        <button onClick={() => setStage('terms')}>Review terms</button>
      </DialogContent>
    </Dialog> : <Dialog key='terms' open={stage === 'terms'} onOpenChange={() => setStage('closed')}>
      <DialogContent aria-describedby={undefined}><DialogTitle>Terms confirmation</DialogTitle></DialogContent>
    </Dialog>;
  }
  render(<Handoff />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Review terms' }));
  await user.click(screen.getByRole('button', { name: 'Close dialog' }));
  await waitFor(() => expect(history.state?.kukuriBackLayer).toBeUndefined());
  await back();
  expect(location.hash).toBe('#/profile');
});
it.each(['Back', 'Close'])('settings %s closes one routed layer before the composer', async dismissal => {
  const harness = createShellHookHarness({ hash: '#/timeline?topic=kukuri%3Atopic%3Ageneral', router: true });
  const api = createDesktopMockApi();
  const settingsTriggerRef = { current: null };
  const pendingRouteUrlRef = { current: null as string | null };
  function View() {
    const [center, setCenter] = useState(false);
    useWebBackLayer(center, () => setCenter(false));
    const routing = useDesktopShellRouting({ api, translate: key => key, loadTopics: async () => {},
      settingsTriggerRef, pendingRouteUrlRef });
    const settings = useDesktopShellStore(state => state.shellChromeState.settingsOpen);
    return <>
      <ColumnComposerFooter active destinationLabel='general' locale='en' target={target}
        onActivate={() => undefined} onAttachmentSelection={async () => undefined}
        onAttachmentPaste={async () => undefined} onRemoveAttachment={() => undefined} onSubmit={async () => undefined} />
      <button onClick={() => setCenter(true)}>Open center</button>
      {center && <button onClick={() => { setCenter(false); routing.openSettingsSection('appearance', 'replace'); }}>Open settings</button>}
      {settings && <button onClick={() => routing.setSettingsOpen(false, true)}>Close settings</button>}
    </>;
  }
  render(<View />, { wrapper: harness.wrapper });
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Post to general' }));
  await user.type(screen.getByPlaceholderText('Write a post'), 'routed settings draft');
  await user.click(screen.getByRole('button', { name: 'Open center' }));
  await user.click(screen.getByRole('button', { name: 'Open settings' }));
  if (dismissal === 'Back') await back();
  else await user.click(screen.getByRole('button', { name: 'Close settings' }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Close settings' })).not.toBeInTheDocument());
  await waitFor(() => expect(location.hash).not.toContain('settings='));
  expect(screen.getByDisplayValue('routed settings draft')).toBeVisible();
  await back();
  await waitFor(() => expect(screen.queryByPlaceholderText('Write a post')).not.toBeInTheDocument());
  expect(location.hash).not.toContain('settings=');
});
it('settings handoff to profile connections is not undone by delayed history traversal', async () => {
  const harness = createShellHookHarness({ hash: '#/timeline?topic=kukuri%3Atopic%3Ageneral', router: true });
  const api = createDesktopMockApi();
  const settingsTriggerRef = { current: null };
  const pendingRouteUrlRef = { current: null as string | null };
  const view = renderHook(() => useDesktopShellRouting({ api, translate: key => key, loadTopics: async () => {},
    settingsTriggerRef, pendingRouteUrlRef }), { wrapper: harness.wrapper });
  act(() => view.result.current.openSettingsSection('account'));
  await waitFor(() => expect(location.hash).toContain('settings=account'));
  const traversal = vi.spyOn(history, 'go');
  act(() => {
    view.result.current.setSettingsOpen(false);
    view.result.current.focusPrimarySection('profile');
    view.result.current.openProfileConnections('following');
  });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 100)); });
  expect(location.hash).toContain('/profile');
  expect(location.hash).toContain('profileMode=connections');
  expect(harness.store.getState().shellChromeState.settingsOpen).toBe(false);
  expect(traversal).not.toHaveBeenCalled();
  traversal.mockRestore();
});
