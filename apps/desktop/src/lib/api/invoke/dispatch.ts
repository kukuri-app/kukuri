import i18n from '@/i18n';

import type { DesktopApi } from '../types';
import { InvokeError, normalizeInvokeError } from './error';

/// mock ビルド(dev / Storybook / Playwright / テスト)では window.__KUKURI_DESKTOP__ に
/// in-memory 実装が後から注入される。ディスパッチは**呼び出し時点**でその存在を見る
/// (モジュール初期化時に固定してはならない)。WP-H7 PR1 で 82 メソッド + 3 スタンドアロン
/// 関数に散っていた `if (window.__KUKURI_DESKTOP__)` 分岐をここへ集約した。

export function isDesktopMockActive(): boolean {
  return Boolean(window.__KUKURI_DESKTOP__);
}

/// #1219 W6: 新しいアクセスの配布を本人の別の端末が行うので、操作を保留した。共有リンクの作成と書き込みのどの操作で
/// 返っても、画面は 1 つのダイアログで示す(#1220 AC-3a)。
export const CONTROLLER_PENDING = 'PRIVATE_CHANNEL_CONTROLLER_PENDING';

const controllerPendingListeners = new Set<() => void>();

export function onControllerPending(listener: () => void): () => void {
  controllerPendingListeners.add(listener);
  return () => {
    controllerPendingListeners.delete(listener);
  };
}

/// 保留ならダイアログへ知らせ、操作の失敗の表示(投稿の card など)にも backend の文言でなく画面の文言を出す。
export function explainControllerPending(error: unknown): unknown {
  if (normalizeInvokeError(error).code !== CONTROLLER_PENDING) return error;
  for (const listener of controllerPendingListeners) listener();
  return new InvokeError(CONTROLLER_PENDING, i18n.t('channels:controllerPending.title'));
}

/// DesktopApi のメソッド 1 本を mock 対応でラップする。mock が居れば同名メソッドへ、
/// 居なければ invoke 実装(引数 → snake_case request 構築)へ、同じ引数のまま転送する。
/// 引数・戻り値の型は DesktopApi[K] から保存されるため、転送のミスは tsc が検出する。
export function command<K extends keyof DesktopApi>(name: K, impl: DesktopApi[K]): DesktopApi[K] {
  const dispatch = (...args: unknown[]) => {
    const mock = window.__KUKURI_DESKTOP__;
    // mock オブジェクトのメソッドとして呼ぶ(this を束縛する)。
    // 一部の mock 実装は内部で this.otherMethod() を呼ぶため、
    // 変数へ取り出して素の関数として呼ぶと this が外れて壊れる。
    const result = mock
      ? (mock[name] as (...forwarded: unknown[]) => unknown).apply(mock, args)
      : (impl as (...forwarded: unknown[]) => unknown)(...args);
    return result instanceof Promise
      ? result.catch((error: unknown) => {
          throw explainControllerPending(error);
        })
      : result;
  };
  return dispatch as DesktopApi[K];
}
