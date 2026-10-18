import { useSyncExternalStore } from 'react';

/// 重ねる表示の設定(#1841 判断 5)。端末に 1 組だけ保存し、すべての Live とライブの列で共通に使う。他の利用者へは送らない。
export type LiveOverlaySettings = {
  posts: boolean;
  reactions: boolean;
  position: 'left' | 'right';
  text: 'small' | 'medium' | 'large';
  reactionSize: 'small' | 'medium' | 'large';
};

export const LIVE_OVERLAY_SIZES = ['small', 'medium', 'large'] as const;
export const LIVE_OVERLAY_POSITIONS = ['left', 'right'] as const;

const KEY = 'kukuri.desktop.live-overlay';
const DEFAULTS: LiveOverlaySettings = {
  posts: true,
  reactions: true,
  position: 'left',
  text: 'medium',
  reactionSize: 'medium',
};

let current: LiveOverlaySettings | null = null;
const listeners = new Set<() => void>();

const pick = <T extends string>(value: unknown, allowed: readonly T[], fallback: T): T =>
  allowed.includes(value as T) ? (value as T) : fallback;

/// 保存した値を読む。読めない・形の違う欄は既定値にする(保存できない環境でも表示は既定値で続く)。
function read(): LiveOverlaySettings {
  if (current) return current;
  let saved: Record<string, unknown> = {};
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    if (parsed && typeof parsed === 'object') saved = parsed as Record<string, unknown>;
  } catch {
    // 既定値で続ける。
  }
  current = {
    posts: typeof saved.posts === 'boolean' ? saved.posts : DEFAULTS.posts,
    reactions: typeof saved.reactions === 'boolean' ? saved.reactions : DEFAULTS.reactions,
    position: pick(saved.position, LIVE_OVERLAY_POSITIONS, DEFAULTS.position),
    text: pick(saved.text, LIVE_OVERLAY_SIZES, DEFAULTS.text),
    reactionSize: pick(saved.reactionSize, LIVE_OVERLAY_SIZES, DEFAULTS.reactionSize),
  };
  return current;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

function update(patch: Partial<LiveOverlaySettings>) {
  current = { ...read(), ...patch };
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    // 保存できなくても、開いている表示には反映する。
  }
  listeners.forEach((listener) => listener());
}

/// 開いているすべての重ねる表示が同じ設定を読み、変えると全部に反映する。
export function useLiveOverlaySettings() {
  return [useSyncExternalStore(subscribe, read), update] as const;
}

/// 試験だけ: 覚えた値を捨てて、次に保存した値から読み直す。
export function resetLiveOverlaySettingsForTest() {
  current = null;
}
