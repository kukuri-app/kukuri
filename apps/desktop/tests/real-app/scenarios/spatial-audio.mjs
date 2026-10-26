// 空間音声(#1033 の runbook、#1800 AC-3)。聞き手 L が Dome を作って稼働・入室し、話者 S と O が入る。
// 音は OS の入出力の境界で確かめる(話者の入力へ入れた音が、聞き手の出力に出るか)。大きさは、最初に測った
// 「音量 1 あたりの出力」を基準に、距離から決まる音量(1 m 以内は 1、以遠は 1 m / 距離)と比べる。instance ごとの
// 仮想の入出力(Linux の CI)でだけ動かす(Windows の runner は 3 つの Dome の 3D の描画で CPU が足りない)。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { eventually } from '../../web-e2e/wait.mjs';
import { hearOwnOutput, isolated, PERIOD_MS, plug, record, speak, unplug } from '../audio.mjs';
import { echoDiagnostics } from '../echo-diagnostics.mjs';
import { accept, connect, openTopic, waitForPeers } from '../session.mjs';
import { audioState, choose, click, createDome, entered, observeAudio, press, pressed, text } from '../ui.mjs';

const t = (key) => text('metaverse', key);
/// Dome の画面(HUD・メニューを含む)。設定の欄など、同じ文言の別の button と分ける。
const STAGE = '.metaverse-room-stage';
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/// 大きさを測る間(話す音の周期の倍数)。
const WINDOW_MS = 5 * PERIOD_MS;
/// 話者 S・O と聞き手 L の声の帯域(Hz。50 Hz 刻みの正弦波の和)。O は S と離れた帯域で話す: S の声を再生している O の
/// エコーキャンセルは、S と同じ周波数の線の O の声を S の声のエコーと区別できずに消し続け、近い帯域の声もエコーが無いと
/// 判定するまで(十数秒)抑える(#1817)。
const VOICE = [300, 950];
const OTHER_VOICE = [2000, 2950];
const REPLY = [1500, 2950];
const distance = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const gainAt = (cm) => (cm <= 100 ? 1 : 100 / cm);
const positionOf = async (instance) => (await audioState(instance)).request.listener_position;
const logOf = (instance) => readFileSync(instance.log, 'utf8');

