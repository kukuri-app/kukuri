import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { waitFor } from '@testing-library/react';
import { createDesktopShellStore } from '@/shell/store';
import { createColumnDraft, columnDraftKey } from '@/shell/slices/columnDrafts';
import { COLUMN_DRAFT_STORAGE_KEY, startColumnDraftPersistence } from '@/shell/columnDraftPersistence';
import { applyPwaShell, startPwaShell, usePwaShellStore } from './pwaShell';

let stop: () => void;
let stopDrafts: () => void;
const waiting = { postMessage: vi.fn() };
const target = { columnId: 'test-column', action: 'post' as const, scope: { topicId: 'test-topic', channelId: null } };
const key = columnDraftKey(target);
let store: ReturnType<typeof createDesktopShellStore>;
let meta: HTMLMetaElement;

beforeEach(async () => {
  localStorage.clear(); waiting.postMessage.mockReset();
  usePwaShellStore.setState({ ready: false, available: false, applying: false, checking: false, error: null });
  meta = document.createElement('meta'); meta.name = 'kukuri-shell-build'; meta.content = 'one'; document.head.append(meta);
  const active = { postMessage: (message: { type: string }, ports: MessagePort[]) => {
    ports[0].postMessage(message.type === 'version' ? 'one' : 'retained'); ports[0].close();
  } };
  const registration = Object.assign(new EventTarget(), { active, waiting, installing: null });
  const serviceWorker = Object.assign(new EventTarget(), {
    getRegistration: async () => registration, ready: Promise.resolve(registration),
  });
  vi.stubGlobal('navigator', { serviceWorker, onLine: false });
  stop = startPwaShell();
  await waitFor(() => expect(usePwaShellStore.getState().available).toBe(true));
  store = createDesktopShellStore();
  store.setState({ columnDraftsByKey: { [key]: { ...createColumnDraft(target), content: 'current unsaved text' } } });
  stopDrafts = startColumnDraftPersistence(store, localStorage);
  localStorage.setItem('test-identity', 'same identity');
  localStorage.setItem('test-settings', 'same settings');
});
afterEach(() => { stopDrafts(); stop(); meta.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

test('a failed draft write prevents worker activation and keeps input, identity, and settings', async () => {
  const write = Storage.prototype.setItem;
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (name, value) {
    if (name === COLUMN_DRAFT_STORAGE_KEY) throw new Error('quota');
    write.call(this, name, value);
  });
  await applyPwaShell();
  expect(waiting.postMessage).not.toHaveBeenCalled();
  expect(usePwaShellStore.getState().error).toBe('apply');
  expect(store.getState().columnDraftsByKey[key].content).toBe('current unsaved text');
  expect(localStorage.getItem('test-identity')).toBe('same identity');
  expect(localStorage.getItem('test-settings')).toBe('same settings');
});

test('the latest draft is committed before explicit activation, and an activation failure keeps the draft writer live', async () => {
  expect(waiting.postMessage).not.toHaveBeenCalled();
  waiting.postMessage.mockImplementation(() => {
    expect(localStorage.getItem(COLUMN_DRAFT_STORAGE_KEY)).toContain('current unsaved text');
    throw new Error('worker activation failed');
  });
  await applyPwaShell();
  expect(waiting.postMessage).toHaveBeenCalledWith({ type: 'apply' });
  expect(usePwaShellStore.getState().error).toBe('apply');
  store.setState({ columnDraftsByKey: { [key]: { ...store.getState().columnDraftsByKey[key], content: 'edited after failure' } } });
  window.dispatchEvent(new Event('pagehide'));
  expect(localStorage.getItem(COLUMN_DRAFT_STORAGE_KEY)).toContain('edited after failure');
});
