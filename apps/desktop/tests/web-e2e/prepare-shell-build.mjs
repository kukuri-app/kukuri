import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { build } from 'vite';

// 同じWASM・fixtureで2つの実Web buildを比較する。entryだけを試験操作で切り替え、
// hashed assetsは双方を配信する。使用中の旧assetのoffline保持は別のcache contractで判定する。
const originalPackage = await readFile('package.json', 'utf8');
const index = await readFile('dist-web/index.html');
const worker = await readFile('dist-web/sw.js');
try {
  const next = JSON.parse(originalPackage);
  next.version += '-pwa-e2e-next';
  await writeFile('package.json', JSON.stringify(next));
  await build({ build: { emptyOutDir: false } });
  await mkdir('dist-web/fixture', { recursive: true });
  await writeFile('dist-web/fixture/shell-next-index.html', await readFile('dist-web/index.html'));
  await writeFile('dist-web/fixture/shell-next-sw.js', await readFile('dist-web/sw.js'));
} finally {
  await writeFile('package.json', originalPackage);
  await writeFile('dist-web/index.html', index);
  await writeFile('dist-web/sw.js', worker);
}
