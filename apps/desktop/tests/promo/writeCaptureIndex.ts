import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { CAPTURE_ROOT, PROMO_ARTIFACT_ROOT } from './promoArtifacts';

/**
 * 撮影後に、原素材全体の索引 `promo-artifacts/captures/index.json` を書く (#1039)。
 *
 * Playwright の globalTeardown として動く。個々の cut の manifest を集め、
 * 使用先と再撮影範囲を 1 か所から追えるようにする。あわせて、browser mock では
 * 撮らない shot を理由付きで明記する。
 */

/**
 * browser mock では撮らない shot。brief の shot list のうち、実機でしか事実を示せないもの。
 * mock の画面を実ネットワーク同期の証拠にしない (INVAR-2)。
 */
const NOT_CAPTURED_BY_MOCK = [
  {
    sceneId: 's4-sync',
    cutId: 'c1',
    owner: '#1040',
    reason:
      'Windows 11 実機で書いた投稿が Linux 実機に届く実同期。mock は実通信を行わないため証拠にならない',
  },
  {
    sceneId: 's4-sync',
    cutId: 'c2',
    owner: '#1040',
    reason: '招待リンクで私的チャンネルへ参加し、投稿が相手の実機に届く実同期。mock では示せない',
  },
];

type ManifestFile = { version: number; manifest: Record<string, unknown> };

function findManifests(dir: string): string[] {
  if (!existsSync(dir)) {
    return [];
  }
  const found: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) {
      found.push(...findManifests(full));
    } else if (entry === 'manifest.json') {
      found.push(full);
    }
  }
  return found.sort();
}

export default function writeCaptureIndex() {
  const manifests = findManifests(CAPTURE_ROOT);
  const cuts = manifests.map((file) => {
    const parsed = JSON.parse(readFileSync(file, 'utf8')) as ManifestFile;
    const m = parsed.manifest;
    return {
      sceneId: m.sceneId,
      cutId: m.cutId,
      locale: m.locale,
      theme: m.theme,
      sourceMode: m.sourceMode,
      sourceCommit: m.sourceCommit,
      sourceRelease: m.sourceRelease,
      developerMode: m.developerMode,
      viewport: m.viewport,
      platform: m.platform,
      capturedAt: m.capturedAt,
      files: m.files,
      checksums: m.checksums,
      manifest: path.relative(PROMO_ARTIFACT_ROOT, file).split(path.sep).join('/'),
    };
  });

  // 開発者モード限定の実験機能を素材に写さない (INVAR-3)。紛れたら索引を書かずに失敗させる。
  const misplaced = cuts.filter((cut) => cut.developerMode === true);
  if (misplaced.length > 0) {
    throw new Error(
      `promo index: 開発者モードで撮った cut がある: ${misplaced
        .map((cut) => `${String(cut.sceneId)}/${String(cut.cutId)}`)
        .join(', ')}`
    );
  }

  const index = {
    version: 1,
    generatedAt: new Date().toISOString(),
    note:
      '原素材の索引。sourceMode が mock の画面は実ネットワーク同期の証拠ではない。' +
      '開発者モード限定の機能を写した素材は含まない。',
    cuts,
    notCapturedByMock: NOT_CAPTURED_BY_MOCK,
  };

  writeFileSync(
    path.join(CAPTURE_ROOT, 'index.json'),
    `${JSON.stringify(index, null, 2)}\n`,
    'utf8'
  );
}
