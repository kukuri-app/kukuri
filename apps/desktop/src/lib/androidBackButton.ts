import { onBackButtonPress } from '@tauri-apps/api/app';

declare global {
  interface Window {
    /** Android の `MainActivity` が WebView へ渡す橋渡し（#1198 AC-2）。 */
    kukuriAndroid?: { moveTaskToBack(): void };
  }
}

const backButtonEscapes = new WeakSet<Event>();

/// Android の戻るが重なった層を閉じるために送った Escape か。履歴に積んだ設定・スレッド・著者は、この Escape では
/// 閉じず、履歴を戻って閉じる（Escape の replace で閉じると、戻る先に同じ画面の履歴が残る）。
export const isBackButtonEscape = (event: Event) => backButtonEscapes.has(event);

function dispatchEscape(target: Element, fromBackButton: boolean) {
  const event = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
  if (fromBackButton) backButtonEscapes.add(event);
  target.dispatchEvent(event);
  return event.defaultPrevented;
}

/// Android の戻る（#1198 AC-2）。上に重なったメニュー・ダイアログ・コントロールセンター・投稿欄を Esc と同じく 1 つ
/// 閉じ、無ければ画面の履歴を戻る。履歴が無ければ、起動時に復元した設定・スレッド・著者を閉じ、それも無ければ
/// アプリを終了せず背景へ移す。
export function handleBackButton(canGoBack: boolean) {
  const target = document.activeElement ?? document.body;
  // 投稿欄は focus が他へ移っていても（起動時に開いたまま復元した場合など）、表示中の列で開いていれば閉じる。
  const composer = document.querySelector('[aria-current="true"] .shell-column-composer');
  if (dispatchEscape(target, true) || (composer !== null && dispatchEscape(composer, true))) return;
  if (canGoBack) {
    window.history.back();
  } else if (!dispatchEscape(target, false)) {
    window.kukuriAndroid?.moveTaskToBack();
  }
}

export async function listenBackButton() {
  const listener = await onBackButtonPress(({ canGoBack }) => handleBackButton(canGoBack));
  window.addEventListener('pagehide', () => { void listener.unregister(); }, { once: true });
}
