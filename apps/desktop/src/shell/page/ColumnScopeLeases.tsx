import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import type { DesktopApi } from '@/lib/api';
import {
  announceScopeLimit,
  useColumnScopeLeases,
  useScopeLimitNotice,
} from '@/shell/columnScopeLeases';
import { closeColumn, type ColumnState } from '@/shell/slices/workspace';
import {
  useDesktopShellFieldSetter,
  useDesktopShellStore,
  useDesktopShellStoreApi,
} from '@/shell/store';

// #1221 R2-C: 開いている列を購読の需要として登録し、上限を超えた列の追加・参加を取り消して説明する。
export function ColumnScopeLeases({
  api,
  onActivateColumn,
}: {
  api: DesktopApi;
  onActivateColumn: (column: ColumnState) => unknown;
}) {
  const { t } = useTranslation('shell');
  const columns = useDesktopShellStore((state) => state.workspaceState.columns);
  const storeApi = useDesktopShellStoreApi();
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const [kind, setKind] = useScopeLimitNotice();
  useColumnScopeLeases(api, columns, (columnId) => {
    const current = storeApi.getState().workspaceState;
    const next = closeColumn(current, columnId);
    if (next !== current) {
      setWorkspaceState(next);
      const active = next.columns.find((column) => column.id === next.activeColumnId);
      if (active && next.activeColumnId !== current.activeColumnId) void onActivateColumn(active);
    }
    announceScopeLimit('column');
  });
  return (
    <Dialog open={kind !== null} onOpenChange={(open) => { if (!open) setKind(null); }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('scopeLimit.title')}</DialogTitle>
          <DialogDescription>
            {t(kind === 'participation' ? 'scopeLimit.participation' : 'scopeLimit.column')}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button onClick={() => setKind(null)}>{t('scopeLimit.confirm')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
