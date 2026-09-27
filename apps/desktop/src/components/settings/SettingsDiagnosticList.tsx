import { cn } from '@/lib/utils';

import { PeerPageList } from './PeerPageList';
import { type LoadConnectivityPeers, type SettingsDiagnosticItemView } from './types';

type SettingsDiagnosticListProps = {
  items: SettingsDiagnosticItemView[];
  columns?: 1 | 2;
  loadPeers?: LoadConnectivityPeers;
};

export function SettingsDiagnosticList({
  items,
  columns = 1,
  loadPeers,
}: SettingsDiagnosticListProps) {
  return (
    <dl
      className='min-w-0 max-w-full gap-3'
      style={
        columns === 2
          ? {
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fit, minmax(min(16rem, 100%), 1fr))',
            }
          : { display: 'grid' }
      }
    >
      {items.map((item) => (
        <div
          key={item.label}
          className='min-w-0 rounded-[18px] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] px-4 py-3 shadow-[var(--shadow-dropdown)]'
        >
          <dt className='text-[0.74rem] uppercase tracking-[0.08em] text-[var(--muted-foreground)]'>
            {item.label}
          </dt>
          <dd
            className={cn(
              'mt-2 min-w-0 [overflow-wrap:anywhere] text-sm leading-6 text-[var(--muted-foreground-soft)]',
              item.monospace && 'break-all font-mono text-[0.8rem]',
              item.tone === 'danger' && 'text-[var(--destructive)]'
            )}
          >
            {item.peers && loadPeers ? (
              <PeerPageList query={item.peers} loadPeers={loadPeers} />
            ) : (
              item.value
            )}
          </dd>
        </div>
      ))}
    </dl>
  );
}
