import { describe, expect, it } from 'vitest';

import {
  panelError,
  panelLoading,
  panelReady,
  removeRecordEntry,
  setRecordEntry,
  setTimelineCursorEntry,
  updateRecordEntry,
} from '@/shell/stateUpdates';

describe('stateUpdates', () => {
  it('内容が同じcursorなら表示状態を更新しない', () => {
    const stored = { created_at: 10, object_id: 'post-1' };
    const current = { topic: stored };
    expect(setTimelineCursorEntry('topic', { ...stored })(current)).toBe(current);
    expect(setTimelineCursorEntry('topic', { created_at: 9, object_id: 'post-0' })(current))
      .not.toBe(current);
  });
  it('setRecordEntry は変更時だけ新オブジェクトを返す', () => {
    const current = { a: 1, b: 2 };
    expect(setRecordEntry('a', 1)(current)).toBe(current);
    const next = setRecordEntry('a', 10)(current);
    expect(next).toEqual({ a: 10, b: 2 });
    expect(next).not.toBe(current);
    expect(current).toEqual({ a: 1, b: 2 });
  });

  it('updateRecordEntry は現在値(未定義含む)から導出する', () => {
    const current: Record<string, number> = { a: 1 };
    expect(updateRecordEntry<number>('a', (prev) => prev ?? 0)(current)).toBe(current);
    expect(updateRecordEntry<number>('a', (prev) => (prev ?? 0) + 1)(current)).toEqual({ a: 2 });
    expect(updateRecordEntry<number>('b', (prev) => (prev ?? 0) + 1)(current)).toEqual({
      a: 1,
      b: 1,
    });
  });

  it('removeRecordEntry はキーが無ければ current をそのまま返す(参照同一)', () => {
    const current = { a: 1 };
    expect(removeRecordEntry('missing')(current)).toBe(current);
    const next = removeRecordEntry('a')(current);
    expect(next).toEqual({});
    expect(next).not.toBe(current);
  });

  it('パネル状態コンストラクタは status と error を対で揃える', () => {
    expect(panelLoading()).toEqual({ status: 'loading', error: null });
    expect(panelReady()).toEqual({ status: 'ready', error: null });
    expect(panelError('boom')).toEqual({ status: 'error', error: 'boom' });
  });
});
