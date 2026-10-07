import { render, screen } from '@testing-library/react';
import { describe, expect, test } from 'vitest';

import { LegalDocumentView } from '@/components/LegalDocumentView';
import i18n, { type SupportedLocale } from '@/i18n';

const EXPECTED_TERMS_HEADINGS: Array<{
  locale: SupportedLocale;
  headings: string[];
  accountBoundary: RegExp;
  accountKey: RegExp;
  thirdPartyBoundary: RegExp;
}> = [
  {
    locale: 'ja',
    headings: [
      '投稿コンテンツの権利帰属',
      '必要な権利または許諾の保有表明',
      '投稿者の責任',
      '限定的な利用許諾',
      '投稿撤回後の取扱い',
      '許諾に含まれない利用',
      '通報・利用制限',
      'アカウントを識別する鍵',
      'コミュニティノード',
      'サービスの変更・中断・終了',
      'オープンソースライセンス',
      '責任の制限',
      '準拠法・合意管轄',
    ],
    accountBoundary: /プロフィール、公開フォロー関係、公開投稿/u,
    accountKey: /アカウントを識別する秘密鍵/u,
    thirdPartyBoundary: /完全に遠隔消去することはできません/u,
  },
  {
    locale: 'en',
    headings: [
      'Ownership of your content',
      'Your rights and permissions',
      'Your responsibilities',
      'Limited technical license',
      'After withdrawal',
      'Uses not licensed',
      'Reports and restrictions',
      'Account-identifying key',
      'Community Nodes',
      'Changes, interruption, and termination',
      'Open-source license',
      'Limitation of liability',
      'Governing law and agreed jurisdiction',
    ],
    accountBoundary: /Public profiles, public follows, public posts/u,
    accountKey: /account-identifying secret key/u,
    thirdPartyBoundary: /cannot completely erase a copy/u,
  },
  {
    locale: 'zh-CN',
    headings: [
      '投稿内容的权利归属',
      '您对权利与许可的声明',
      '发布者的责任',
      '有限的技术性许可',
      '撤回投稿后的处理',
      '不在许可范围内的使用',
      '举报与使用限制',
      '用于识别账号的密钥',
      '社区节点',
      '服务的变更、中断与终止',
      '开源软件许可',
      '责任限制',
      '准据法与约定管辖',
    ],
    accountBoundary: /公开资料、公开关注关系、公开帖子/u,
    accountKey: /用于识别账号的私密密钥/u,
    thirdPartyBoundary: /无法彻底删除其他参与者已经取得的副本/u,
  },
];

describe.each(EXPECTED_TERMS_HEADINGS)('legal document in $locale', ({ locale, headings }) => {
  test('shows bundle version 9, effective date, and every required terms clause', async () => {
    await i18n.changeLanguage(locale);
    render(
      <LegalDocumentView
        documentVersions={{ terms: 9, privacy: 9 }}
        documentMetadata={{
          terms: {
            effectiveDate: '2026-10-07',
            authoritativeLanguage: 'ja',
            materialChange: true,
            controllerName: 'Preview Distributor',
            contact: 'privacy@example.test',
          },
          privacy: {
            effectiveDate: '2026-10-07',
            authoritativeLanguage: 'ja',
            materialChange: true,
            controllerName: 'Preview Distributor',
            contact: 'privacy@example.test',
          },
        }}
      />
    );

    expect(screen.getAllByText('v9')).toHaveLength(2);
    expect(screen.getAllByText(/2026-10-07/u)).toHaveLength(2);
    expect(screen.getAllByText(/Preview Distributor/u)).toHaveLength(2);
    expect(screen.getAllByText(/privacy@example\.test/u)).toHaveLength(2);
    if (locale === 'ja') {
      expect(
        screen.queryByText(
          "この表示は日本語正文に基づく参考訳です。差異がある場合は日本語正文を優先します。",
        ),
      ).not.toBeInTheDocument();
    } else {
      const referenceNotice =
        locale === 'en'
          ? 'This is a reference translation of the authoritative Japanese version. The Japanese version controls if there is any discrepancy.'
          : '此内容为日文正式文本的参考译文。如有差异，以日文版为准。';
      expect(screen.getAllByText(referenceNotice)).toHaveLength(2);
    }
    for (const heading of headings) {
      expect(screen.getByRole('heading', { name: heading })).toBeInTheDocument();
    }
  });

  test('uses concrete account terms and preserves the third-party disclosure boundary', async () => {
    const expected = EXPECTED_TERMS_HEADINGS.find((item) => item.locale === locale)!;
    await i18n.changeLanguage(locale);
    render(<LegalDocumentView documentVersions={{ terms: 9, privacy: 9 }} />);

    expect(screen.getByText(expected.accountBoundary)).toBeInTheDocument();
    expect(screen.getByText(expected.accountKey)).toBeInTheDocument();
    expect(screen.getByText(expected.thirdPartyBoundary)).toBeInTheDocument();
  });

  test('does not render a draft notice', async () => {
    await i18n.changeLanguage(locale);
    render(<LegalDocumentView documentVersions={{ terms: 9, privacy: 9 }} />);

    expect(screen.queryByText(/draft|ドラフト|草案/i)).not.toBeInTheDocument();
  });
});
