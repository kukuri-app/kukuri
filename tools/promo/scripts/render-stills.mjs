import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';

/**
 * 媒体別の静止画 (#1041) を tools/promo/presets/stills.json から作り、出力一覧を書く。
 *
 * - LP の画面・OGP (apps/lp/public/assets/screens/) と、Product Hunt・note・X の画像
 *   (promo-artifacts/stills/) をまとめて出す
 * - 原素材は promo-artifacts/captures/ の撮影結果と、実機の静止画の取り込み結果を使う
 * - 開発者モードで撮った原素材（実験機能の画面）は、どの出力にも使わずに失敗する
 * - LP の画面 (SceneStill) は、原素材を撮った解像度のまま出す（2 倍で撮った素材は 2 倍の寸法）
 * - 出力一覧 promo-artifacts/stills/index.json と outputs.md に、寸法・形式・locale・掲載順・
 *   alt・説明・checksum・原素材を書く
 *
 *   node scripts/render-stills.mjs              # 全部
 *   node scripts/render-stills.mjs ph- note-    # id の先頭一致で絞る
 */

const PROMO = path.resolve(import.meta.dirname, '..');
const REPO = path.resolve(PROMO, '../..');
const ARTIFACTS = path.join(REPO, 'promo-artifacts');
const STILLS = path.join(ARTIFACTS, 'stills');
const REMOTION = path.join(PROMO, 'node_modules/@remotion/cli/remotion-cli.js');
const DEMO_LABEL = { ja: 'デモ画面', en: 'Demo screen' };

function fail(message) {
  process.stderr.write(`render-stills: ${message}\n`);
  process.exit(1);
}

function pngSize(file) {
  const buffer = readFileSync(file);
  if (buffer.toString('ascii', 1, 4) !== 'PNG') fail(`PNG ではない: ${file}`);
  return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
}

function sha256(file) {
  return createHash('sha256').update(readFileSync(file)).digest('hex');
}

const presets = JSON.parse(readFileSync(path.join(PROMO, 'presets/stills.json'), 'utf8')).outputs;

const ids = new Set();
for (const preset of presets) {
  if (ids.has(preset.id)) fail(`id が重複している: ${preset.id}`);
  ids.add(preset.id);
}

const filters = process.argv.slice(2);
const selected = filters.length === 0 ? presets : presets.filter((p) => filters.some((f) => p.id.startsWith(f)));
if (selected.length === 0) fail(`一致する preset が無い: ${filters.join(', ')}`);

function captureProps(preset) {
  const file = path.join(ARTIFACTS, 'captures', preset.capture, `${preset.locale}-dark`, 'props.json');
  if (!existsSync(file)) {
    fail(`${preset.id}: 原素材が無い (${path.relative(REPO, file)})。先に撮影または取り込みを行う`);
  }
  const props = JSON.parse(readFileSync(file, 'utf8'));
  // 開発者モード限定の実験機能を、既定の機能として素材に載せない (brief の INVAR-3)。
  if (props.manifest.developerMode) fail(`${preset.id}: 開発者モードで撮った原素材は使わない`);
  return props;
}

function propsFor(preset) {
  if (preset.composition === 'SceneStill') {
    // LP では見出しと説明を本文に書くので、字幕は焼き込まない。
    return { ...captureProps(preset), caption: null };
  }
  if (preset.layout === 'icon') {
    return { width: preset.width, height: preset.height, layout: 'icon', locale: preset.locale, icon: preset.icon };
  }
  const source = captureProps(preset).manifest;
  return {
    width: preset.width,
    height: preset.height,
    layout: preset.layout,
    locale: preset.locale,
    eyebrow: preset.eyebrow,
    headline: preset.headline,
    subhead: preset.subhead,
    footer: preset.footer,
    demoLabel: DEMO_LABEL[preset.locale],
    screen: {
      still: source.files.still,
      sourceWidth: source.viewport.width,
      sourceHeight: source.viewport.height,
      crop: preset.crop,
    },
  };
}

// アイコンは docs/ASSET_MANIFEST.json で管理しているアプリのアイコンをそのまま使う。
mkdirSync(path.join(ARTIFACTS, 'brand'), { recursive: true });
copyFileSync(
  path.join(REPO, 'apps/desktop/src-tauri/icons/128x128@2x.png'),
  path.join(ARTIFACTS, 'brand/icon-256.png')
);

