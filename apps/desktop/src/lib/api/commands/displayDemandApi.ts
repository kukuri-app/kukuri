import type { DesktopApi } from '../types';
import type { ScopeDisplayRequest, SessionDisplayRequest } from '../types.generated';
import { invokeDesktop } from '../invoke/desktop';
import { command } from '../invoke/dispatch';

// 表示の需要。session の表示と、開いている列の購読(#1221 R2-C。上限なら code `SCOPE_LIMIT_REACHED`)。
export const displayDemandApi: Pick<DesktopApi, 'setSessionDisplay' | 'setScopeDisplay'> = {
  setSessionDisplay: command('setSessionDisplay', async (request) =>
    invokeDesktop<void>('set_session_display', { request: request satisfies SessionDisplayRequest })),
  setScopeDisplay: command('setScopeDisplay', async (request) =>
    invokeDesktop<void>('set_scope_display', { request: request satisfies ScopeDisplayRequest })),
};
