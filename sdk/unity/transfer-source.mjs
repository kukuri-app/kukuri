// test.ps1 の移行の確認で、desktop（配布と同じ build）を移行元として画面から動かす（#1996 AC-3）。
// 使い方: node sdk/unity/transfer-source.mjs <desktop の実行 file> <作業 dir>
// 同意の後、アカウントメニューの「アカウント追加」→「別の端末へ移す」で「リンクをコピー」し（OS の clipboard へ書く）、
// <作業 dir>/copied に公開鍵を書く。ゲームの確認コードと同じコードが出たら、画面を <作業 dir>/desktop-confirming.png に
// 撮って「一致する」を押し、完了を待って終わる。
import { writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

import { launch } from '../../apps/desktop/tests/real-app/driver.mjs';
import { accept } from '../../apps/desktop/tests/real-app/session.mjs';
import { click, text } from '../../apps/desktop/tests/real-app/ui.mjs';
import { eventually } from '../../apps/desktop/tests/web-e2e/wait.mjs';

const [app, root] = process.argv.slice(2).map((path) => resolve(path));
const desktop = await launch({ app, root, name: 'desktop', port: 9700 });
try {
  await accept(desktop);
  const key = (await desktop.invoke('get_sync_status')).local_author_pubkey;
  await click(desktop, text('shell', 'accountMenu.open'));
  await click(desktop, text('shell', 'accountMenu.add'));
  await click(desktop, text('shell', 'accountMenu.transferOut'));
  await click(desktop, text('settings', 'accountTransfer.source.copy'));
  const copied = JSON.stringify(text('settings', 'accountTransfer.source.copied'));
  await eventually('the link to be copied', () =>
    desktop.evaluate(`[...document.querySelectorAll('button')].some((node) => node.textContent.trim() === ${copied})`),
    10_000
  );
  writeFileSync(join(root, 'copied'), key);
  console.log(`copied the transfer link of ${key}`);
  const code = await eventually('the confirmation code', () =>
    desktop.invoke('get_account_transfer_status').then((status) => (status.state === 'confirming' ? status.code : null)),
    120_000
  );
  console.log(`confirming ${code}`);
  const match = JSON.stringify(text('settings', 'accountTransfer.confirm.match'));
  await eventually('the match button', () =>
    desktop.evaluate(`[...document.querySelectorAll('button')].some((node) => node.textContent.trim() === ${match})`)
  );
  const shot = await desktop.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(root, 'desktop-confirming.png'), Buffer.from((shot.result ?? shot).data, 'base64'));
  await click(desktop, text('settings', 'accountTransfer.confirm.match'));
  await eventually('the transfer to complete', () =>
    desktop.invoke('get_account_transfer_status').then((status) => status.state === 'completed'),
    60_000
  );
  console.log('completed');
} finally {
  await desktop.close();
}
