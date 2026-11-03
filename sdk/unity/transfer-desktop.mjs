// test.ps1 の確認で、desktop（配布と同じ build）や Web を画面から動かす。Player とは <作業 dir> の file でやり取りする。
// 使い方: node sdk/unity/transfer-desktop.mjs <desktop の実行 file> <作業 dir> <source|target|merge|board|live>
//        node sdk/unity/transfer-desktop.mjs <Web の fixture の origin> <作業 dir> web
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
// - live（#1983 AC-2c のゲーム外からの観戦と応援）: ゲームを入れていない desktop が、host・観戦の Player と互いの ticket を
//   取り込み、Player が書いた配信（<作業 dir>/live の topic・session・開始 post）に参加して映像を受け（音量は 0）、開始
//   post へ 👏 で応援する。`watched`・`reacted` を log に書く。続けて、host が試合の後に投稿した結果
//   （<作業 dir>/result-post）を timeline で読む（`result-seen <添付の数>`）。host が終えたら（<作業 dir>/host-done）自分も
//   camera で配信し（#1984 AC-1）、session を <作業 dir>/desktop-live に書き、観戦の Player のモニターに映った印
//   （<作業 dir>/desktop-live-seen）で配信を終える（`broadcast-seen`。映る前の見ている間に、相手が 1 人以上だった最後の topic の経路と相手の数を `path` で書く）。ゲームが
//   終わった印（<作業 dir>/game-exited）の後に返信して <作業 dir>/desktop-replied に書き、再起動したゲームが掲示板で読んだ印
//   （<作業 dir>/done）まで待って終わる。
// - web（#1984 AC-1）: live と同じことを、試験の Community Node の fixture（harness の `web_e2e_fixture`）が配る Web の
//   build を開いた Chrome（画面なし、偽の camera・mic、音は出さない）で行う。ticket は使わず、fixture の Community Node に
//   同意してつながる。応援は 🎉 で、書く file の名前は desktop の代わりに web で始まる。
// board 以外は確認コードの画面を <作業 dir>/desktop-confirming.png に撮る。target・merge・board は最後の timeline も
// <作業 dir>/desktop-timeline.png に撮る。
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

import { connectCdp, launch } from '../../apps/desktop/tests/real-app/driver.mjs';
import { BAR, injectCapture, startBroadcast, t } from '../../apps/desktop/tests/real-app/live-media.mjs';
import { TOPIC, accept, openTopic } from '../../apps/desktop/tests/real-app/session.mjs';
import { choose, click, text } from '../../apps/desktop/tests/real-app/ui.mjs';
import { eventually } from '../../apps/desktop/tests/web-e2e/wait.mjs';

const [target, dir, mode] = process.argv.slice(2, 5);
const root = resolve(dir);
// Player が読む file は、一時 file に書いてから名前を変える（書きかけを読ませない。Player の側と同じ）。
const write = (name, value) => {
  writeFileSync(join(root, `${name}.tmp`), value);
  renameSync(join(root, `${name}.tmp`), join(root, name));
};
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
  write('desktop-key', (await desktop.invoke('get_sync_status')).local_author_pubkey);
  await openTopic(desktop, TOPIC, 'timeline');
  write('desktop-ticket', await desktop.invoke('get_local_peer_ticket'));
  await desktop.invoke('import_peer_ticket', { request: { ticket: await marker('game-ticket') } });
  const post = await marker('game-post');
  const request = { topic: TOPIC, scope: { kind: 'public' }, cursor: null, limit: 20 };
  await eventually('the game board post on desktop', () =>
    desktop.invoke('list_timeline', { request }).then((view) => view.items.some((item) => item.object_id === post)),
    120_000
  );
  write('seen', post);
  await marker('game-exited');
  const reply = await desktop.invoke('create_post', {
    request: { topic: TOPIC, channel_ref: { kind: 'public' }, content: 'test: reply while the game is closed', reply_to: post },
  });
  console.log(`replied ${reply}`);
  write('replied', reply);
  await marker('done');
  await shoot('desktop-timeline.png');
};