export async function run({ launch, restart, log, topic }) {
  if (!isolated) {
    log('skipped: needs a virtual audio device per instance (REAL_APP_AUDIO=pulse)');
    return;
  }
  let [listener, speaker, other] = await launch('listener', 'speaker', 'other');
  const all = () => [listener, speaker, other];
  unplug('other'); // SA-2: O の入力デバイスは最初は無い。
  await Promise.all(all().map(accept));
  await connect(all());
  await Promise.all(all().map((instance) => openTopic(instance, topic, 'game')));
  await Promise.all(all().map((instance) => waitForPeers(instance, topic, 2)));
  await Promise.all(all().map(observeAudio));
  const ids = {};
  for (const instance of all()) {
    const status = await instance.invoke('get_sync_status');
    ids[instance.name] = { endpoint: status.discovery.local_endpoint_id, pubkey: status.local_author_pubkey };
  }

  // L が Dome を作って稼働・入室し、S と O は一覧に届いた Dome へ自動で入る(入る先が 1 つだけのため)。
  await createDome(listener, 'dome-a');
  await Promise.all([speaker, other].map(entered));
  log('all instances entered the Dome');

  // SA-1: 入室した直後の mic は off。
  for (const instance of all()) assert.equal(await pressed(instance, t('hud.enableMicrophone')), 'false');
  log('SA-1: the microphone starts off');

  const heard = record(listener);
  // S と O は話し続ける(O は入力デバイスを付け直してから)。送るかは mic の操作で決まる。
  const voices = [speak('speaker', VOICE)];
  let unit = null; // 音量 1 あたりの、聞き手の出力の大きさ。
  const silent = () => (unit ?? 0.05) * 0.002;
  /// 聞き手の出力に音量 `gain` の音が出るのを待ち、落ち着いてから測った大きさを返す。
  const expectSound = async (what, gain) => {
    await eventually(`the listener to hear ${what}`, () => heard.level(WINDOW_MS) > (unit ? 0.3 * unit * gain : 1e-3), 30_000);
    await sleep(WINDOW_MS);
    return heard.level(WINDOW_MS);
  };
  /// 聞き手の出力が `ms` 以内に無音になるのを待ち、かかった時間を返す。
  const expectSilence = async (what, ms) => {
    const since = Date.now();
    while (heard.level(PERIOD_MS) >= silent()) {
      if (Date.now() - since > ms) throw new Error(`the listener still hears ${what} after ${ms} ms`);
      await sleep(100);
    }
    return Date.now() - since;
  };
  const sameDome = async (instance) => gainAt(distance(await positionOf(listener), await positionOf(instance)));
  try {
    // SA-2: mic を on にすると on。入力デバイスを開けない構成では on にならず、聞き手の出力は無音のまま。
    await click(other, t('hud.enableMicrophone'));
    await eventually('other to fail to open the microphone', () => logOf(other).includes('microphone could not be opened'));
    await sleep(WINDOW_MS);
    assert.equal(await pressed(other, t('hud.enableMicrophone')), 'false');
    assert.ok(heard.level(WINDOW_MS) < 1e-4, 'the listener stays silent');
    plug('other');
    log('SA-2: the microphone stays off without an input device');
    voices.push(speak('other', OTHER_VOICE));
    await micOn(speaker);
    log('SA-2: the microphone turns on');

    // SA-3: 話者の入力の音が聞き手の出力に出る。
    const gain = await sameDome(speaker);
    const near = await expectSound('the speaker', gain);
    unit = near / gain;
    log(`SA-3: heard the speaker at gain ${gain.toFixed(3)} as ${near.toFixed(4)}`);

    // SA-4: L も話し、L のスピーカーの音が L の mic に入る部屋でも、S の出力に S 自身の声は戻らない(L のエコー
    // キャンセル)。L が黙っている間にエコーキャンセルが学び、その後で L が話す(L の声は L の部屋=出力へ流す)。
    const echo = record(speaker);
    await micOn(listener);
    const restore = hearOwnOutput(listener);
    const diagnostics = echoDiagnostics({ listener, speaker, heard, echo, voice: VOICE, reply: REPLY });
    let voice = () => undefined;
    let measurement;
    try {
      await sleep(4_000);
      voice = speak('listener', REPLY, 'listener_out');
      await sleep(6_000);
      const window = echo.snapshot(2 * WINDOW_MS);
      const own = echo.level(2 * WINDOW_MS, VOICE, window);
      const reply = echo.level(2 * WINDOW_MS, REPLY, window);
      measurement = { near, own, reply, window, listener: heard.snapshot(2 * WINDOW_MS, window.at) };
      log(`SA-4: the speaker hears the listener as ${reply.toFixed(4)} and its own voice as ${own.toFixed(4)}`);
      // L の声が消されていないこと(エコーの経路の遅れが揺れると、エコーキャンセルは L の声も数秒 1/4 ほどまで弱めるので、
      // 基準は S の声の 10%。2026-10-15 のユーザー判断、#1822)。
      assert.ok(reply > 0.1 * near, 'the speaker hears the listener');
      assert.ok(own < 0.05 * reply, 'the own voice of the speaker does not come back');
    } catch (error) {
      if (measurement) {
        await diagnostics.save(measurement).catch((reason) => log(`SA-4: could not save diagnostics: ${reason}`));
      }
      throw error;
    } finally {
      voice();
      restore();
      echo.stop();
      await diagnostics.stop();
    }
    await micOff(listener);

    // SA-3: 離れると、出力の大きさが音量の比のとおりに減る。
    await walk(listener, 's', () =>
      eventually('the listener to walk 4 m away', async () => (await sameDome(speaker)) <= 0.25)
    );
    const farGain = await sameDome(speaker);
    await sleep(1_000 + WINDOW_MS);
    const far = heard.level(WINDOW_MS);
    const ratio = far / near / (farGain / gain);
    log(`SA-3: at gain ${farGain.toFixed(3)} heard as ${far.toFixed(4)} (${ratio.toFixed(2)} of the expected)`);
    assert.ok(Math.abs(ratio - 1) < 0.2, 'the output follows the attenuation');

    // SA-6: L が S を mute すると S の音だけが止まる。presence と移動は変わらない。
    await authorAction(listener, topic, ids.speaker.pubkey, text('common', 'actions.mute'));
    await expectSilence('the muted speaker', 10_000);
    await openCategory(listener, 'diagnostics');
    const peers = await diagnostic(listener, 'hud.knownPeers_other');
    await walk(speaker, 'w', () =>
      // 診断は avatar ごとの動作を出す。歩くのは S だけ。
      eventually('the listener to see the muted speaker walk', async () =>
        (await diagnostic(listener, 'hud.remoteAnimation'))?.includes(':walk')
      )
    );
    assert.equal(await diagnostic(listener, 'hud.knownPeers_other'), peers);
    await micOn(other);
    await expectSound('the other speaker', await sameDome(other));
    await micOff(other);
    await expectSilence('the other speaker', 10_000);
    await authorAction(listener, topic, ids.speaker.pubkey, text('common', 'actions.unmute'));
    await expectSound('the unmuted speaker', await sameDome(speaker));
    log(`SA-6: muting stops only the muted speaker, who stays present (${peers}) and moving`);

    // SA-7: S が Dome を出ると、L の出力は 5 秒以内に無音になる。
    await click(speaker, t('hud.leave'));
    log(`SA-7: silent ${await expectSilence('the speaker who left', 5_000)} ms after the speaker left`);

    // SA-5: 通行可能な隣の Dome の話者の音が出る。通行可能でない隣からは出ない。O が Dome を作り、L が提案した接続を
    // O が採用する。L が接続を解除すると、O の Dome は通行可能でない隣になる。
    await click(other, t('hud.leave'));
    await eventually('other to leave', async () => (await pressed(other, t('hud.leave'))) === undefined, 30_000);
    await createDome(other, 'dome-b');
    await openCategory(listener, 'connections');
    await choose(listener, t('connections.target'), 'dome-b');
    await click(listener, t('connections.propose'), { within: STAGE });
    await openCategory(other, 'connections');
    // 提案した向き(北)の反対の向きに届く。
    await click(other, t('connections.directions.south'), { prefix: true, within: STAGE });
    await click(other, t('connections.accept'), { within: STAGE, timeout: 60_000 });
    const neighbor = await eventually(
      'the listener to have a passable neighbor',
      async () => (await audioState(listener)).request.neighbors[0],
      90_000
    );
    await micOn(other);
    const through =
      distance(await positionOf(other), neighbor.speaker_opening) +
      distance(neighbor.listener_opening, await positionOf(listener));
    await expectSound('the speaker in the neighbor Dome', gainAt(through));
    await click(listener, t('connections.revoke'), { within: STAGE });
    await expectSilence('the speaker behind the closed connection', 15_000);
    log(`SA-5: heard through the opening (${(through / 100).toFixed(1)} m) only while the connection was passable`);

    // SA-7: 再起動の後の mic は off(O は mic を on のまま止める)。
    other = await restart(other);
    await accept(other);
    await openTopic(other, topic, 'game');
    await entered(other);
    assert.equal(await pressed(other, t('hud.enableMicrophone')), 'false');
    log('SA-7: the microphone starts off after a restart');
  } finally {
    for (const stop of voices) stop();
    heard.stop();
  }

  // SA-8: 経路は direct_p2p。音声まわりの log に、他の端末の識別子と音声が出ない。
  const established = logOf(listener)
    .split('\n')
    .filter((line) => line.includes('media connection established'));
  assert.ok(established.length > 0 && established.every((line) => line.includes('path="direct_p2p"')), established.join('\n'));
  for (const instance of all()) {
    const lines = logOf(instance)
      .split('\n')
      .filter((line) => /media|audio|microphone|speaker/i.test(line));
    for (const line of lines) {
      assert.ok(line.length < 1_000, `a long media log line: ${line.slice(0, 200)}`);
      for (const [name, { endpoint, pubkey }] of Object.entries(ids)) {
        if (name !== instance.name) assert.ok(!line.includes(endpoint) && !line.includes(pubkey), line);
      }
    }
  }
  log(`SA-8: ${established.length} media connection(s) over direct_p2p, no identifiers in the media logs`);
}

