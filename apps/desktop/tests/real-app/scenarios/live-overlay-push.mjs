// #1950 AC-2: 2送信者が各10回/秒を60秒押し、別の視聴者へx1200として届く。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { eventually } from '../../web-e2e/wait.mjs';
import { audioMode, PERIOD_MS, record, speak } from '../audio.mjs';
import { accept, connect, openTopic, waitForPeers } from '../session.mjs';
import { drawsSince, injectCapture, observeDraws, startBroadcast, t, watch, VOICE } from '../live-media.mjs';
import { click } from '../ui.mjs';
import { startVirtualDisplay } from '../virtual-display.mjs';

const ACTIVE = '.shell-column-surface[data-active="true"]';
const STAGE = `${ACTIVE} .live-stage`;
const label = t('push.target').replace('{{index}}', '2');
const button = `${ACTIVE} .live-column-side .media-push-control button[aria-label="${label}"]`;
const attachmentButton = (index) => button.replace(label, t('push.target').replace('{{index}}', String(index)));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const p95 = (values) => [...values].sort((a, b) => a - b)[Math.ceil(values.length * 0.95) - 1];
const count = (instance) => instance.evaluate(`Number(document.querySelector('${STAGE} .live-overlay-push-count')?.textContent.slice(1) ?? 0)`);
const route = (instance, hash) => instance.evaluate(`(location.hash=${JSON.stringify(hash)},true)`);

async function halfThread(instance, objectId) {
  await click(instance, t('stage.split'), { within: ACTIVE });
  await click(instance, t('stage.publicThread'), { within: `${ACTIVE} [role="tablist"]` });
  await eventually(`${instance.name} to offer Push`, () => instance.evaluate(`(() => {
    const card=document.querySelector('${ACTIVE} .live-column-side article[data-post-object-id="${objectId}"]');
    if(!card){
      document.querySelector('${ACTIVE} .live-column-side .timeline-feed-refresh-banner')?.click();
      return false;
    }
    card?.scrollIntoView({block:'center'});
    const push=document.querySelector(${JSON.stringify(button)});
    if(push && !push.disabled)return true;
    const retry=card?.querySelector('.media-fetch-retry:not([aria-disabled="true"])');
    retry?.click();
    return false;
  })()`, { gesture: true }), 60000);
}

async function observePushes(instance, objectId) {
  await instance.evaluate(`(async () => {
    const internal=window.__TAURI_INTERNALS__;
    const m=window.__pushMetrics={received:0,counts:{},paint:[],wire:[],pending:[],busy:0,lastId:null,enabled:true};
    const collect=()=>{
      m.pending=m.pending.filter((item)=>{
        const shown=Number(document.querySelector('${STAGE} [data-asset-hash="'+item.hash+'"] .live-overlay-push-count')?.textContent.slice(1)??0);
        if(item.expected>shown)return true;
        m.paint.push(Date.now()-item.at);return false;
      });
    };
    m.observer=new MutationObserver(collect);
    m.observer.observe(document.querySelector('${STAGE} .live-stage-pushes'),{subtree:true,childList:true,characterData:true});
    m.listener=await internal.invoke('plugin:event|listen',{event:'kukuri://runtime-event',target:{kind:'Any'},handler:internal.transformCallback((event)=>{
      const data=event.payload;
      if(data.type!=='live_overlay_push_received') return;
      if(!data.push) {if(m.enabled)m.busy++;return;}
      const push=data.push;
      if(push.target.object_id!==${JSON.stringify(objectId)})return;
      m.lastId=push.event_id;
      if(!m.enabled)return;
      m.wire.push(Date.now()-push.sent_at);
      const hash=push.target.attachment_hash;
      m.received++;
      m.counts[hash]=(m.counts[hash]??0)+1;
      m.pending.push({hash,expected:m.counts[hash],at:push.received_at});
      collect();
    })});
    return true;
  })()`);
}

