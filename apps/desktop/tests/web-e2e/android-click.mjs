/** Android の scroll-snap と固定の操作帯の下で、見えている押下位置を求める。 */
export async function androidClick(browser, element, original, timeout) {
  const result = await browser.waitUntil(async () => {
    // execute は browser の command なので、要素の自動再取得の対象にならない。
    await element.waitForExist({ timeout });
    try {
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
            return { offset: { x: Math.round(x - (rect.left + rect.width / 2)), y: 0 } };
          }
        }
        return false;
      }, element);
    } catch (error) {
      return error.name === 'stale element reference' ? false : { error };
    }
  }, { timeout, timeoutMsg: 'Android click target does not settle in view' });
  // waitUntil はcallbackの例外も再試行するので、他のerrorは待機の外で投げる。
  if (result.error) throw result.error;
  // offset が 0 でも渡す。引数なしの element click は driver が再 scroll する。
  return original(result.offset);
}