async function micOn(instance) {
  await click(instance, t('hud.enableMicrophone'));
  await eventually(
    `${instance.name} to turn the microphone on`,
    async () => (await pressed(instance, t('hud.disableMicrophone'))) === 'true',
    30_000
  );
}

async function micOff(instance) {
  await click(instance, t('hud.disableMicrophone'));
  await eventually(
    `${instance.name} to turn the microphone off`,
    async () => (await pressed(instance, t('hud.enableMicrophone'))) === 'false',
    30_000
  );
}

/// avatar を `key` の向きへ、`until()` が終わるまで歩かせる(利用者と同じく、window を前に出して Dome の列を選び、操作を
/// 再開してから押し、Esc でポインタの固定を解く)。Dome の操作は、window の focus・Dome の列が選ばれていること・列が見えて
/// いることがそろうまで止まる。window の切り替え(xdotool)が効かない回や、列が選ばれていない・見えていない回があるので、
/// 操作を再開できるようになるまで置き直す。列の外枠に focus を置くと列が選ばれ、見える位置へ動く(既に focus があると event
/// が出ないので一度外す)。Dome の画面の中を押しても列は選ばれない(#1822)。進む速さは描画の frame の数で変わるので、時間
/// ではなく着いたことで止める。
async function walk(instance, key, until) {
  const resume = JSON.stringify(t('camera.resume'));
  await eventually(
    `${instance.name} to be able to resume the avatar`,
    async () => {
      if (!(await instance.evaluate('document.hasFocus()'))) instance.focus();
      return instance.evaluate(`(() => {
        const column = document.querySelector('${STAGE}').closest('[data-column-id]');
        if (document.activeElement === column) column.blur();
        column.focus();
        return document.hasFocus() && [...column.querySelectorAll('button')].some((node) => node.textContent.trim() === ${resume} && !node.disabled);
      })()`);
    },
    30_000
  );
  await click(instance, t('camera.resume'));
  await press(instance, key, until);
  await press(instance, 'Escape');
}

