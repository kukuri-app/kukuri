import distributionCommunityNodes from '../../src-tauri/distribution/community-nodes.json';
import type { RuntimeEvent } from './api/types';

/// Web の build（`VITE_KUKURI_TARGET=web`）では、command と event をブラウザ内の web-runtime（WASM）へ向ける
/// （ADR 0056 §6・ADR 0060 §1）。Tauri の build ではこの module の runtime を読み込まない。
export const IS_WEB_RUNTIME = import.meta.env.VITE_KUKURI_TARGET === 'web';

type WebRuntime = typeof import('@kukuri/web-runtime');

let runtime: WebRuntime | null = null;
const listeners = new Set<(event: RuntimeEvent) => void>();

/// WASM を初期化し、runtime の起動を始める。起動（保存先の版の更新を含む）の終わりは待たない。画面は native と同じく
/// 起動の状態を読みながら待つ（ADR 0059 §1）。Community Node の初期設定は native の配布と同じもの（開発・試験は
/// `VITE_KUKURI_COMMUNITY_NODE_BASE_URL` で替える）。
export async function startWebRuntime(): Promise<void> {
  const module = await import('@kukuri/web-runtime');
  await module.default();
  runtime = module;
  const baseUrl = import.meta.env.VITE_KUKURI_COMMUNITY_NODE_BASE_URL;
  void module
    .start({
      communityNodeConfig: baseUrl ? { nodes: [{ base_url: baseUrl }] } : distributionCommunityNodes,
    })
    .then(() => {
      // web-runtime の callback は外せないので、1 つだけ渡して購読者へ配る。
      module.listen((event: RuntimeEvent) => {
        for (const listener of listeners) listener(event);
      });
    })
    .catch(() => undefined);
}

export function invokeWebRuntime<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!runtime) {
    return Promise.reject({ code: 'command_failed', message: 'the web runtime is not loaded' });
  }
  return runtime.invoke(command, args) as Promise<T>;
}

export function listenWebRuntimeEvents(listener: (event: RuntimeEvent) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
