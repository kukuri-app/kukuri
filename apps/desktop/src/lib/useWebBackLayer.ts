import { useEffect, useRef } from 'react';
import { flushSync } from 'react-dom';
import { IS_WEB_RUNTIME } from './webRuntime';

const KEY = 'kukuriBackLayer';
type Layer = { id: number; url: string; popped: boolean; close: () => void };
const layers: Layer[] = [];
let serial = 0;
const retain = (layer: Layer) => history.pushState({ ...history.state, [KEY]: layer.id }, '', layer.url);
function onBack(event: PopStateEvent) {
  const layer = layers.at(-1);
  if (!layer || event.state?.[KEY] === layer.id) return;
  layer.popped = true;
  flushSync(layer.close);
  // dismiss を拒む同意等の modal は、そのままの画面で次の戻るも受ける。
  if (layers.includes(layer)) { layer.popped = false; retain(layer); }
}

/** OS keyboard の戻るの後、表示中の overlay/composer を一層だけ閉じる。 */
export function useWebBackLayer(open: boolean, close: () => void) {
  const callback = useRef(close);
  useEffect(() => { callback.current = close; }, [close]);
  useEffect(() => {
    if (!IS_WEB_RUNTIME || !open) return;
    const layer: Layer = { id: ++serial, url: location.href, popped: false, close: () => callback.current() };
    if (!layers.length) window.addEventListener('popstate', onBack);
    layers.push(layer);
    retain(layer);
    return () => {
      layers.splice(layers.indexOf(layer), 1);
      if (!layers.length) window.removeEventListener('popstate', onBack);
      if (!layer.popped) queueMicrotask(() => {
        if (history.state?.[KEY] === layer.id && location.href === layer.url) history.back();
      });
    };
  }, [open]);
}