/// 作者の詳細で `label` の操作をし、Metaverse の列へ戻る(作者の詳細は timeline の列の右の欄に開く。各列は並んだまま)。
async function authorAction(instance, topic, pubkey, label) {
  const hash = `#/timeline?topic=${encodeURIComponent(topic)}&context=author&authorPubkey=${pubkey}`;
  await instance.evaluate(`(location.hash = ${JSON.stringify(hash)}, true)`);
  await click(instance, label);
  await openTopic(instance, topic, 'game');
}

/// Dome のメニューの `category` を開く(メニューが閉じていれば開く)。
async function openCategory(instance, category) {
  const label = t(`menu.categories.${category}`);
  const shown = await instance.evaluate(
    `[...document.querySelectorAll('${STAGE} button')].some((node) => node.textContent.trim() === ${JSON.stringify(label)} && node.getClientRects().length > 0)`
  );
  if (!shown) await click(instance, t('menu.open'), { within: STAGE });
  await click(instance, label, { within: STAGE });
}

/// 診断の欄の `key` の行の値(`{{...}}` の部分)。
function diagnostic(instance, key) {
  const [before] = t(key).split('{{');
  return instance.evaluate(
    `[...document.querySelectorAll('span')].map((node) => node.textContent).find((value) => value.startsWith(${JSON.stringify(before)}))?.slice(${before.length}) ?? null`
  );
}