// #1983 AC-2c・#1984 AC-1: ゲームを入れていない desktop・Web から、ゲームの試合の配信を観戦して応援し、結果に返信する。
// 試合の後は自分も配信し、観戦の Player のモニターに映るまで続ける。`frame` は視聴の映像が届いたか。
const live = async (emoji, frame) => {
  const [topic, session, post] = (await marker('live')).split('\n');
  const request = { topic, session_id: session };
  // Web は topic を開いてから Community Node で相手を見つけるので、参加できるまで繰り返す。
  await openTopic(desktop, topic, 'timeline');
  await eventually(`${desktop.name} to join the live`, () =>
    desktop.invoke('join_live_session', { request }).then(() => true, (error) => console.log(`join: ${error.message}`)),
    120_000
  );
  await desktop.invoke('watch_live_media', { request });
  await desktop.invoke('set_live_volume', { request: { session_id: session, volume: 0 } });
  await eventually(`a live frame on ${desktop.name}`, () => frame(request), 120_000);
  console.log('watched');
  await desktop.invoke('toggle_reaction', {
    request: { target_topic_id: topic, target_object_id: post, reaction_key: { kind: 'emoji', emoji }, channel_ref: { kind: 'public' } },
  });
  console.log('reacted');
  const result = await marker('result-post');
  const timeline = { request: { topic, scope: { kind: 'public' }, cursor: null, limit: 20 } };
  const item = await eventually(`the game result on ${desktop.name}`, () =>
    desktop.invoke('list_timeline', timeline).then((view) => view.items.find((entry) => entry.object_id === result)),
    120_000
  );
  console.log(`result-seen ${(item.attachments ?? []).length}`);
  // 試合の後は、同じ topic に Live を作り、その個別の列を開いて camera で配信する。
  await marker('host-done');
  await injectCapture(desktop);
  const own = await desktop.invoke('create_live_session', {
    request: { topic, channel_ref: { kind: 'public' }, title: `test: ${desktop.name} live`, description: '' },
  });
  const route = new URLSearchParams({ topic, sessionId: own, liveTopic: topic });
  await desktop.evaluate(`(location.hash = ${JSON.stringify(`#/live?${route}`)}, true)`);
  await startBroadcast(desktop);
  write(`${desktop.name}-live`, own);
  // 観戦の Player が見ている間の topic の経路と、相手の数・relay を通る相手の数。見終えた観戦の Player は終了するので、
  // 映った印が届くまでの間に読み、相手が 1 人以上だった最後の値を書く（相手 0 人の経路は分類できない）。
  let topicPath;
  await eventually(`the ${desktop.name}-live-seen file`, async () => {
    const sync = await desktop.invoke('get_sync_status');
    topicPath = sync.topic_diagnostics.find((each) => each.topic === topic && each.peer_count > 0) ?? topicPath;
    return existsSync(join(root, `${desktop.name}-live-seen`));
  }, 300_000);
  console.log(`path ${topicPath?.active_path} ${topicPath?.peer_count} ${topicPath?.fallback_peer_count}`);
  // 利用者と同じく配信を止めてから終える（取り込みが続く間は映像が届き、モニターは次の配信へ移らない）。
  await click(desktop, t('media.stopBroadcast'), { within: BAR });
  await desktop.invoke('end_live_session', { request: { topic, session_id: own } });
  console.log('broadcast-seen');
  await marker('game-exited');
  const reply = await desktop.invoke('create_post', {
    request: { topic, channel_ref: { kind: 'public' }, content: `test: GG from ${desktop.name}`, reply_to: result },
  });
  console.log(`replied ${reply}`);
  write(`${desktop.name}-replied`, reply);
  await marker('done');
};

/// Web（#1984 AC-1）: fixture が配る Web の build を、画面の無い Chrome で開く。偽の camera・mic を許可の確認なしで使い、
/// 音は出さない。runtime の command は、画面が読み込んだ web-runtime の module から呼ぶ。
const CHROME = 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe';
const RUNTIME = `(await import(performance.getEntriesByType('resource').find((entry) =>
  entry.name.includes('/kukuri_web_runtime-') && entry.name.endsWith('.js')).name))`;
async function openWeb(origin) {
  const port = 9701;
  const chrome = spawn(CHROME, [
    `--remote-debugging-port=${port}`, '--remote-allow-origins=*', `--user-data-dir=${join(root, 'web-chrome')}`, '--headless=new',
    '--no-first-run', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--mute-audio', origin,
  ], { stdio: 'ignore' });
  const page = await connectCdp(port, origin);
  return {
    name: 'web',
    evaluate: page.evaluate,
    invoke: (command, args = {}) =>
      page.evaluate(`${RUNTIME}.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})
        .catch((error) => { throw new Error(${JSON.stringify(command)} + ': ' + JSON.stringify(error)); })`),
    frame: ({ topic, session_id }) =>
      page.evaluate(`${RUNTIME}.next_live_video(${JSON.stringify(topic)}, ${JSON.stringify(session_id)}).then((data) => data.length > 64)`),
    async close() {
      page.close();
      chrome.kill();
    },
  };
}

// web の役でも、操作の相手は `desktop` と呼ぶ（desktop と同じ画面と command を持つ）。
const desktop = mode === 'web' ? await openWeb(target) : await launch({ app: resolve(target), root, name: 'desktop', port: 9700 });
try {
  if (mode === 'web') {
    await accept(desktop, { communityNode: (await (await fetch(`${target}/fixture/info`)).json()).community_node });
    await live('🎉', desktop.frame);
  } else if (mode === 'live') {
    await accept(desktop);
    write('desktop-ticket', await desktop.invoke('get_local_peer_ticket'));
    await desktop.invoke('import_peer_ticket', { request: { ticket: await marker('game-ticket') } });
    // host は試合の後に終了するので、試合の後の配信を届ける観戦の Player とも直接つながる。
    await desktop.invoke('import_peer_ticket', { request: { ticket: await marker('spectator.ticket') } });
    await live('👏', (request) =>
      desktop.evaluate(`window.__TAURI_INTERNALS__.invoke('next_live_video', ${JSON.stringify({ request })}).then((data) => (data.byteLength ?? data.length ?? 0) > 64)`)
    );
  } else if (mode === 'board') {
    await accept(desktop);
    await board();
  } else {
    await accept(desktop);
    await click(desktop, text('shell', 'accountMenu.open'));
    await click(desktop, text('shell', 'accountMenu.add'));
    if (mode !== 'target') {
      const key = (await desktop.invoke('get_sync_status')).local_author_pubkey;
      await click(desktop, text('shell', 'accountMenu.transferOut'));
      await click(desktop, text('settings', 'accountTransfer.source.copy'));
      await eventually('the link to be copied', () => visible(text('settings', 'accountTransfer.source.copied')), 10_000);
      write('copied', key);
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
      write('desktop-ticket', await desktop.invoke('get_local_peer_ticket'));
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
      write('desktop-ticket', await desktop.invoke('get_local_peer_ticket'));
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
      write('received', post);
    }
  }
} finally {
  await desktop.close();
}
