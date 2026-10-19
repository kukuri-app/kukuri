/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_KUKURI_DESKTOP_MOCK?: string;
  readonly VITE_KUKURI_DISTRIBUTION?: string;
  /** `web` なら Web の build（ADR 0060 §1）。 */
  readonly VITE_KUKURI_TARGET?: string;
  /** Web の build の Community Node の初期設定を、配布の設定から替える（開発・試験）。 */
  readonly VITE_KUKURI_COMMUNITY_NODE_BASE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

/** web-runtime の `wasm-bindgen --target web` の出力（Web の build だけが読む。vite.config.ts の alias）。 */
declare module '@kukuri/web-runtime' {
  export default function init(): Promise<unknown>;
  export function start(config: unknown): Promise<unknown>;
  export function invoke(command: string, args: unknown): Promise<unknown>;
  export function prepare_attachment(blob: Blob): Promise<{ owner: string; hash: string }>;
  export function invoke_with_attachments(command: string, args: unknown, sources: unknown[]): Promise<unknown>;
  export function push_live_video(sessionId: string, keyframe: boolean, ageUs: number, data: Uint8Array): Promise<void>;
  export function listen(callback: (event: never) => void): void;
}
