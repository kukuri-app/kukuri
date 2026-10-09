import { IS_WEB_RUNTIME } from './webRuntime';

/// Android の Tauri の build か（Tauri CLI が build・dev の前の command に `TAURI_ENV_PLATFORM=android` を渡す）。
export const IS_ANDROID = import.meta.env.TAURI_ENV_PLATFORM === 'android';

/// live・game・metaverse・Dome の入口を出せる build か。Web（ADR 0060 §3）と Android（#1193 D1）では開発者モードでも出さない。
export const EXTENDED_FEATURES_AVAILABLE = !IS_WEB_RUNTIME && !IS_ANDROID;
