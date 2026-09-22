import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

import { App } from '@/App';
import { DESKTOP_THEME_STORAGE_KEY } from '@/lib/theme';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { DESKTOP_LOCALE_STORAGE_KEY } from '@/i18n';

beforeEach(() => {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    writable: true,
    value: 1024,
  });
  window.dispatchEvent(new Event('resize'));
  window.history.replaceState(null, '', '/');
  window.localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
  invokeMock.mockReset();
  delete window.__KUKURI_DESKTOP__;
});

test('desktop app bootstraps the shell with the default timeline workspace', async () => {
  render(<App api={createDesktopMockApi()} />);

  await waitFor(() => {
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  });
  const timelineColumn = screen.getByRole('region', { name: /^Timeline Column/ });
  expect(timelineColumn).toHaveTextContent('general');
  expect(within(timelineColumn).getByRole('button', { name: /^Post to / })).toBeInTheDocument();
  expect(screen.getByTestId('control-center-trigger')).toBeInTheDocument();
  expect(window.localStorage.getItem(DESKTOP_THEME_STORAGE_KEY)).toBe('dark');
});

test('desktop app restores a persisted light theme on boot', async () => {
  window.localStorage.setItem(DESKTOP_THEME_STORAGE_KEY, 'light');

  render(<App api={createDesktopMockApi()} />);

  await waitFor(() => {
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  });
});

test('settings drawer can open the release section', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await user.click(await screen.findByTestId('control-center-trigger'));
  await user.click(
    within(screen.getByRole('complementary', { name: 'Control Center' })).getByRole('button', {
      name: 'Settings',
    })
  );
  expect(screen.getByRole('dialog', { name: 'Settings' })).toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: 'Release' }));

  expect(screen.getByRole('heading', { name: 'Release' })).toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'External transmissions' })).toBeInTheDocument();
  expect(screen.getByText('Peers, the Mainline DHT, and configured relays')).toBeInTheDocument();
  expect(
    screen.getByText(
      'Copying or exporting a diagnostic report does not send it automatically. It leaves the device only if you attach it to a destination you choose.'
    )
  ).toBeInTheDocument();
  expect(screen.getByRole('link', { name: /Latest release/ })).toHaveAttribute(
    'href',
    'https://github.com/kukuri-app/kukuri/releases/latest'
  );
  expect(screen.getByRole('link', { name: /Privacy policy/ })).toHaveAttribute(
    'href',
    'https://api.kukuri.app/privacy'
  );
});

test('settings drawer exposes the window close behavior under System', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await user.click(await screen.findByTestId('control-center-trigger'));
  await user.click(
    within(screen.getByRole('complementary', { name: 'Control Center' })).getByRole('button', {
      name: 'Settings',
    })
  );
  await user.click(screen.getByRole('button', { name: 'System' }));

  expect(screen.getByRole('heading', { name: 'System' })).toBeVisible();
  expect(screen.getByRole('combobox', { name: 'When closing the window' })).toHaveValue('ask');
  expect(screen.getByRole('option', { name: 'Quit kukuri' })).toBeInTheDocument();
  expect(screen.getByRole('option', { name: 'Keep kukuri in the task tray' })).toBeInTheDocument();
});

// #857: startup gate は文書単位の同意状態を受け取る。
function consentDocuments(acceptedVersion: number | null) {
  return ['terms', 'privacy'].map((slug) => ({
    slug,
    currentVersion: 8,
    effectiveDate: '2026-09-19',
    authoritativeLanguage: 'ja',
    materialChange: true,
    controllerName: 'Preview Distributor',
    contact: 'privacy@example.test',
    acceptedVersion,
    acceptedAt: acceptedVersion === null ? null : 1_700_000_000,
    acceptedLanguage: acceptedVersion === null ? null : 'en',
    acceptedAppVersion: acceptedVersion === null ? null : '0.1.7',
  }));
}

// #858: 年齢自己申告の状態。null は未申告。
function ageAttestation(attestedVersion: number | null) {
  return {
    currentVersion: 1,
    attestedVersion,
    attestedAt: attestedVersion === null ? null : 1_700_000_000,
  };
}

