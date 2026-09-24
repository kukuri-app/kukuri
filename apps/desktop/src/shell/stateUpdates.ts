import type { AsyncPanelState } from '@/shell/store';
import type { TimelineCursor } from '@/lib/api';

export function sameTimelineCursor(left: TimelineCursor | null | undefined,
  right: TimelineCursor | null | undefined): boolean {
  return left?.created_at === right?.created_at && left?.object_id === right?.object_id;
}

export function setTimelineCursorEntry(key: string, value: TimelineCursor | null) {
  return (current: Record<string, TimelineCursor | null>) =>
    Object.prototype.hasOwnProperty.call(current, key) && sameTimelineCursor(current[key], value)
      ? current : { ...current, [key]: value };
}

// Record 状態(`Record<string, V>`)の 1 キー更新・削除の定型(WP-H6 PR1)。
// shell の状態更新はこの形が大半で、各所で spread / delete を手書きしていた。
// setter にそのまま渡せる updater を返す。

/**
 * 1 キーを値で差し替える updater。同じ値なら元のオブジェクトを返す。
 *
 * `NoInfer` により V は渡した値からではなく setter の文脈(状態フィールドの型)から
 * 推論される(`null` やリテラルを渡したとき V が狭く固定されるのを防ぐ)。
 */
export function setRecordEntry<V>(
  key: string,
  value: NoInfer<V>
): (current: Record<string, V>) => Record<string, V> {
  return (current) =>
    Object.prototype.hasOwnProperty.call(current, key) && Object.is(current[key], value)
      ? current
      : { ...current, [key]: value as V };
}

/** 1 キーを「現在値からの導出」で差し替える updater。V は文脈から推論(上と同じ理由)。 */
export function updateRecordEntry<V>(
  key: string,
  update: (prev: NoInfer<V> | undefined) => NoInfer<V>
): (current: Record<string, V>) => Record<string, V> {
  return (current) => {
    const nextValue = update(current[key]);
    return Object.prototype.hasOwnProperty.call(current, key) && Object.is(current[key], nextValue)
      ? current
      : { ...current, [key]: nextValue as V };
  };
}

/**
 * 1 キーを削除する updater。キーが無ければ **current をそのまま返す**
 * (参照同一性を保ち、不要な再レンダーを起こさない。従来の手書き delete と同じ)。
 */
export function removeRecordEntry<V>(
  key: string
): (current: Record<string, V>) => Record<string, V> {
  return (current) => {
    if (!(key in current)) {
      return current;
    }
    const next = { ...current };
    delete next[key];
    return next;
  };
}

// AsyncPanelState({ status, error })の定型コンストラクタ。
// 「loading にするとき error を消し忘れる」类のずれを防ぐ。

export function panelLoading(): AsyncPanelState {
  return { status: 'loading', error: null };
}

export function panelReady(): AsyncPanelState {
  return { status: 'ready', error: null };
}

export function panelError(error: string): AsyncPanelState {
  return { status: 'error', error };
}