const tmp = path.join(ARTIFACTS, 'still-props');
mkdirSync(tmp, { recursive: true });

function render(composition, props, output, scale) {
  mkdirSync(path.dirname(output), { recursive: true });
  const propsFile = path.join(tmp, `${path.basename(output)}.json`);
  writeFileSync(propsFile, JSON.stringify(props), 'utf8');
  const args = [REMOTION, 'still', 'src/index.ts', composition, output, `--props=${propsFile}`, '--log=error'];
  if (scale && scale !== 1) args.push(`--scale=${scale}`);
  try {
    execFileSync(process.execPath, args, { cwd: PROMO, stdio: 'inherit' });
  } catch {
    fail(`${path.relative(REPO, output)} を作れなかった。上の Remotion のエラーを確かめる`);
  }
}

const indexPath = path.join(STILLS, 'index.json');
const previous = existsSync(indexPath) ? JSON.parse(readFileSync(indexPath, 'utf8')).outputs : [];
const results = new Map(previous.map((entry) => [entry.id, entry]));

/** SceneStill は viewport の寸法の composition なので、2 倍で撮った原素材は 2 倍で出す。 */
function renderScale(preset, props) {
  if (preset.composition !== 'SceneStill') return 1;
  return pngSize(path.join(ARTIFACTS, props.manifest.files.still)).width / props.manifest.viewport.width;
}

for (const preset of selected) {
  const props = propsFor(preset);
  const output = path.join(REPO, preset.file);
  const scale = renderScale(preset, props);
  render(preset.composition, props, output, scale);
  const size = pngSize(output);
  const variants = [];
  for (const width of preset.variants ?? []) {
    const variantFile = output.replace(/\.png$/, `-${width}.png`);
    render(preset.composition, props, variantFile, (width / size.width) * scale);
    const variantSize = pngSize(variantFile);
    variants.push({ file: path.relative(REPO, variantFile).split(path.sep).join('/'), ...variantSize, sha256: sha256(variantFile) });
  }
  const source = preset.capture ? captureProps(preset).manifest : null;
  results.set(preset.id, {
    id: preset.id,
    media: preset.media,
    order: preset.order,
    locale: preset.locale,
    file: preset.file,
    format: 'png',
    ...size,
    sha256: sha256(output),
    variants,
    alt: preset.alt,
    description: preset.description,
    source: source
      ? {
          capture: preset.capture,
          sourceMode: source.sourceMode,
          sourceCommit: source.sourceCommit,
          sourceRelease: source.sourceRelease,
          sha256: source.checksums.still,
        }
      : { asset: 'apps/desktop/src-tauri/icons/128x128@2x.png' },
  });
  process.stdout.write(`wrote ${preset.file} (${size.width}x${size.height})\n`);
}

rmSync(tmp, { recursive: true, force: true });

const outputs = presets.map((p) => results.get(p.id)).filter(Boolean);

mkdirSync(STILLS, { recursive: true });
writeFileSync(
  indexPath,
  `${JSON.stringify({ version: 1, generatedAt: new Date().toISOString(), outputs }, null, 2)}\n`,
  'utf8'
);

const MEDIA_ORDER = ['lp', 'ogp', 'product-hunt', 'note', 'x'];
const rows = [...outputs].sort(
  (a, b) => MEDIA_ORDER.indexOf(a.media) - MEDIA_ORDER.indexOf(b.media) || a.order - b.order || a.locale.localeCompare(b.locale)
);
const lines = [
  '# 媒体別の静止画の出力一覧',
  '',
  `生成: ${new Date().toISOString()}（tools/promo/scripts/render-stills.mjs）`,
  '',
  '| 媒体 | 順 | id | locale | 寸法 | 幅違いの版 | sha256 (先頭 12) | 原素材 |',
  '| --- | --- | --- | --- | --- | --- | --- | --- |',
  ...rows.map(
    (o) =>
      `| ${o.media} | ${o.order} | \`${o.id}\` | ${o.locale} | ${o.width}×${o.height} | ${o.variants.map((v) => `${v.width}×${v.height}`).join(', ') || '—'} | \`${o.sha256.slice(0, 12)}\` | ${o.source.capture ?? o.source.asset} |`
  ),
  '',
];
writeFileSync(path.join(STILLS, 'outputs.md'), lines.join('\n'), 'utf8');
process.stdout.write(`出力一覧: ${path.relative(REPO, indexPath)} / promo-artifacts/stills/outputs.md\n`);