test('language selection before consent preserves the age choice without any consent side effects', async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue({
    status: 'consent_required', documents: consentDocuments(null), age_attestation: ageAttestation(null),
  });
  render(<App />);
  const language = await screen.findByRole('combobox', { name: 'Language' });
  const checkbox = screen.getByRole('checkbox');
  await user.click(checkbox);
  await user.selectOptions(language, 'ja');
  expect(screen.getByRole('heading', { name: 'ご利用の前に' })).toBeVisible();
  expect(language).toHaveValue('ja');
  expect(checkbox).toBeChecked();
  expect(document.documentElement).toHaveAttribute('lang', 'ja');
  expect(localStorage.getItem(DESKTOP_LOCALE_STORAGE_KEY)).toBe('ja');
  expect(invokeMock.mock.calls.every(([command]) => command === 'get_desktop_startup_status')).toBe(true);
  await user.selectOptions(language, 'zh-CN');
  expect(screen.getByRole('heading', { name: '继续之前' })).toBeVisible();
  expect(document.documentElement).toHaveAttribute('lang', 'zh-CN');
  expect(checkbox).toBeChecked();
  expect(screen.queryByTestId('control-center-trigger')).not.toBeInTheDocument();
});

test('pending consent fixes the display language and retry sends the newly displayed language', async () => {
  const user = userEvent.setup();
  let rejectSave!: (error: Error) => void;
  invokeMock.mockImplementation((command: string) => {
    if (command === 'get_desktop_startup_status') return Promise.resolve({
      status: 'consent_required', documents: consentDocuments(null), age_attestation: ageAttestation(null),
    });
    if (command === 'accept_app_consents') return new Promise((_resolve, reject) => { rejectSave = reject; });
    throw new Error(`Unexpected consent effect: ${command}`);
  });
  render(<App />);
  const language = await screen.findByRole('combobox', { name: 'Language' });
  await user.selectOptions(language, 'ja');
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: '同意して続行' }));
  expect(language).toBeDisabled();
  await user.selectOptions(language, 'en');
  expect(language).toHaveValue('ja');
  expect(invokeMock).toHaveBeenLastCalledWith('accept_app_consents', {
    documents: [{slug:'terms',version:8},{slug:'privacy',version:8}], language:'ja', ageAttested:true,
  });
  rejectSave(new Error('storage unavailable'));
  await screen.findByText('同意の保存に失敗しました。もう一度お試しください。');
  expect(language).toBeEnabled();
  await user.selectOptions(language, 'en');
  expect(screen.getByRole('checkbox')).toBeChecked();
  await user.click(screen.getByRole('button', { name: 'Accept and continue' }));
  expect(invokeMock).toHaveBeenLastCalledWith('accept_app_consents', {
    documents: [{slug:'terms',version:8},{slug:'privacy',version:8}], language:'en', ageAttested:true,
  });
  rejectSave(new Error('retry result'));
  await screen.findByText('Failed to save your consent. Please try again.');
  expect(invokeMock.mock.calls.filter(([command]) => command === 'accept_app_consents')).toHaveLength(2);
});

test('failed language storage leaves a usable translated consent screen and permits retry', async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue({
    status:'consent_required', documents:consentDocuments(null), age_attestation:ageAttestation(null),
  });
  render(<App />);
  const language = await screen.findByRole('combobox', { name:'Language' });
  const original = Storage.prototype.setItem;
  const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key, value) {
    if (key === DESKTOP_LOCALE_STORAGE_KEY) throw new Error('quota exceeded');
    original.call(this, key, value);
  });
  try {
    await user.selectOptions(language, 'ja');
    expect(screen.getByRole('heading', { name:'ご利用の前に' })).toBeVisible();
    expect(language).toHaveAccessibleDescription(expect.stringContaining('保存できませんでした'));
    expect(localStorage.getItem(DESKTOP_LOCALE_STORAGE_KEY)).toBeNull();
  } finally {
    write.mockRestore();
  }
  await user.click(screen.getByRole('button', { name:'言語の保存を再試行' }));
  expect(localStorage.getItem(DESKTOP_LOCALE_STORAGE_KEY)).toBe('ja');
  expect(language).toHaveAccessibleDescription('変更はすぐに反映され、この端末に保存されます。');
  expect(invokeMock.mock.calls.filter(([command]) => command === 'accept_app_consents')).toHaveLength(0);
});

test('consent explains the missing age confirmation before any action', async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue({
    status: 'consent_required', documents: consentDocuments(null),
    age_attestation: ageAttestation(null),
  });
  render(<App />);
  const checkbox = await screen.findByRole('checkbox');
  const accept = screen.getByRole('button', { name: 'Age confirmation required' });
  const reason = 'To continue, check the box to confirm that you are 18 or older.';
  expect(screen.getByText(reason)).toBeVisible();
  expect(checkbox).toHaveAccessibleDescription(reason);
  expect(accept).toHaveAccessibleDescription(reason);
  expect(accept).toBeDisabled();
  await user.click(checkbox);
  expect(accept).toBeEnabled();
  expect(accept).toHaveAccessibleName('Accept and continue');
  expect(screen.queryByText(reason)).not.toBeInTheDocument();
  await user.click(checkbox);
  expect(accept).toBeDisabled();
  expect(accept).toHaveAccessibleName('Age confirmation required');
  expect(screen.getByText(reason)).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Decline' }));
  expect(invokeMock.mock.calls.filter(([command]) => command === 'accept_app_consents')).toHaveLength(0);
});

