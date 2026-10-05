import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { LegalDocumentView } from '@/components/LegalDocumentView';
import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';
import { getAppConsentStatus, type AppConsentStatus } from '@/lib/api';
import { getResolvedLocale } from '@/i18n/format';
import { IS_WEB_RUNTIME } from '@/lib/webRuntime';
import { useAppUpdateStore } from '@/shell/useAppUpdateStore';

import { SettingsDiagnosticList } from './SettingsDiagnosticList';

function formatAcceptedAt(value: number | null | undefined, locale: string): string | null {
  if (!value) {
    return null;
  }
  return new Intl.DateTimeFormat(getResolvedLocale(locale), {
    dateStyle: 'medium',
    timeStyle: 'medium',
  }).format(new Date(value * 1000));
}

export function AboutPanel() {
  const { t, i18n } = useTranslation(['legal', 'common']);
  const currentVersion = useAppUpdateStore((state) => state.updateState.currentVersion);
  const [consentStatus, setConsentStatus] = useState<AppConsentStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    void getAppConsentStatus()
      .then((status) => {
        if (!disposed) {
          setConsentStatus(status);
          setError(null);
        }
      })
      .catch((caught) => {
        if (!disposed) {
          setError(caught instanceof Error ? caught.message : String(caught));
        }
      });
    return () => {
      disposed = true;
    };
  }, []);

  const locale = i18n.resolvedLanguage ?? i18n.language;
  // #857: 同意状態は文書単位で表示する(対象文書・版・日時)。
  const diagnostics = useMemo(
    () => [
      {
        label: t('legal:about.appVersionLabel'),
        value: currentVersion,
        monospace: true,
      },
      ...(consentStatus?.documents ?? []).map((document) => {
        const acceptedAt = formatAcceptedAt(document.acceptedAt, locale);
        return {
          label: t(`legal:documents.${document.slug}.title`),
          value:
            document.acceptedVersion !== null && acceptedAt
              ? t('legal:about.acceptedValue', {
                  version: document.acceptedVersion,
                  acceptedAt,
                })
              : t('legal:about.notAccepted'),
        };
      }),
      // #858: 年齢の自己申告は文書同意とは別状態として表示する。
      ...(consentStatus
        ? [
            {
              label: t('legal:about.ageAttestationHeading'),
              value: (() => {
                const attestedAt = formatAcceptedAt(
                  consentStatus.ageAttestation.attestedAt,
                  locale
                );
                return consentStatus.ageAttestation.attestedVersion !== null && attestedAt
                  ? t('legal:about.ageAttestedValue', { attestedAt })
                  : t('legal:about.notAttested');
              })(),
            },
          ]
        : []),
    ],
    [consentStatus, currentVersion, locale, t]
  );

  return (
    <Card className='min-w-0 space-y-5'>
      <CardHeader>
        <h3>{t('legal:about.title')}</h3>
        <small>{t('legal:about.summary')}</small>
      </CardHeader>
      {error ? <Notice tone='destructive'>{error}</Notice> : null}
      {IS_WEB_RUNTIME ? <Notice>{t('legal:about.webUnavailable')}</Notice> : null}
      <SettingsDiagnosticList items={diagnostics} columns={2} />
      <section className='min-w-0 space-y-4'>
        <h4 className='text-base font-semibold text-foreground'>
          {t('legal:about.documentsHeading')}
        </h4>
        {consentStatus ? (
          <LegalDocumentView
            documentVersions={Object.fromEntries(
              consentStatus.documents.map((document) => [
                document.slug,
                document.currentVersion,
              ])
            )}
            documentMetadata={Object.fromEntries(
              consentStatus.documents.map((document) => [
                document.slug,
                {
                  effectiveDate: document.effectiveDate,
                  authoritativeLanguage: document.authoritativeLanguage,
                  materialChange: document.materialChange,
                  controllerName: document.controllerName,
                  contact: document.contact,
                },
              ])
            )}
            compact
          />
        ) : null}
      </section>
    </Card>
  );
}
