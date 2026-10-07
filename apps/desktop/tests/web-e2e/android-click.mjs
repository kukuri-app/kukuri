import { eventually } from './wait.mjs';

/** Android の scroll-snap と固定の操作帯の下で、見えている押下位置を求める。 */
export async function androidClick(browser, element, original, timeout) {
  const offset = await eventually('Android click target settles in view', async () => {
    // execute は browser の command なので、要素の自動再取得の対象にならない。
    try {
      await element.waitForExist({ timeout });
      return await browser.execute(async (node) => {
        node.scrollIntoView({ block: 'center', inline: 'center' });
        let last = '';
        for (let i = 0; i < 40; i += 1) {
          await new Promise((resolve) => setTimeout(resolve, 50));
          const { left, top } = node.getBoundingClientRect();
          if (`${left},${top}` === last) break;
          last = `${left},${top}`;
        }
        if (!node.isConnected) return false;
        const rect = node.getBoundingClientRect();
        for (const fx of [0.5, 0.85, 0.15, 0.95, 0.05]) {
          const x = rect.left + rect.width * fx;
          const y = rect.top + rect.height / 2;
          if (node.contains(document.elementFromPoint(x, y))) {
            return { x: Math.round(x - (rect.left + rect.width / 2)), y: 0 };
          }
        }
        return false;
      }, element);
    } catch (error) {
      if (error.name !== 'stale element reference') throw error;
      return false;
    }
  }, timeout);
  // offset が 0 でも渡す。引数なしの element click は driver が再 scroll する。
  return original(offset);
}