async function pressForOneMinute(instance, index) {
  const pubkey = (await instance.invoke('get_sync_status')).local_author_pubkey;
  await instance.evaluate(`(async()=>{
    const m=window.__pushSender={sent:0,acks:0,unavailable:0,paint:[],wall:[],idle:[],done:false};
    const internal=window.__TAURI_INTERNALS__;
    m.listener=await internal.invoke('plugin:event|listen',{event:'kukuri://runtime-event',target:{kind:'Any'},handler:internal.transformCallback((event)=>{
      if(event.payload.type==='live_overlay_push_received' && event.payload.push?.sender_pubkey===${JSON.stringify(pubkey)})m.acks++;
    })});
    const node=document.querySelector(${JSON.stringify(attachmentButton(index))});
    node.scrollIntoView({block:'center'});
    await new Promise((done)=>{
      let samples=0;
      const idle=setInterval(()=>{
        const at=performance.now();
        requestAnimationFrame(()=>m.idle.push(performance.now()-at));
        if(++samples===20)clearInterval(idle);
      },50);
      setTimeout(done,1200);
    });
    return true;
  })()`, { gesture: true });
  const pending = new Set();
  const report = { attempted: 0, rejected: 0, peak: 0 };
  const timer = setInterval(() => {
    report.attempted++;
    if (pending.size === 256) report.rejected++;
    else {
      const queued = Date.now();
      const job = instance.evaluate(`(() => {
        const m=window.__pushSender,at=performance.now();
        const current=document.querySelector(${JSON.stringify(attachmentButton(index))});
        if(current && !current.disabled)current.click();else m.unavailable++;
        m.sent++;
        requestAnimationFrame(()=>{m.paint.push(performance.now()-at);m.wall.push(Date.now()-${queued});});
        if(m.sent===600)m.done=true;
        return true;
      })()`, { gesture: true }).catch(() => { report.rejected++; }).finally(() => pending.delete(job));
      pending.add(job);
      report.peak = Math.max(report.peak, pending.size);
    }
    if (report.attempted === 600) clearInterval(timer);
  }, 100);
  return { stop: () => clearInterval(timer), report };
}

