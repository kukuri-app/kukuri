import type { DesktopApi } from '../types';
import type {
  ConnectivityPeersRequest,
  PeerPage,
  ScopeDisplayRequest,
  SessionDisplayRequest,
} from '../types.generated';
import { invokeDesktop } from '../invoke/desktop';
import { command } from '../invoke/dispatch';

// 表示の需要。session の表示と、開いている列の購読(#1221 R2-C。上限なら code `SCOPE_LIMIT_REACHED`)、
// 設定画面の詳細を開いたときに読む peer の一覧のページ(#1221 R2-D)。
export const displayDemandApi: Pick<
  DesktopApi,
  'setSessionDisplay' | 'setScopeDisplay' | 'listConnectivityPeers'
> = {
  setSessionDisplay: command('setSessionDisplay', async (request) =>
    invokeDesktop<void>('set_session_display', { request: request satisfies SessionDisplayRequest })),
  setScopeDisplay: command('setScopeDisplay', async (request) =>
    invokeDesktop<void>('set_scope_display', { request: request satisfies ScopeDisplayRequest })),
  listConnectivityPeers: command('listConnectivityPeers', async (request) =>
    invokeDesktop<PeerPage>('list_connectivity_peers', {
      request: request satisfies ConnectivityPeersRequest,
    })),
};
