import { createHash } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';

/**
 * 実機など Playwright 以外で撮った静止画を、原素材として取り込む。
 *
 * 画像は先に `promo-artifacts/captures/<sceneId>/<cutId>/<locale>-<theme>/` へ置いておく。
 * 由来は JSON の spec ファイルで渡す（Windows の shell quoting を避けるため、引数に直接書かない）。
 * 画像の寸法と SHA-256 を読み、Playwright の撮影と同じ形の manifest.json と props.json を書く。
 *
 *   node scripts/import-still.mjs device-captures/<spec>.json
 */

const ROOT = path.resolve(import.meta.dirname, '../../../promo-artifacts');

function fail(message) {
  process.stderr.write(`import-still: ${message}\n`);
  process.exit(1);
}

/** PNG / WebP のヘッダーから寸法を読む。読めない形式は取り込まない。 */
function imageSize(buffer) {
  if (buffer.toString('ascii', 1, 4) === 'PNG') {
    return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
  }
  if (buffer.toString('ascii', 0, 4) === 'RIFF' && buffer.toString('ascii', 8, 12) === 'WEBP') {
    const chunk = buffer.toString('ascii', 12, 16);
    if (chunk === 'VP8X') {
      return { width: 1 + buffer.readUIntLE(24, 3), height: 1 + buffer.readUIntLE(27, 3) };
    }
    if (chunk === 'VP8L') {
      const bits = buffer.readUInt32LE(21);
      return { width: (bits & 0x3fff) + 1, height: ((bits >> 14) & 0x3fff) + 1 };
    }
    if (chunk === 'VP8 ') {
      return { width: buffer.readUInt16LE(26) & 0x3fff, height: buffer.readUInt16LE(28) & 0x3fff };
    }
  }
  return null;
}

const specPath = process.argv[2];
if (!specPath) {
  fail('spec ファイルを渡す (例: node scripts/import-still.mjs device-captures/<sceneId>.<locale>-<theme>.json)');
}
const spec = JSON.parse(readFileSync(specPath, 'utf8'));

const required = [
  'file',
  'sceneId',
  'cutId',
  'locale',
  'theme',
  'sourceMode',
  'sourceCommit',
  'sourceRelease',
  'developerMode',
  'platform',
  'capturedAt',
];
for (const key of required) {
  if (spec[key] === undefined) {
    fail(`spec に ${key} が無い`);
  }
}
if (spec.sourceMode !== 'device') {
  fail('取り込めるのは sourceMode が device の素材だけ。browser mock は Playwright で撮る');
}

const dir = path.join(ROOT, 'captures', spec.sceneId, spec.cutId, `${spec.locale}-${spec.theme}`);
const imagePath = path.join(dir, spec.file);
if (!existsSync(imagePath)) {
  fail(`画像が無い: ${imagePath}`);
}
const image = readFileSync(imagePath);
const size = imageSize(image);
if (!size) {
  fail(`PNG / WebP として寸法を読めない: ${imagePath}`);
}
const checksum = createHash('sha256').update(image).digest('hex');
if (spec.sha256 && spec.sha256 !== checksum) {
  fail(`SHA-256 が spec と一致しない (spec ${spec.sha256} / 実物 ${checksum})。別の画像を置いていないか確かめる`);
}

const relative = path.relative(ROOT, imagePath).split(path.sep).join('/');
const manifest = {
  sceneId: spec.sceneId,
  cutId: spec.cutId,
  sourceCommit: spec.sourceCommit,
  sourceRelease: spec.sourceRelease,
  sourceMode: 'device',
  locale: spec.locale,
  theme: spec.theme,
  developerMode: spec.developerMode,
  // 静止画は画像そのものの寸法で出力する。縮小の検査は録画にだけ意味があるので同値にする。
  viewport: size,
  recordSize: size,
  platform: spec.platform,
  fonts: [],
  clip: null,
  capturedAt: spec.capturedAt,
  files: { video: null, still: relative },
  checksums: { video: null, still: checksum },
  // 画像に焼き込まず、画像の外（LP の本文など）に置く説明文。媒体側がここから読む。
  externalCaption: spec.externalCaption ?? null,
  notes: spec.notes ?? [],
};

writeFileSync(
  path.join(dir, 'manifest.json'),
  `${JSON.stringify({ version: 1, manifest }, null, 2)}\n`,
  'utf8'
);
// props の caption は画像に焼き込む字幕。画像の外に置く説明は manifest の externalCaption を使う。
writeFileSync(
  path.join(dir, 'props.json'),
  `${JSON.stringify({ manifest, caption: spec.caption ?? null, demoBadge: true, fps: 30 }, null, 2)}\n`,
  'utf8'
);

process.stdout.write(
  `imported: ${relative} (${size.width}x${size.height}, sha256 ${checksum.slice(0, 12)}...)\n`
);
