import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import i18n from '@/i18n';
import { CommunityNodeConsentDialog } from './CommunityNodeConsentDialog';
import type { CommunityNodeConsentPolicyView, CommunityNodeConsentView } from './types';

const ABUSE_BODY = [
  '# Abuse ポリシー',
  '',
  '> 注記: この文書は operator config から自動生成された下書きです。',
  '',
  '- 運営者: Example Operator',
  '- 連絡先: [窓口](https://example.com/contact)',
  '',
  '## 文書情報',
  '',
  '<script>window.__consentXss = true</script><img src=x onerror="window.__consentXss = true">',
  '',
  '[危険なリンク](javascript:alert(1))',
].join('\n');

function policy(overrides: Partial<CommunityNodeConsentPolicyView>): CommunityNodeConsentPolicyView {
  return {
    policySlug: 'terms_of_service',
    title: 'Terms of Service',
    body: 'You must follow the community node terms of service.',
    policyVersion: 1,
    policySnapshotRevision: 'snap-2',
    referenceTranslation: false,
    fallback: false,
    required: true,
    acceptedAtLabel: null,
    updated: false,
    previouslyAcceptedVersion: null,
    ...overrides,
  };
}

function consent(policies: CommunityNodeConsentPolicyView[]): CommunityNodeConsentView {
  return {
    loaded: true,
    loading: false,
    loadError: null,
    withdrawn: false,
    hasLocalConsent: false,
    allRequiredAccepted: false,
    hasPendingUpdate: policies.some((item) => item.updated),
    policies,
  };
}

function renderDialog(view: CommunityNodeConsentView, onAccept = vi.fn()) {
  render(
    <CommunityNodeConsentDialog
      open
      onOpenChange={() => {}}
      baseUrl='https://api.kukuri.app'
      consent={view}
      busy={false}
      onAccept={onAccept}
      onRetry={() => {}}
    />
  );
  return { dialog: screen.getByRole('dialog'), onAccept };
}

test('policies start collapsed with their title and consent state still listed', () => {
  const { dialog } = renderDialog(consent([
    policy({ policySlug: 'abuse', title: 'Abuse ポリシー', body: ABUSE_BODY }),
    policy({ policySlug: 'privacy', title: 'Privacy Policy', required: false }),
  ]));

  const abuse = within(dialog).getByRole('button', { name: /Abuse ポリシー/ });
  const privacy = within(dialog).getByRole('button', { name: /Privacy Policy/ });
  expect(abuse).toHaveAttribute('aria-expanded', 'false');
  expect(privacy).toHaveAttribute('aria-expanded', 'false');
  // #1192: 一括同意なので必須 / 任意は出さない。版と同意状況だけを見出し行に残す。
  expect(abuse).toHaveAccessibleDescription(/v1.*Not accepted/);
  expect(privacy).toHaveAccessibleDescription(/v1.*Not accepted/);
  expect(within(dialog).queryByText('Required')).not.toBeInTheDocument();
  expect(within(dialog).queryByText('Optional')).not.toBeInTheDocument();
  expect(within(dialog).queryByText('運営者: Example Operator')).not.toBeInTheDocument();
});

test('an expanded policy renders its Markdown without raw syntax, HTML, or unsafe links', async () => {
  const user = userEvent.setup();
  const { dialog } = renderDialog(consent([
    policy({ policySlug: 'abuse', title: 'Abuse ポリシー', body: ABUSE_BODY }),
  ]));

  const toggle = within(dialog).getByRole('button', { name: /Abuse ポリシー/ });
  await user.click(toggle);
  expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const panel = within(dialog).getByRole('region', { name: /Abuse ポリシー/ });

  expect(within(panel).getByRole('heading', { name: '文書情報' })).toBeInTheDocument();
  expect(within(panel).getByRole('list')).toBeInTheDocument();
  expect(within(panel).getByText('運営者: Example Operator').tagName).toBe('LI');
  expect(panel.querySelector('blockquote')).toHaveTextContent('注記: この文書は');
  expect(within(panel).getByRole('link', { name: '窓口' })).toHaveAttribute(
    'href',
    'https://example.com/contact'
  );
  expect(within(panel).queryByRole('link', { name: '危険なリンク' })).not.toBeInTheDocument();
  expect(panel).not.toHaveTextContent(/^#|## 文書情報|> 注記/);
  expect(panel.querySelector('script, img')).toBeNull();
  expect(panel).toHaveTextContent('<script>window.__consentXss = true</script>');
  expect((window as unknown as { __consentXss?: boolean }).__consentXss).toBeUndefined();

  await user.keyboard('{Enter}');
  expect(toggle).toHaveAttribute('aria-expanded', 'false');
  expect(within(dialog).queryByRole('region', { name: /Abuse ポリシー/ })).not.toBeInTheDocument();
  await user.keyboard(' ');
  expect(toggle).toHaveAttribute('aria-expanded', 'true');
});

test('collapsed policies remain part of the acceptance', async () => {
  const user = userEvent.setup();
  const { dialog, onAccept } = renderDialog(consent([
    policy({ policySlug: 'terms' }),
    policy({ policySlug: 'privacy', title: 'Privacy Policy' }),
  ]));

  await user.click(within(dialog).getByRole('button', { name: 'Accept' }));
  expect(onAccept).toHaveBeenCalledTimes(1);
});

test.each([
  ['en', 'The content was updated.', 'Updated from v1 to v2.'],
  ['ja', '内容が更新されました。', 'v1 から v2 に更新されました。'],
  ['zh-CN', '内容已更新。', '已从 v1 更新至 v2。'],
])('update detail distinguishes a same-version revision in %s', async (locale, sameVersion, newVersion) => {
  await i18n.changeLanguage(locale);
  const { dialog } = renderDialog(consent([
    policy({ policySlug: 'moderation', title: 'Moderation', updated: true, previouslyAcceptedVersion: 1 }),
    policy({ policySlug: 'terms', title: 'Terms', policyVersion: 2, updated: true, previouslyAcceptedVersion: 1 }),
  ]));

  expect(within(dialog).getByRole('button', { name: /Moderation/ })).toHaveAccessibleDescription(
    new RegExp(sameVersion.replace('.', '\\.'))
  );
  expect(within(dialog).getByRole('button', { name: /Terms/ })).toHaveAccessibleDescription(
    new RegExp(newVersion.replace(/\./g, '\\.'))
  );
  expect(dialog).not.toHaveTextContent(/v1 から v1|v1 to v1|v1 更新至 v1/);
});
