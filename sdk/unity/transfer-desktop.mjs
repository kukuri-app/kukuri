// test.ps1 の移行の確認で、desktop（配布と同じ build）を画面から動かす。Player とは <作業 dir> の file でやり取りする。
// 使い方: node sdk/unity/transfer-desktop.mjs <desktop の実行 file> <作業 dir> <source|target|merge|board>
// - source（#1996 AC-3）: アカウントメニューの「アカウント追加」→「別の端末へ移す」で「リンクをコピー」し（OS の clipboard
//   へ書く）、<作業 dir>/copied に公開鍵を書く。同じ確認コードが出たら「一致する」を押し、完了を待って終わる。
// - target（#1998 AC-2「kukuriで続ける」）: 「別の端末から移す」を開き、Player が <作業 dir>/handoff を書いたら OS の
//   clipboard のリンクを貼り付け、投稿の履歴に「直近 30 日」を選んで接続する（ゲームは履歴を送り終えるのを待たずに
//   プレイに戻る）。「一致する」→「このアカウントを使う」でゲームのアカウントに切り替え、Player と
//   互いの ticket を取り込み、試験の topic の timeline でゲームの投稿（<作業 dir>/game-post）を読んで返信する。Player が
//   <作業 dir>/received を書いたら、ゲームの投稿が画面の timeline に出るのを待って `shown` を log に書き、終わる。
// - merge（#1998 AC-3「既存のアカウントへまとめる」）: source と同じくリンクをコピーして承認する。その後 Player と互いの
//   ticket を取り込み、まとめた後に Player が書いた投稿（<作業 dir>/game-post）が desktop のアカウントの投稿として画面の
//   timeline に出るのを待って `shown` を log に書き、<作業 dir>/received を書いて終わる。
// - board（#1981 AC-2 の掲示板）: 移行は使わない。試験の topic を開き、Player と互いの ticket を取り込み、ゲームの掲示板の
//   投稿（<作業 dir>/game-post）が届いたら <作業 dir>/seen を書く。Player が終わった印（<作業 dir>/game-exited）の後に
//   返信して <作業 dir>/replied に返信の ID を書き、再起動した Player が読んだ印（<作業 dir>/done）まで投稿を保持して終わる。
// board 以外は確認コードの画面を <作業 dir>/desktop-confirming.png に撮る。target・merge・board は最後の timeline も
// <作業 dir>/desktop-timeline.png に撮る。
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

import { launch } from '../../apps/desktop/tests/real-app/driver.mjs';
import { TOPIC, accept, openTopic } from '../../apps/desktop/tests/real-app/session.mjs';
import { choose, click, text } from '../../apps/desktop/tests/real-app/ui.mjs';
import { eventually } from '../../apps/desktop/tests/web-e2e/wait.mjs';

const [app, root] = process.argv.slice(2, 4).map((path) => resolve(path));
const mode = process.argv[4];
const marker = (name) => eventually(`the ${name} file`, () => existsSync(join(root, name)) && readFileSync(join(root, name), 'utf8').trim(), 300_000);
const visible = (label) => {
  const name = JSON.stringify(label);
  return desktop.evaluate(`[...document.querySelectorAll('button')].some((node) => node.textContent.trim() === ${name} && node.getClientRects().length > 0)`);
};
const shoot = async (name) => {
  const shot = await desktop.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(root, name), Buffer.from((shot.result ?? shot).data, 'base64'));
};
const status = () => desktop.invoke('get_account_transfer_status');

// #1981 AC-2: ゲームの掲示板の投稿に、ゲームを終えている間に返信し、再起動したゲームが読むまで保持する。
const board = async () => {
  writeFileSync(join(root, 'desktop-key'), (await desktop.invoke('get_sync_status')).local_author_pubkey);
  await openTopic(desktop, TOPIC, 'timeline');
  writeFileSync(join(root, 'desktop-ticket'), await desktop.invoke('get_local_peer_ticket'));
  await desktop.invoke('import_peer_ticket', { request: { ticket: await marker('game-ticket') } });
  const post = await marker('game-post');
  const request = { topic: TOPIC, scope: { kind: 'public' }, cursor: null, limit: 20 };
  await eventually('the game board post on desktop', () =>
    desktop.invoke('list_timeline', { request }).then((view) => view.items.some((item) => item.object_id === post)),
    120_000
  );
  writeFileSync(join(root, 'seen'), post);
  await marker('game-exited');
  const reply = await desktop.invoke('create_post', {
    request: { topic: TOPIC, channel_ref: { kind: 'public' }, content: 'test: reply while the game is closed', reply_to: post },
  });
  console.log(`replied ${reply}`);
  writeFileSync(join(root, 'replied'), reply);
  await marker('done');
  await shoot('desktop-timeline.png');
};

