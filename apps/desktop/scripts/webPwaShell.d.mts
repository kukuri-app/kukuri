import type { Plugin } from 'vite';
export function pwaWorkerSource(version: string, assets: string[]): Promise<string>;
export function webPwaShell(): Plugin;
