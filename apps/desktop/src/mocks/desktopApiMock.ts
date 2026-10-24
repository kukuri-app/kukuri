import { type DesktopApi } from '@/lib/api';

import { type DesktopMockApiOptions } from './desktopMockModel';
import { createMockRuntime } from './mockRuntime';
import { createChannelsMock } from './api/channels';
import { createConnectivityMock } from './api/connectivity';
import { createDeveloperLogsMock } from './api/developerLogs';
import { createDirectMessagesMock } from './api/directMessages';
import { createLiveGameMock } from './api/liveGame';
import { createNotificationsMock } from './api/notifications';
import { createMediaBookmarksMock } from './api/mediaBookmarks';
import { createPostsMock } from './api/posts';
import { createProfileSocialMock } from './api/profileSocial';
import { createReactionsMock } from './api/reactions';

export type { DesktopMockApiOptions } from './desktopMockModel';

// 実行時は 1 個のオブジェクト。ドメイン別ファイル(api/*)が返す部分実装を
// 共有 runtime の上で合成する(WP-H7 PR2)。importChannelAccessToken が内部で
// this.importFriendOnlyGrant を呼ぶ等の相互参照は、全メソッドがこの単一 api に
// 集約されるため this 経由で解決される。
export function createDesktopMockApi(options?: DesktopMockApiOptions): DesktopApi {
  const runtime = createMockRuntime(options);
  const api: DesktopApi = {
    ...createPostsMock(runtime),
    ...createMediaBookmarksMock(runtime),
    ...createReactionsMock(runtime),
    ...createProfileSocialMock(runtime),
    ...createNotificationsMock(runtime),
    ...createDirectMessagesMock(runtime),
    ...createLiveGameMock(runtime),
    ...createChannelsMock(runtime),
    ...createConnectivityMock(runtime),
    ...createDeveloperLogsMock(),
  };
  return api;
}
