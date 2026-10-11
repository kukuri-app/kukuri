import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

export async function pwaWorkerSource(version, assets) {
  const source = await readFile(new URL('./pwa-service-worker.mjs', import.meta.url), 'utf8');
  return source.replace('/* KUKURI_PWA_BUILD */ null', JSON.stringify({ version, assets }));
}

export function webPwaShell() {
  return {
    name: 'web-pwa-shell',
    async writeBundle(options, bundle) {
      const hash = createHash('sha256');
      for (const name of Object.keys(bundle).sort()) {
        const output = bundle[name];
        hash.update(name).update(output.type === 'chunk' ? output.code : output.source);
      }
      const root = path.resolve(options.dir);
      for (const name of ['manifest.webmanifest', '_headers']) hash.update(name).update(await readFile(path.join(root, name)));
      const version = hash.digest('hex');
      const html = await readFile(path.join(root, 'index.html'), 'utf8');
      await writeFile(path.join(root, 'index.html'), html.replace('</head>', `<meta name="kukuri-shell-build" content="${version}"></head>`));
      const assets = ['/', '/manifest.webmanifest', ...Object.keys(bundle).filter(name => name !== 'index.html').map(name => `/${name}`)];
      await writeFile(path.join(root, 'sw.js'), await pwaWorkerSource(version, assets));
    },
  };
}