test('consent keeps the choice after a save error and suppresses repeated pending submissions', async () => {
  const user = userEvent.setup();
  let rejectSave!: (error: Error) => void;
  invokeMock.mockImplementation((command: string) => {
    if (command === 'get_desktop_startup_status') return Promise.resolve({
      status: 'consent_required', documents: consentDocuments(null),
      age_attestation: ageAttestation(null),
    });
    if (command === 'accept_app_consents') return new Promise((_resolve, reject) => { rejectSave = reject; });
    throw new Error(`Unexpected command before consent: ${command}`);
  });
  render(<App />);
  const checkbox = await screen.findByRole('checkbox');
  await user.click(checkbox);
  await user.dblClick(screen.getByRole('button', { name: 'Accept and continue' }));
  expect(screen.getByRole('button', { name: 'Applying…' })).toBeDisabled();
  expect(checkbox).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Decline' })).toBeDisabled();
  expect(invokeMock.mock.calls.filter(([command]) => command === 'accept_app_consents')).toHaveLength(1);
  rejectSave(new Error('storage unavailable'));
  expect(await screen.findByText('Failed to save your consent. Please try again.')).toBeVisible();
  expect(checkbox).toBeChecked();
  expect(checkbox).toBeEnabled();
  expect(screen.queryByTestId('control-center-trigger')).not.toBeInTheDocument();
  // 非readyの結果では復元frontend stateの取得も行わない。
  invokeMock.mockResolvedValueOnce({ status: 'initializing' });
  await user.click(screen.getByRole('button', { name: 'Accept and continue' }));
  expect(await screen.findByText('Checking startup status…')).toBeVisible();
  expect(invokeMock.mock.calls.filter(([command]) => command === 'accept_app_consents')).toHaveLength(2);
  expect(invokeMock).not.toHaveBeenCalledWith('get_pending_device_restore_frontend_state', undefined);
});

test('an outdated age attestation needs a new explicit choice, and remount discards an unsaved choice', async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue({
    status: 'consent_required', documents: consentDocuments(5), age_attestation: ageAttestation(0),
  });
  const first = render(<App />);
  await user.click(await screen.findByRole('checkbox'));
  expect(screen.getByRole('button', { name: 'Accept and continue' })).toBeEnabled();
  first.unmount();
  render(<App />);
  expect(await screen.findByRole('checkbox')).not.toBeChecked();
  expect(screen.getByRole('button', { name: 'Age confirmation required' })).toBeDisabled();
  expect(invokeMock.mock.calls.filter(([command]) => command === 'accept_app_consents')).toHaveLength(0);
});

test('a ready consent result applies pending restore frontend state before opening the shell', async () => {
  const user = userEvent.setup();
  let resolveRestore!: (value: null) => void;
  invokeMock.mockImplementation((command: string) => {
    if (command === 'get_desktop_startup_status') return Promise.resolve({
      status: 'consent_required', documents: consentDocuments(null), age_attestation: ageAttestation(null),
    });
    if (command === 'accept_app_consents') return Promise.resolve({ status: 'ready' });
    if (command === 'get_pending_device_restore_frontend_state') return new Promise((resolve) => { resolveRestore = resolve; });
    throw new Error(`Unexpected command before restore: ${command}`);
  });
  render(<App />);
  await user.click(await screen.findByRole('checkbox'));
  expect(invokeMock).not.toHaveBeenCalledWith('get_pending_device_restore_frontend_state', undefined);
  await user.click(screen.getByRole('button', { name: 'Accept and continue' }));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('get_pending_device_restore_frontend_state', undefined));
  expect(screen.queryByTestId('control-center-trigger')).not.toBeInTheDocument();
  window.__KUKURI_DESKTOP__ = createDesktopMockApi();
  resolveRestore(null);
  expect(await screen.findByTestId('control-center-trigger')).toBeInTheDocument();
});