export async function run({ launch, log, topic }) {
  const displays = [];
  let all;
  try {
    if (process.platform === 'linux') for (let index = 0; index < 3; index++) displays.push(await startVirtualDisplay());
    const env = Object.fromEntries(['host', 'first', 'second'].map((name, index) => [name, displays[index]?.env ?? {}]));
    all = await launch(env, 'host', 'first', 'second');
  } catch (error) { await Promise.all(displays.map((display) => display.close())); throw error; }
  displays.forEach((display, index) => {
    const close = all[index].close;
    all[index].close = async () => { try { await close(); } finally { await display.close(); } };
  });
  const [host, sender, viewer] = all;
  await Promise.all(all.map(accept));
  await Promise.all(all.map((instance) => eventually(`${instance.name} to be in the foreground`, () => instance.evaluate('document.hasFocus() && !document.hidden'), 10000)));
  log(`viewports: ${JSON.stringify(await Promise.all(all.map((instance) => instance.evaluate('({width:innerWidth,height:innerHeight,focus:document.hasFocus(),hidden:document.hidden})'))))}`);
  for (const instance of all) assert.equal(await instance.evaluate('Boolean(window.__KUKURI_DESKTOP__)'), false, 'use the native runtime');
  if (process.platform === 'win32') for (const instance of all) await instance.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'no-preference' }] });
  await connect(all);
  await Promise.all(all.map((instance) => openTopic(instance, topic, 'timeline')));
  await Promise.all(all.map((instance) => waitForPeers(instance, topic, 2)));
  const channel = await host.invoke('create_public_channel', { request: { topic, title: 'Push thread' } });
  const ref = { kind: 'public_channel', channel_id: channel.channel_id };
  const threadRoute = `/timeline?topic=${encodeURIComponent(topic)}&channel=${channel.channel_id}`;
  await Promise.all(all.map((instance) => route(instance, threadRoute)));
  await Promise.all(all.map((instance) => eventually(`${instance.name} to resolve the public thread`, () => instance.invoke('get_public_channel', { request: { topic, channel_id: channel.channel_id } }), 60000)));
  const live = await host.invoke('create_live_session', { request: { topic, channel_ref: ref, title: 'Push A', description: '' } });
  const otherLive = await sender.invoke('create_live_session', { request: { topic, channel_ref: ref, title: 'Push B', description: '' } });
  const attachments = ['png', 'gif'].map((kind) => {
    const bytes = readFileSync(new URL(`../../fixtures/overlay-${kind === 'png' ? 'transparent.png' : 'animated.gif'}`, import.meta.url));
    return { file_name: `overlay.${kind}`, mime: `image/${kind}`, byte_size: bytes.length, data_base64: bytes.toString('base64'), role: 'image_original' };
  });
  const postId = await host.invoke('create_post', { request: { topic, channel_ref: ref, content: 'Push these attachments', reply_to: null, attachments } });
  const original = (await host.invoke('list_thread', { request: { topic, thread_id: postId, cursor: null, limit: 1 } })).items.find((post) => post.object_id === postId);
  for (let index = 0; index < 7; index++) await host.invoke('create_post', { request: { topic, channel_ref: ref, content: `Newer ${index}`, reply_to: null } });
  const liveRoute = (id) => `/live?topic=${encodeURIComponent(topic)}&channel=${channel.channel_id}&sessionId=${id}`;
  await Promise.all(all.map((instance) => route(instance, liveRoute(live))));
  await Promise.all(all.map((instance) => eventually(`${instance.name} to show Live A`, () => instance.evaluate(`document.querySelector('${STAGE}')?.dataset.liveSessionId===${JSON.stringify(live)}`), 60000)));
  await Promise.all([host, sender].map((instance) => halfThread(instance, postId)));
  await injectCapture(host);
  await Promise.all([sender, viewer].map((instance) => observeDraws(instance, { readPixels: false })));
  await startBroadcast(host);
  await Promise.all([sender, viewer].map(watch));
  const heard = audioMode ? record(viewer) : null;
  const stop = audioMode ? speak('host', VOICE) : () => {};
  try {
    if (heard) await eventually('to hear the host before Push', () => heard.level(5 * PERIOD_MS) > 1e-3, 30000);
    const baseline = heard?.level(5 * PERIOD_MS) ?? 0;
    await click(host, label, { within: `${ACTIVE} .live-column-side` });
    await eventually('a remote Push to display', async () => await count(viewer) === 1, 30000);
    await eventually('the GIF to decode', () => viewer.evaluate(`document.querySelector('${STAGE} .live-stage-pushes img')?.naturalWidth===48`), 30000);
    const sharedUrl = await viewer.evaluate(`document.querySelector('${STAGE} .live-stage-pushes img').src.split('#')[0]`);
    await eventually('the warmup Push to expire', async () => await count(viewer) === 0, 10000);
    await observePushes(viewer, postId);
    for (const indices of [[2, 2], [1, 2]]) {
      await viewer.evaluate('(Object.assign(window.__pushMetrics,{received:0,counts:{},paint:[],wire:[],pending:[],busy:0}),true)');
      const since = Date.now();
      const actors = await Promise.all([host, sender].map((instance, index) => pressForOneMinute(instance, indices[index])));
      try {
        await eventually('both senders to finish 600 accepted presses', async () => (await Promise.all([host, sender].map((instance) => instance.evaluate('window.__pushSender.done && window.__pushSender.acks===600 && window.__pushSender.paint.length===600')))).every(Boolean), 75000);
      } catch (error) {
        const observed = await Promise.all([host, sender].map((instance) => instance.evaluate(`(() => {const m=window.__pushSender,q=(values)=>[...values].sort((a,b)=>a-b)[Math.ceil(values.length*.95)-1];return {sent:m.sent,acks:m.acks,unavailable:m.unavailable,paint:m.paint.length,done:m.done,input:q(m.paint),wall:q(m.wall),idle:q(m.idle)};})()`)));
        throw new Error(`${error.message}; driver=${JSON.stringify(actors.map((actor) => actor.report))}, senders=${JSON.stringify(observed)}, remote=${await count(viewer)}`, { cause: error });
      } finally { actors.forEach((actor) => actor.stop()); }
      await eventually('all 1200 Pushes to display remotely', () => viewer.evaluate(`JSON.stringify([...document.querySelectorAll('${STAGE} .live-overlay-push-count')].map(node=>Number(node.textContent.slice(1))).sort())===${JSON.stringify(JSON.stringify(indices[0] === indices[1] ? [1200] : [600, 600]))}`), 5000);
      assert.equal(await viewer.evaluate(`[...document.querySelectorAll('${STAGE} .live-stage-pushes img')].filter(image=>image.naturalWidth===48).length`), new Set(indices).size, 'every pushed attachment remains decoded under load');
      await eventually('all received Pushes to paint', () => viewer.evaluate('window.__pushMetrics.pending.length===0'), 5000);
      const receive = await viewer.evaluate('({paint:window.__pushMetrics.paint,wire:window.__pushMetrics.wire,busy:window.__pushMetrics.busy})');
      const sends = await Promise.all([host, sender].map((instance) => instance.evaluate('({unavailable:window.__pushSender.unavailable,paint:window.__pushSender.paint,wall:window.__pushSender.wall,idle:window.__pushSender.idle})')));
      log(`${indices[0] === indices[1] ? 'same' : 'different'} attachment measurements: ${JSON.stringify({input:sends.map((send)=>p95(send.paint)),wall:sends.map((send)=>p95(send.wall)),idle:sends.map((send)=>p95(send.idle)),render:p95(receive.paint),delivery:p95(receive.wire),busy:receive.busy})}`);
      assert.equal(receive.paint.length, 1200);
      assert.equal(receive.busy, 0);
      assert.equal(await viewer.evaluate(`document.querySelector('${STAGE} [data-asset-hash="${original.attachments[1].hash}"] img').src.split('#')[0]`), sharedUrl, 'the warmed GIF URL is shared throughout repeated display');
      for (const actor of actors) assert.equal(actor.report.rejected, 0, 'the input driver did not drop or reject presses');
      for (const send of sends) { assert.equal(send.unavailable, 0); assert.ok(Math.max(p95(send.paint), p95(send.wall)) <= 100, `input p95 ${p95(send.paint)}/${p95(send.wall)}ms`); }
      assert.ok(p95(receive.paint) <= 250, `render p95 ${p95(receive.paint)}ms`);
      assert.ok(p95(receive.wire) <= 1000, `delivery p95 ${p95(receive.wire)}ms`);
      assert.ok((await drawsSince(viewer, Date.now() - 3000)).length >= 15, 'video continues under repeated Push');
      const samplePixel = () => viewer.evaluate(`(() => {const canvas=document.querySelector('${STAGE} canvas[aria-label="${t('media.video')}"]');return [...canvas.getContext('2d').getImageData(2,2,1,1).data].join(',');})()`);
      const pixel = await samplePixel();
      await eventually('the video pixels to keep changing after the load', async () => await samplePixel() !== pixel, 3000);
      if (heard) assert.ok(heard.level(5 * PERIOD_MS) > baseline * 0.3, 'audio continues under repeated Push');
      const after = (await host.invoke('list_thread', { request: { topic, thread_id: postId, cursor: null, limit: 1 } })).items.find((post) => post.object_id === postId);
      for (const key of ['content', 'created_at', 'attachments', 'reaction_summary']) assert.deepEqual(after[key], original[key], `Push leaves ${key} unchanged`);
      log(`${indices[0] === indices[1] ? 'same' : 'different'} attachments: 1200 Pushes / ${(Date.now() - since) / 1000}s: input p95=${sends.map((send) => p95(send.paint).toFixed(1)).join('/')}ms, render p95=${p95(receive.paint)}ms, delivery p95=${p95(receive.wire)}ms, shared GIF URL unchanged`);
      await eventually('the load effects to expire', () => viewer.evaluate(`document.querySelectorAll('${STAGE} .live-overlay-push-count').length===0`), 10000);
    }
    await viewer.evaluate('(window.__pushMetrics.enabled=false,window.__pushMetrics.observer.disconnect(),window.__pushMetrics.observer=null,true)');
    const toggle = `document.querySelector('${STAGE} .live-stage-controls button[aria-pressed]')`;
    await viewer.evaluate(`(${toggle}.click(),true)`, { gesture: true });
    await eventually('hidden overlays to release Push', async () => await count(viewer) === 0);
    await viewer.evaluate(`(${toggle}.click(),true)`, { gesture: true });
    await sleep(500);
    assert.equal(await count(viewer), 0, 'showing overlays does not replay past Pushes');
    await route(sender, liveRoute(otherLive));
    await eventually('the second broadcaster to select Live B', () => sender.evaluate(`document.querySelector('${STAGE}')?.dataset.liveSessionId===${JSON.stringify(otherLive)}`));
    await halfThread(sender, postId);
    await injectCapture(sender);
    await startBroadcast(sender);
    await route(viewer, liveRoute(otherLive));
    await eventually('the viewer to select Live B', () => viewer.evaluate(`document.querySelector('${STAGE}')?.dataset.liveSessionId===${JSON.stringify(otherLive)}`));
    await watch(viewer);
    assert.equal(await count(viewer), 0, 'switching Live does not restore the previous effect');
    await click(host, label, { within: `${ACTIVE} .live-column-side` });
    await eventually('the same conversation to display Push on Live B', async () => await count(viewer) === 1);
    const other = await host.invoke('create_public_channel', { request: { topic, title: 'Other conversation' } });
    const intro = { session_id: otherLive, topic_id: topic, channel_id: channel.channel_id };
    await host.invoke('create_post', { request: { topic, channel_ref: { kind: 'public_channel', channel_id: other.channel_id }, content: 'Introducing Live B', live: intro, reply_to: null } });
    await eventually('the viewer to resolve the other conversation', () => viewer.invoke('get_public_channel', { request: { topic, channel_id: other.channel_id } }), 30000);
    await route(viewer, `/live?topic=${encodeURIComponent(topic)}&channel=${other.channel_id}&sessionId=${otherLive}&liveTopic=${encodeURIComponent(topic)}&liveChannel=${channel.channel_id}`);
    await eventually('the introduced Live to use another conversation', () => viewer.evaluate(`document.querySelector('${STAGE}')?.dataset.liveSessionId===${JSON.stringify(otherLive)} && document.querySelector('${ACTIVE}').closest('[data-column-id]').dataset.columnId.includes(${JSON.stringify(other.channel_id)})`), 30000);
    await click(host, label, { within: `${ACTIVE} .live-column-side` });
    await sleep(1000);
    assert.equal(await count(viewer), 0, 'Push does not leak into an introduction in another thread');
    await host.invoke('end_live_session', { request: { topic, session_id: live } });
    await sender.invoke('end_live_session', { request: { topic, session_id: otherLive } });
    await eventually('ended Lives to disable Push', () => host.evaluate(`document.querySelector(${JSON.stringify(button)})?.disabled===true`), 30000);
    const lastId = await viewer.evaluate('window.__pushMetrics.lastId');
    await sleep(10100);
    assert.equal(await viewer.invoke('resolve_live_overlay_push', { request: { event_id: lastId } }), null, 'expired Push metadata is reclaimed');
    log('Live switch, another conversation, layers off/on, end, and expiry preserve scope and reclaim effects');
  } finally { stop(); heard?.stop(); }
}
