/// Android の Tauri の build か（Tauri CLI が build・dev の前の command に `TAURI_ENV_PLATFORM=android` を渡す）。
export const IS_ANDROID = import.meta.env.TAURI_ENV_PLATFORM === 'android';