test('desktop app blocks startup until app-level legal consent is accepted', async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValueOnce({
    status: 'consent_required',
    documents: consentDocuments(null),
    age_attestation: ageAttestation(null),
  });
  invokeMock.mockResolvedValueOnce({
    status: 'failed',
    error: {
      kind: 'unknown',
      message: 'kukuri could not open the local app database.',
      detail: 'runtime starts after consent',
      db_path: null,
    },
  });

  render(<App />);

  expect(await screen.findByRole('heading', { name: 'Before you continue' })).toBeInTheDocument();
  expect(screen.getByText('Terms of Service')).toBeInTheDocument();
  expect(screen.getByText('Privacy Policy')).toBeInTheDocument();
  expect(screen.getAllByText(/Effective date: 2026-09-19/)).toHaveLength(2);
  expect(
    screen.getAllByText(
      'This is a reference translation of the authoritative Japanese version. The Japanese version controls if there is any discrepancy.'
    )
  ).toHaveLength(2);
  expect(
    screen.queryByText('These documents are drafts and are not legal advice.')
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText(
      'This is a draft and is not legal advice. Final decisions should be made in consultation with appropriate experts or regulators.'
    )
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText(
      'The terms of service or privacy policy have been updated. Please review and accept again to continue.'
    )
  ).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Post' })).not.toBeInTheDocument();

  // #858: 年齢の自己申告チェックが無い間は同意ボタンが無効。
  const acceptButton = screen.getByRole('button', { name: 'Age confirmation required' });
  expect(acceptButton).toBeDisabled();
  expect(screen.getByText('I am 18 years of age or older.')).toBeInTheDocument();
  await user.click(screen.getByTestId('age-attestation-checkbox'));
  expect(acceptButton).toBeEnabled();

  await user.click(acceptButton);

  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith('accept_app_consents', {
      documents: [
        { slug: 'terms', version: 8 },
        { slug: 'privacy', version: 8 },
      ],
      language: 'en',
      ageAttested: true,
    });
  });
  expect(await screen.findByText('kukuri could not open the local database.')).toBeInTheDocument();
  expect(screen.getByDisplayValue(/runtime starts after consent/)).toBeInTheDocument();
});

test('desktop app requires renewed consent for legal bundle version 8 (#1174)', async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValueOnce({
    status: 'consent_required',
    documents: consentDocuments(7),
    age_attestation: ageAttestation(1),
  });
  invokeMock.mockResolvedValueOnce({
    status: 'failed',
    error: {
      kind: 'unknown',
      message: 'kukuri could not open the local app database.',
      detail: 'runtime starts only after renewed consent',
      db_path: null,
    },
  });

  render(<App />);

  expect(
    await screen.findAllByText(
      'The terms of service or privacy policy have been updated. Please review and accept again to continue.'
    )
  ).toHaveLength(1);
  expect(
    screen.queryByText('These documents are drafts and are not legal advice.')
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText(
      'This is a draft and is not legal advice. Final decisions should be made in consultation with appropriate experts or regulators.'
    )
  ).not.toBeInTheDocument();
  expect(screen.getAllByText('v8')).toHaveLength(2);
  expect(screen.queryByTestId('control-center-trigger')).not.toBeInTheDocument();

  // #858: 現行版で申告済みならチェックボックスは再表示されず、ボタンは有効のまま。
  expect(screen.queryByTestId('age-attestation-checkbox')).not.toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: 'Accept and continue' }));

  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith('accept_app_consents', {
      documents: [
        { slug: 'terms', version: 8 },
        { slug: 'privacy', version: 8 },
      ],
      language: 'en',
      ageAttested: false,
    });
  });
  expect(await screen.findByText('kukuri could not open the local database.')).toBeInTheDocument();
});

test('desktop app renders a startup error when the local database cannot be opened', async () => {
  invokeMock.mockResolvedValueOnce({
    status: 'failed',
    error: {
      kind: 'database_migration',
      message: 'kukuri could not open the local app database.',
      detail: 'migration checksum mismatch',
      db_path: 'C:\\Users\\tester\\AppData\\Roaming\\kukuri\\kukuri.db',
    },
  });

  render(<App />);

  expect(await screen.findByText('kukuri could not open the local database.')).toBeInTheDocument();
  expect(screen.getByText('Migration failure')).toBeInTheDocument();
  expect(screen.getByDisplayValue(/migration checksum mismatch/)).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Post' })).not.toBeInTheDocument();
});

test('desktop app keeps the startup screen visible while the native runtime initializes', async () => {
  invokeMock.mockResolvedValueOnce({ status: 'initializing' });
  invokeMock.mockResolvedValueOnce({
    status: 'failed',
    error: {
      kind: 'unknown',
      message: 'kukuri could not finish desktop startup.',
      detail: 'background initialization completed with an error',
      db_path: null,
    },
  });

  render(<App />);

  expect(await screen.findByText('Checking startup status…')).toBeInTheDocument();
  expect(await screen.findByText('kukuri could not open the local database.')).toBeInTheDocument();
  expect(invokeMock).toHaveBeenCalledTimes(2);
  expect(invokeMock).toHaveBeenNthCalledWith(1, 'get_desktop_startup_status', undefined);
  expect(invokeMock).toHaveBeenNthCalledWith(2, 'get_desktop_startup_status', undefined);
});