const desktop = await launch({ app, root, name: 'desktop', port: 9700 });
try {
  await accept(desktop);
  if (mode === 'board') {
    await board();
  } else {
    await click(desktop, text('shell', 'accountMenu.open'));
    await click(desktop, text('shell', 'accountMenu.add'));
    if (mode !== 'target') {
      const key = (await desktop.invoke('get_sync_status')).local_author_pubkey;
      await click(desktop, text('shell', 'accountMenu.transferOut'));
      await click(desktop, text('settings', 'accountTransfer.source.copy'));
      await eventually('the link to be copied', () => visible(text('settings', 'accountTransfer.source.copied')), 10_000);
      writeFileSync(join(root, 'copied'), key);
      console.log(`copied the transfer link of ${key}`);
    } else {
      await click(desktop, text('shell', 'accountMenu.transferIn'));
      const key = await marker('handoff');
      // 利用者が貼り付けるのと同じく、OS の clipboard のリンクを入力欄へ入れる。
      const link = execFileSync('powershell', ['-NoProfile', '-Command', 'Get-Clipboard'], { encoding: 'utf8' }).trim();
      await desktop.evaluate(
        `(() => {
          const node = document.querySelector('[data-testid="account-transfer-target"] input');
          Object.getOwnPropertyDescriptor(Object.getPrototypeOf(node), 'value').set.call(node, ${JSON.stringify(link)});
          node.dispatchEvent(new Event('input', { bubbles: true }));
          return true;
        })()`
      );
      await choose(desktop, text('settings', 'accountTransfer.history.label'), text('settings', 'accountTransfer.history.choice.month'));
      await click(desktop, text('settings', 'accountTransfer.target.connect'));
      console.log(`pasted the transfer link of ${key}`);
    }
    const code = await eventually('the confirmation code', () => status().then((each) => (each.state === 'confirming' ? each.code : null)), 120_000);
    console.log(`confirming ${code}`);
    await eventually('the match button', () => visible(text('settings', 'accountTransfer.confirm.match')));
    await shoot('desktop-confirming.png');
    await click(desktop, text('settings', 'accountTransfer.confirm.match'));
    await eventually('the transfer to complete', () => status().then((each) => each.state === 'completed'), 60_000);
    console.log('completed');
    if (mode === 'target') {
      const key = await marker('handoff');
      await click(desktop, text('settings', 'accountTransfer.completed.use'));
      await eventually('the game account to be active', () =>
        desktop.invoke('get_sync_status').then((sync) => sync.local_author_pubkey === key).catch(() => false)
      );
      console.log(`switched to ${key}`);
      // 切り替えたアカウントも、試験の端末どうしだけでつながるようにコミュニティノードを外し、案内を閉じる（accept と同じ）。
      await desktop.invoke('set_community_node_config', { request: { nodes: [], trust_node_priority: null } });
      await click(desktop, text('settings', 'communityNode.onboarding.later'), { timeout: 10_000 }).catch(() => undefined);
      await openTopic(desktop, TOPIC, 'timeline');
      writeFileSync(join(root, 'desktop-ticket'), await desktop.invoke('get_local_peer_ticket'));
      await desktop.invoke('import_peer_ticket', { request: { ticket: await marker('game-ticket') } });
      const post = await marker('game-post');
      const request = { topic: TOPIC, scope: { kind: 'public' }, cursor: null, limit: 20 };
      await eventually('the game post on desktop', () =>
        desktop.invoke('list_timeline', { request }).then((view) => view.items.some((item) => item.object_id === post && item.author_pubkey === key)),
        120_000
      );
      const reply = await desktop.invoke('create_post', {
        request: { topic: TOPIC, channel_ref: { kind: 'public' }, content: 'test: reply from kukuri app', reply_to: post },
      });
      console.log(`replied ${reply}`);
      await marker('received');
      await eventually('the game post on the timeline', () =>
        desktop.evaluate(`document.body.innerText.includes('test: from the game')`),
        60_000
      );
      await shoot('desktop-timeline.png');
      console.log('shown');
    }
    if (mode === 'merge') {
      const key = (await desktop.invoke('get_sync_status')).local_author_pubkey;
      await openTopic(desktop, TOPIC, 'timeline');
      writeFileSync(join(root, 'desktop-ticket'), await desktop.invoke('get_local_peer_ticket'));
      await desktop.invoke('import_peer_ticket', { request: { ticket: await marker('game-ticket') } });
      const post = await marker('game-post');
      const request = { topic: TOPIC, scope: { kind: 'public' }, cursor: null, limit: 20 };
      await eventually('the merged game post on desktop', () =>
        desktop.invoke('list_timeline', { request }).then((view) => view.items.some((item) => item.object_id === post && item.author_pubkey === key)),
        120_000
      );
      await eventually('the merged game post on the timeline', () =>
        desktop.evaluate(`document.body.innerText.includes('test: after the merge')`),
        60_000
      );
      await shoot('desktop-timeline.png');
      console.log(`shown ${post} ${key}`);
      writeFileSync(join(root, 'received'), post);
    }
  }
} finally {
  await desktop.close();
}
