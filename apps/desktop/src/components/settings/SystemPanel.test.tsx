import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

const { getPreferenceMock, setPreferenceMock } = vi.hoisted(() => ({
  getPreferenceMock: vi.fn(),
  setPreferenceMock: vi.fn(),
}));

vi.mock('@/lib/api/windowClosePreference', () => ({
  getWindowClosePreference: getPreferenceMock,
  setWindowClosePreference: setPreferenceMock,
}));
vi.mock('@/lib/releaseReadiness', () => ({ isTauriRuntime: () => true }));

import { SystemPanel } from './SystemPanel';

beforeEach(() => {
  getPreferenceMock.mockReset();
  setPreferenceMock.mockReset();
});

test('loads and saves the window close behavior immediately', async () => {
  const user = userEvent.setup();
  getPreferenceMock.mockResolvedValue({ behavior: null });
  setPreferenceMock.mockResolvedValue({ behavior: 'tray' });
  render(<SystemPanel />);

  const field = await screen.findByRole('combobox', { name: 'When closing the window' });
  await waitFor(() => expect(field).toHaveValue('ask'));
  await user.selectOptions(field, 'tray');
  expect(setPreferenceMock).toHaveBeenCalledWith({ behavior: 'tray' });
  await waitFor(() => expect(field).toHaveValue('tray'));
});

test('restores the previous value when persistence fails', async () => {
  const user = userEvent.setup();
  getPreferenceMock.mockResolvedValue({ behavior: 'quit' });
  setPreferenceMock.mockRejectedValue(new Error('disk full'));
  render(<SystemPanel />);

  const field = await screen.findByRole('combobox', { name: 'When closing the window' });
  await waitFor(() => expect(field).toHaveValue('quit'));
  await user.selectOptions(field, 'tray');
  expect(await screen.findByText(/could not be saved/)).toBeVisible();
  expect(field).toHaveValue('quit');
});
