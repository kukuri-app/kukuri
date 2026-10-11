import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { startPwaInstall, usePwaInstallStore } from '@/lib/pwaInstall';
import { PwaInstallNotice } from './PwaInstallNotice';

let stop: () => void;
let standalone = false;
beforeEach(() => {
  standalone = false;
  usePwaInstallStore.setState({ phase: 'unavailable' });
  vi.stubGlobal('matchMedia', () => ({
    matches: standalone, addEventListener: vi.fn(), removeEventListener: vi.fn(),
  }));
  stop = startPwaInstall();
});
afterEach(() => { stop(); vi.unstubAllGlobals(); });

function offer(outcome: 'accepted' | 'dismissed', prompt = vi.fn().mockResolvedValue(undefined)) {
  const event = Object.assign(new Event('beforeinstallprompt', { cancelable: true }), {
    prompt, userChoice: Promise.resolve({ outcome }),
  });
  act(() => { window.dispatchEvent(event); });
  expect(event.defaultPrevented).toBe(true);
  return prompt;
}

test('captures an offer before settings opens, prompts only on click, and preserves use after dismissal', async () => {
  const prompt = offer('dismissed');
  render(<PwaInstallNotice />);
  expect(prompt).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Add to home screen' }));
  expect(prompt).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(screen.getByText(/browser menu/i)).toBeInTheDocument();
  // 同じpromptを使い回さず、Chromeが新しく提供したときに再び選べる。
  const next = offer('accepted');
  await userEvent.click(screen.getByRole('button', { name: 'Add to home screen' }));
  expect(next).toHaveBeenCalledTimes(1);
});

test('does not report installed from acceptance alone, then reflects the browser installation event', async () => {
  offer('accepted');
  render(<PwaInstallNotice />);
  await userEvent.click(screen.getByRole('button', { name: 'Add to home screen' }));
  expect(screen.queryByText('Added to home screen.')).not.toBeInTheDocument();
  act(() => { window.dispatchEvent(new Event('appinstalled')); });
  expect(screen.getByText('Added to home screen.')).toBeInTheDocument();
});

test('keeps a failed installation optional and exposes the browser menu', async () => {
  offer('dismissed', vi.fn().mockRejectedValue(new Error('prompt failed')));
  render(<PwaInstallNotice />);
  await userEvent.click(screen.getByRole('button', { name: 'Add to home screen' }));
  expect(screen.getByText(/Could not open/i)).toBeInTheDocument();
  expect(screen.getByText('If supported, add kukuri from the browser menu.')).toBeInTheDocument();
});

test('standalone launch shows the installed state without offering another prompt', () => {
  stop(); standalone = true; stop = startPwaInstall();
  render(<PwaInstallNotice />);
  expect(screen.getByText('Added to home screen.')).toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

test('blocks repeated clicks while the browser owns the installation prompt', async () => {
  offer('dismissed', vi.fn(() => new Promise(() => {})));
  render(<PwaInstallNotice />);
  await userEvent.click(screen.getByRole('button', { name: 'Add to home screen' }));
  expect(screen.getByRole('button', { name: 'Waiting for your choice…' })).toBeDisabled();
});
