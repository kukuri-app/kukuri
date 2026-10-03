import distributionCommunityNodes from '../../src-tauri/distribution/community-nodes.json';
import type { RuntimeEvent } from './api/types';

/// Web の build（`VITE_KUKURI_TARGET=web`）では、command と event をブラウザ内の web-runtime（WASM）へ向ける
/// （ADR 0056 §6・ADR 0060 §1）。Tauri の build ではこの module の runtime を読み込まない。
export const IS_WEB_RUNTIME = import.meta.env.VITE_KUKURI_TARGET === 'web';

type WebRuntime = typeof import('@kukuri/web-runtime');

let runtime: WebRuntime | null = null;
const listeners = new Set<(event: RuntimeEvent) => void>();

/// WASM を初期化し、runtime を始める。Community Node の初期設定は native の配布と同じもの（開発・試験は
/// `VITE_KUKURI_COMMUNITY_NODE_BASE_URL` で替える）。
export async function startWebRuntime(): Promise<void> {
  const module = await import('@kukuri/web-runtime');
  await module.default();
  runtime = module;
  const baseUrl = import.meta.env.VITE_KUKURI_COMMUNITY_NODE_BASE_URL;
  await module.start({
    communityNodeConfig: baseUrl ? { nodes: [{ base_url: baseUrl }] } : distributionCommunityNodes,
  });
  // web-runtime の callback は外せないので、1 つだけ渡して購読者へ配る。
  module.listen((event: RuntimeEvent) => {
    for (const listener of listeners) listener(event);
  });
}

export function invokeWebRuntime<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!runtime) {
    return Promise.reject({ code: 'command_failed', message: 'the web runtime is not loaded' });
  }
  // DIAG(一時)
  if (/post|channel|follow|direct_message/.test(command)) {
    const diagId = (diagSeq += 1);
    const started = performance.now();
    console.info(`DIAGJS start #${diagId} ${command}`);
    return (runtime.invoke(command, args) as Promise<T>).then(
      (value) => {
        const items = command === 'list_joined_private_channels'
          ? ` items=${((value as { items?: { channel_id: string; current_epoch_id?: string }[] })?.items ?? []).map((item) => `${item.channel_id.slice(-8)}@${(item.current_epoch_id ?? '').slice(-6)}`).join(',')}`
          : '';
        console.info(`DIAGJS end #${diagId} ${command} ok ${Math.round(performance.now() - started)}ms${items}`);
        return value;
      },
      (error: unknown) => {
        console.info(`DIAGJS end #${diagId} ${command} err ${Math.round(performance.now() - started)}ms ${JSON.stringify(error).slice(0, 200)}`);
        throw error;
      }
    );
  }
  return runtime.invoke(command, args) as Promise<T>;
}
let diagSeq = 0;

export function listenWebRuntimeEvents(listener: (event: RuntimeEvent) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
