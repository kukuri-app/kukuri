// 起動した instance を、利用者と同じ状態(同意・開発者モード・互いに直接つながる)へ進める(#1800)。
import { eventually } from '../web-e2e/wait.mjs';

/// 同意して runtime が動き始めるのを待ち、開発者モード(live・Metaverse の列)を有効にし、画面の言語を日本語に
/// して(文言は日本語の翻訳 file から引く。OS の言語に左右されない)読み直す。
export async function accept(instance) {
  const status = await eventually(`${instance.name} startup status`, () =>
    instance.invoke('get_desktop_startup_status').catch(() => null)
  );
  const fresh = status.status === 'consent_required';
  if (fresh) {
    await instance.invoke('accept_app_consents', {
      documents: status.documents.map(({ slug, currentVersion }) => ({ slug, version: currentVersion })),
      language: 'ja',
      ageAttested: true,
    });
  }
  await eventually(
    `${instance.name} to become ready`,
    async () => (await instance.invoke('get_desktop_startup_status')).status === 'ready',
    60_000
  );
  if (fresh) {
    // 試験の instance どうしだけでつながるよう、コミュニティノードを外し(利用者が設定で外すのと同じ)、初回プロフィールを
    // 保存する(どちらの案内の dialog も開かず、操作の focus を奪わない)。
    await instance.invoke('set_community_node_config', { request: { nodes: [], trust_node_priority: null } });
    const { active_account_id: accountId } = await instance.invoke('list_accounts');
    await instance.invoke('save_initial_profile', {
      request: {
        account_id: accountId,
        profile: { name: null, display_name: instance.name, about: null, picture_upload: null, clear_picture: false },
      },
    });
  }
  // 読み直しの前の画面には印を付け、印の無い(読み直した後の)画面を待つ。読み直しと応答が競うので、応答は待たない。
  await instance
    .evaluate(
      `(localStorage.setItem('kukuri.desktop.developer-mode', 'true'), localStorage.setItem('kukuri.desktop.locale', 'ja'),` +
        ` window.__beforeReload = true, setTimeout(() => location.reload()), true)`
    )
    .catch(() => undefined);
  await eventually(`${instance.name} to show the main screen`, () =>
    instance
      .evaluate(`!window.__beforeReload && Boolean(document.querySelector('[data-testid="control-center-trigger"]'))`)
      .catch(() => false)
  );
}

/// 互いの ticket を取り込む(接続は topic を開いたときに始まる)。
export async function connect(instances) {
  const tickets = await Promise.all(instances.map((instance) => instance.invoke('get_local_peer_ticket')));
  for (const [index, instance] of instances.entries()) {
    for (const [other, ticket] of tickets.entries()) {
      if (other !== index) await instance.invoke('import_peer_ticket', { request: { ticket } });
    }
  }
}

/// 画面で topic の列(`section` は timeline・live・game)を開く。
export async function openTopic(instance, topic, section) {
  await instance.evaluate(`(location.hash = ${JSON.stringify(`#/${section}?topic=${encodeURIComponent(topic)}`)}, true)`);
}

/// topic で `peers` 人と直接つながるまで待つ。
export async function waitForPeers(instance, topic, peers) {
  return eventually(
    `${instance.name} to reach ${peers} peer(s) on ${topic}`,
    async () => {
      const sync = await instance.invoke('get_sync_status');
      const entry = sync.topic_diagnostics.find((each) => each.topic === topic);
      return entry && entry.peer_count >= peers && (entry.joined || entry.delivery_state === 'Live') ? entry : null;
    },
    60_000
  );
}
