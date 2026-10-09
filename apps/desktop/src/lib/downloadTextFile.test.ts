import { afterEach, expect, test, vi } from 'vitest';

import { downloadTextFile } from './downloadTextFile';

const platform = vi.hoisted(() => ({ android: false }));
const save = vi.hoisted(() => vi.fn<() => Promise<string | null>>());
const invokeDesktop = vi.hoisted(() => vi.fn<(command: string, args?: unknown) => Promise<void>>());

vi.mock('@/lib/platform', () => ({
  get IS_ANDROID() {
    return platform.android;
  },
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save }));
vi.mock('@/lib/api/invoke/desktop', () => ({ invokeDesktop }));

afterEach(() => {
  platform.android = false;
  save.mockReset();
  invokeDesktop.mockReset();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

test('downloadTextFile hands the text to an anchor download and releases the object URL', async () => {
  const createObjectURL = vi.fn<(blob: Blob) => string>(() => 'blob:kukuri/logs');
  const revokeObjectURL = vi.fn();
  vi.stubGlobal('URL', { ...URL, createObjectURL, revokeObjectURL });
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
    expect(this.download).toBe('kukuri-logs.txt');
    expect(this.href).toBe('blob:kukuri/logs');
  });

  await expect(downloadTextFile('kukuri-logs.txt', 'hello')).resolves.toBe(true);

  expect(click).toHaveBeenCalledTimes(1);
  const blob = createObjectURL.mock.calls[0]?.[0];
  expect(blob?.type).toBe('text/plain;charset=utf-8');
  expect(await blob?.text()).toBe('hello');
  expect(revokeObjectURL).toHaveBeenCalledWith('blob:kukuri/logs');
  expect(save).not.toHaveBeenCalled();
});

test('on Android the text is written to the location chosen on the save screen', async () => {
  platform.android = true;
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click');
  save.mockResolvedValue('content://downloads/document/7');
  invokeDesktop.mockResolvedValue(undefined);

  await expect(downloadTextFile('kukuri-diagnostics.txt', 'report')).resolves.toBe(true);

  expect(save).toHaveBeenCalledWith({ defaultPath: 'kukuri-diagnostics.txt' });
  expect(invokeDesktop).toHaveBeenCalledWith('write_text_document', {
    path: 'content://downloads/document/7',
    text: 'report',
  });
  expect(click).not.toHaveBeenCalled();
});

test('on Android cancelling the save screen writes nothing', async () => {
  platform.android = true;
  save.mockResolvedValue(null);

  await expect(downloadTextFile('kukuri-logs.txt', 'hello')).resolves.toBe(false);

  expect(invokeDesktop).not.toHaveBeenCalled();
});
