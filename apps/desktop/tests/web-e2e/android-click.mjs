/** Android の scroll-snap と固定の操作帯の下で、見えている押下位置を求める。 */
export async function androidClickOffset(browser, element, timeout) {
  const { offset } = await browser.waitUntil(async () => {
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
        return { offset: null };
      }, element);
    } catch (error) {
      if (error.name !== 'stale element reference') throw error;
      return false;
    }
  }, { timeout, timeoutMsg: 'Android click target keeps being replaced' });
  return offset;
}
