import * as React from 'react';

import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { Card } from '@/components/ui/card';
import { IconButton } from '@/components/ui/icon-button';
import { cn } from '@/lib/utils';

import { type SettingsSection } from './types';

type SettingsDrawerSection = {
  id: SettingsSection;
  label: string;
  description: string;
  content: React.ReactNode;
  keepMounted?: boolean;
};

type SettingsDrawerProps = {
  drawerId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  activeSection: SettingsSection;
  onSectionChange: (section: SettingsSection) => void;
  sections: SettingsDrawerSection[];
};

export function SettingsDrawer({
  drawerId,
  open,
  onOpenChange,
  activeSection,
  onSectionChange,
  sections,
}: SettingsDrawerProps) {
  const { t } = useTranslation('shell');
  const currentSection = sections.find((section) => section.id === activeSection) ?? sections[0];
  const [visited, setVisited] = React.useState<SettingsSection[]>([]);
  React.useEffect(() => {
    if (currentSection.keepMounted) {
      setVisited((previous) => previous.includes(currentSection.id) ? previous : [...previous, currentSection.id]);
    }
  }, [currentSection.id, currentSection.keepMounted]);
  // #967: 短い window では nav が scroll するため、選択中 section の nav item を可視域へ入れる。
  // focus は動かさない(deep link / 診断導線の既存 focus 挙動を維持する)。
  React.useEffect(() => {
    if (!open) return;
    const item = document.getElementById(`${drawerId}-section-${currentSection.id}`);
    if (item && typeof item.scrollIntoView === 'function') {
      item.scrollIntoView({ block: 'nearest' });
    }
  }, [open, drawerId, currentSection.id]);

  return (
    <>
      <div
        className='shell-overlay-backdrop shell-settings-backdrop'
        data-open={open}
        onClick={() => onOpenChange(false)}
        aria-hidden='true'
      />
      <Card
        as='section'
        id={drawerId}
        className='shell-settings-drawer'
        data-open={open}
        role='dialog'
        aria-modal='true'
        aria-hidden={!open}
        aria-labelledby={`${drawerId}-title`}
      >
        <div className='shell-settings-toolbar'>
          <h2 id={`${drawerId}-title`} className='shell-pane-heading'>
            {t('settingsDrawer.title')}
          </h2>
          <IconButton
            className='shell-icon-button shell-settings-close'
            variant='ghost'
            type='button'
            label={t('settingsDrawer.close')}
            onClick={() => onOpenChange(false)}
          >
            <X className='size-5' aria-hidden='true' />
          </IconButton>
        </div>

        <div className='shell-settings-nav'>
          <nav aria-label={t('settingsDrawer.sectionsLabel')} className='shell-settings-nav-list'>
            {sections.map((section) => (
              <button
                key={section.id}
                id={`${drawerId}-section-${section.id}`}
                className={cn(
                  'shell-settings-nav-item',
                  activeSection === section.id && 'shell-settings-nav-item-active'
                )}
                type='button'
                aria-current={activeSection === section.id ? 'location' : undefined}
                data-testid={`settings-section-${section.id}`}
                onClick={() => onSectionChange(section.id)}
              >
                <span className='shell-primary-nav-label'>{section.label}</span>
              </button>
            ))}
          </nav>
        </div>

        <div className='shell-settings-body'>
          <div className='shell-settings-content'>
            {sections.filter(section => section.keepMounted && (section.id === currentSection.id || visited.includes(section.id))).map(section => (
              <div key={section.id} hidden={section.id !== currentSection.id}>
                {section.content}
              </div>
            ))}
            {!currentSection.keepMounted ? currentSection.content : null}
          </div>
        </div>
      </Card>
    </>
  );
}
