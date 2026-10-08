# 告知素材の制作 runbook

LP・Product Hunt・note・X 向けの素材を、撮影（Playwright）とレンダリング（Remotion）で再生成する手順。

制作仕様の正本は [LP・告知素材の共通brief](../progress/2026-09-15-promo-lp-brief.md)。本書は実行手順だけを扱う。開始前に今回納品するcut・言語・媒体・形式と確認条件を固定し、該当成果物が揃えば終了する。未依頼の別媒体・別テーマや追加のpolishを増やさない。

## 前提

| 項目 | 値 |
| --- | --- |
| Node | `^20.19.0 \|\| >=22.12.0`（検証時 v22.14.0） |
| pnpm | 10.16.1 |
| Playwright | `@playwright/test` 1.62.1（`apps/desktop` の既存依存をそのまま使う） |
| Remotion | 4.0.526（`tools/promo` の専用依存） |
| 撮影 browser | Playwright 同梱の Chromium（検証時 151.0.7922.34） |

`tools/promo` は root の workspace に含めない独立パッケージで、通常の desktop build・release build には入らない。

### Remotion のライセンス

Remotion は個人および従業員3名以下の営利組織に Free License を許諾し、商用の映像・画像制作に使える。4名以上の組織は Company License の購入が必要になる。

- 出典: [LICENSE.md](https://github.com/remotion-dev/remotion/blob/main/LICENSE.md) の "Free License" → "Eligibility"（an individual / a for-profit organization with up to 3 employees）と "Allowed use cases"
- 出典: [remotion.pro/license](https://www.remotion.pro/license)「Remotion is free to use for individuals and companies up to three people」
- 確認日: 2026-09-18
- 現状: kukuri の開発者は1名であり Free License の条件内。人数が4名以上になった時点で Company License へ切り替える。

## 出力先

すべての中間物と成果物は repository root の `promo-artifacts/` に集約する。`.gitignore` 済みで、git へは commit しない。

```
promo-artifacts/
  captures/
    index.json                                原素材全体の索引（撮影のたびに作り直す）
    <sceneId>/<cutId>/<locale>-<theme>/       1 カットの原素材
      still.png                               静止画
      video.webm                              録画（VP8, viewport と同解像度）
      manifest.json                           原素材の由来（version 1）
      props.json                              Remotion へ渡す props
  stills/                                     媒体別の静止画（Product Hunt・note・X）と出力一覧
    index.json / outputs.md                   寸法・形式・locale・掲載順・alt・説明・checksum・原素材
    review.html                               縮小表示で確かめるためのページ（任意）
  brand/                                      静止画に使うアプリのアイコンの写し
  renders/                                    Remotion の出力（PNG / MP4）
  playwright-output/                          Playwright 自身の artifact
```

LP が配信する画像（画面の静止画・幅違いの版・OGP）は `apps/lp/public/assets/screens/` に出し、git で管理する。

同じ場面を言語・テーマ違いで撮っても互いに上書きしないよう、カットの下を `<locale>-<theme>` で分ける（例: `captures/s3-private-channel/c3/ja-dark/`）。

既存の `apps/desktop/test-results/` と `tests/playwright/__screenshots__/`（視覚回帰 baseline）には書き込まない。

## 1. 依存を入れる

```bash
cd apps/desktop && npx pnpm@10.16.1 install
```

```bash
cd tools/promo && npx pnpm@10.16.1 install
```

撮影用の Chromium が入っていない環境では、先に取得する。

```bash
cd apps/desktop && npx pnpm@10.16.1 exec playwright install chromium
```

初回の `remotion still` / `remotion render` は、Remotion が使う Headless Shell（約113MB）を取得する。これは Playwright の Chromium とは別で、レンダリング側が自動で取りに行く。

## 2. 撮影する

```bash
cd apps/desktop && npx pnpm@10.16.1 exec playwright test --config=playwright.promo.config.ts
```

- 既存の `playwright.config.ts` とは port（4177 / 4176）、build 出力（`dist-promo` / `dist`）、test 選択（`tests/promo` / `tests/playwright`）、artifact 出力先が分かれている。既存の test 選択と視覚回帰 baseline は変更しない。
- `worker=1`、`retries=0`。失敗した撮影を retry で上書きせず、失敗として報告する。
- 撮影のたびに対象 cut のディレクトリを作り直すので、前回の残骸が新しい撮影の成果物に混ざらない。
- ブラウザは毎回使い捨ての context で動く。開発機の既存プロファイルは読み書きしない。

撮影対象の release を manifest に残す場合は、環境変数で渡す。

```bash
cd apps/desktop && KUKURI_PROMO_SOURCE_RELEASE=v0.2.5-preview.3 npx pnpm@10.16.1 exec playwright test --config=playwright.promo.config.ts
```

`KUKURI_PROMO_SOURCE_COMMIT` を指定しない場合は、作業ツリーの `git rev-parse HEAD` を記録する。

### 撮る場面

`tests/promo/` には次の spec がある。

| spec | 内容 |
| --- | --- |
| `lp.spec.ts` | LP の画面（L0 Hero、L1 話題を見つける、L2 会話に加わる、L3 小さな輪で続ける）を JA / EN で撮る。スマートフォンの幅（430×760）を 2 倍の解像度で撮る。計 4 カット × 2 言語（#1668） |
| `scenes.spec.ts` | Product Hunt・note・X 用の 3 場面（S0 Hero 候補、S1 話題を選ぶ、S2 公開で会話する、S3 私的チャンネルへ移る）を JA / EN で撮る。計 9 カット × 2 言語 |
| `guards.spec.ts` | 撮影の guard が、不完全な画面を素材として採用しないことを確かめる。撮影の出力には書かない |
| `smoke.spec.ts` | 制作環境が通ることを確かめる最小経路（#1038）。素材ではない |

特定の spec や場面だけを撮るときは、ファイル名や `-g` で絞る。

```bash
cd apps/desktop && npx pnpm@10.16.1 exec playwright test --config=playwright.promo.config.ts scenes.spec.ts -g "S3"
```

場面の台本と scene ID の対応は [brief の shot list](../progress/2026-09-15-promo-lp-brief.md) を正本とする。撮影の手順と入力は次の 2 つにまとまっている。

- `tests/promo/fixtures/demoStory.ts`: 合成のデモ物語（話題 `kukuri:topic:dev`、デモ参加者 2 名、会話、時刻）。実在の利用者のデータは使わない
- `tests/promo/fixtures/captureScene.ts`: 1 カットの撮影手順と guard

撮影は開発者モードを無効のまま行い、実験機能（Dome など）の場面は撮らない。2 台の実機間の実同期は browser mock では撮らない（#1040 が実機で撮る）。どの shot を撮っていないかは `captures/index.json` の `notCapturedByMock` に理由付きで残る。

### 不完全な画面を採用しない

撮影の直前に次を確かめ、満たさなければ撮影を失敗させる。

| 確認 | 失敗の条件 |
| --- | --- |
| フォント | 15 秒以内に `document.fonts.status` が `loaded` にならない |
| 画像 | 読み込みが終わっていない、またはデコードできない画像がある |
| 文字 | 画面に文字が 1 つも無い |
| 文字化け | 置換文字（U+FFFD）が出ている |
| 画面の安定 | 150ms 間隔の連続 2 回の撮影が、3 秒以内に一度も一致しない |

静止画は動きを止め、入力欄のカーソルを隠し、画面が落ち着いてから撮る。そのため同じ fixture・設定で撮り直すと、静止画はバイト単位で同じになる（#1039 で 3 回撮影して 18 カットすべて一致を確認した）。操作の録画は実時間で動くので、撮り直すと細部が変わる。

### 実機の静止画を取り込む

配布版の実機など、Playwright 以外で撮った静止画（PNG / WebP）は、由来を付けて原素材として取り込む。

1. 画像を `promo-artifacts/captures/<sceneId>/<cutId>/<locale>-<theme>/` に置く。
2. 由来を `tools/promo/device-captures/<sceneId>.<locale>-<theme>.json` に書く。画像は git に入れず、この spec だけを commit する。
3. 取り込む。

```bash
cd tools/promo && node scripts/import-still.mjs device-captures/<sceneId>.<locale>-<theme>.json
```

スクリプトは画像の寸法と SHA-256 を読み、Playwright の撮影と同じ形の `manifest.json` と `props.json` を書く。spec に `sha256` があれば照合し、違う画像なら失敗する。取り込めるのは `sourceMode` が `device` の素材だけ。

開発者モードを有効にした素材（実験機能の画面）は、どの出力にも使わない。紛れていると、撮影後の索引（`captures/index.json`）の作成と `render-stills.mjs` が失敗する。

### 撮影が途中で失敗したとき

失敗したcutと原因を確認してから、その対象に絞って再実行する。対象ディレクトリは作り直されるため、古い素材が新しい撮影として残ることはない。原因や前提が変わらないまま同じ失敗を繰り返さない。`video.webm` が空の場合は撮影が失敗として報告され、manifest は書かれない。

## 3. 編集を確認する（Remotion Studio）

```bash
cd tools/promo && npx pnpm@10.16.1 studio --props=../../promo-artifacts/captures/smoke/c1/ja-dark/props.json
```

## 4. 静止画を出す

1 カットだけを確かめるときは、原素材の props をそのまま渡す。

```bash
cd tools/promo && npx pnpm@10.16.1 still SceneStill ../../promo-artifacts/renders/<出力名>.png --props=../../promo-artifacts/captures/<sceneId>/<cutId>/<locale>-<theme>/props.json
```

### 媒体別の静止画を作る

LP・OGP・Product Hunt・note・X の画像は、`tools/promo/presets/stills.json` の固定出力一覧から一括で作る（#1041）。

```bash
cd tools/promo && node scripts/render-stills.mjs
```

id の先頭一致で絞れる（例: `node scripts/render-stills.mjs ph- note-`）。

| composition | 用途 |
| --- | --- |
| `SceneStill` | LP の画面。原素材に「デモ画面」「実機」の表記を重ねる。字幕は焼き込まない。2 倍で撮った原素材は 2 倍の寸法で出し、`variants` の幅で縮小版も作る |
| `PromoStill` | OGP・Product Hunt・note・X。`layout` は `split`（文字と画面を左右に並べる）、`header`（見出し中心）、`icon`（アイコンだけ）。画面は原素材を `crop` の範囲で切り抜いて拡大するだけで、UI を作り直さない |

preset の主な項目は、寸法（`width`・`height`）、原素材（`capture`）、切り抜き（`crop`、原素材 1600×1000 の座標）、文言（`eyebrow`・`headline`・`subhead`・`footer`）、掲載順（`order`）、`alt`、説明（`description`）。日本語の見出しは文節で折り返し、区切りたい位置があれば文言に `\n` を入れる。

スクリプトは次のときに失敗し、失敗した画像を完成扱いにしない。

| 条件 | 結果 |
| --- | --- |
| 文字が枠からあふれる | `promo still: 文字が枠からあふれている (...)`。文言を短くするか寸法を見直す |
| 原素材が無い | `render-stills: <id>: 原素材が無い (...)`。先に撮影または取り込みを行う |
| 開発者モードで撮った原素材を使う | `render-stills: <id>: 開発者モードで撮った原素材は使わない` |

出力一覧は `promo-artifacts/stills/index.json` と `outputs.md` に書かれる。寸法は制作時の preset なので、投稿の直前に各媒体の現行の要件を確かめる。

## 5. 動画を出す

```bash
cd tools/promo && npx pnpm@10.16.1 render SceneClip ../../promo-artifacts/renders/<出力名>.mp4 --props=../../promo-artifacts/captures/<sceneId>/<cutId>/<locale>-<theme>/props.json
```

出力は H.264 / yuv420p / bt709 / 30fps。寸法と長さは props の manifest（`viewport` と `clip`）から決まるので、composition 側に固定値を持たせない。

### 出力を確認する

```bash
cd tools/promo && npx pnpm@10.16.1 exec remotion ffprobe ../../promo-artifacts/renders/<出力名>.mp4
```

`Stream #0:0` の行が `h264`、`yuv420p`、意図した解像度、`30 fps` であること、`Duration` が想定の長さであることを確認する。

## 6. 片付ける

```bash
cd tools/promo && npx pnpm@10.16.1 clean
```

render 出力と Playwright artifact だけを消し、撮り直しに時間のかかる原素材は残す。原素材も消す場合は `--captures` を付ける。

```bash
cd tools/promo && npx pnpm@10.16.1 clean -- --captures
```

## props と manifest の契約

`props.json` は撮影時に自動生成され、次の形を持つ。

| key | 意味 |
| --- | --- |
| `manifest` | 原素材の由来一式（下表） |
| `caption` | 焼き込む字幕。無音で理解できるようにするため、動画では原則入れる |
| `demoBadge` | デモ表記を出すか。既定 `true` |
| `fps` | 出力 fps。既定 30 |

manifest の主な項目。

| key | 意味 |
| --- | --- |
| `sceneId` / `cutId` | brief の shot list と対応する場面・カット |
| `sourceCommit` / `sourceRelease` | 撮影対象の commit と release tag |
| `sourceMode` | `mock`（browser mock 撮影）または `device`（実機撮影） |
| `locale` / `theme` / `developerMode` | 画面の言語・テーマ・開発者モードの状態 |
| `viewport` / `recordSize` | 撮影 viewport と録画解像度。異なる場合は縮小が起きているので失敗として扱う |
| `platform` | 撮影した OS・browser・browser の版 |
| `clip` | 採用する区間（ms） |
| `files` / `checksums` | 出力ファイルの相対パスと SHA-256 |

props は必ず JSON ファイルで渡す（`--props=<path>`）。inline の JSON 文字列で渡す運用はしない（Windows の shell quoting で壊れるため）。

### 欠落・不正な入力は失敗する

次はいずれも exit code 1 で終わり、出力ファイルを作らない。

| 入力 | 結果 |
| --- | --- |
| `--props` を渡さない | `promo props: manifest が無い。--props=<manifest を含む JSON> を渡す` |
| `recordSize` が `viewport` と異なる | `promo manifest: 録画解像度が viewport と一致しない (viewport WxH / record WxH)` |
| `SceneClip` に `video` の無い manifest | `promo: scene <id>/<cut> に video が無い` |
| `clip` の長さが 0 以下 | `promo props: clip の長さが 0 以下` |

## 原素材の保管と復元

- 原素材は `promo-artifacts/captures/` に置き、git へは commit しない。大きな動画を通常の git 履歴へ積まない。
- 各 cut の `manifest.json` に SHA-256 があるので、別の場所へ退避したファイルの同一性を確認できる。
- 期限付きの CI artifact を唯一の保管先にしない。採用した素材は、撮影者が保持する別の保管先（外付けドライブ・オブジェクトストレージなど）へ退避し、`manifest.json` を一緒に保管する。
- 退避先から戻すときは `captures/<sceneId>/<cutId>/<locale>-<theme>/` の構造ごと戻し、checksum を照合してから render する。
- どの原素材がどの場面・言語で、どの commit / release から撮ったかは `captures/index.json` で一覧できる。
- 原素材を失った場合は、manifest の `sourceCommit` / `sourceRelease` / `locale` / `theme` / `viewport` を同じにして撮り直す。

## 関連

- 制作仕様の正本: [LP・告知素材の共通brief](../progress/2026-09-15-promo-lp-brief.md)
- 視覚仕様: [DESIGN.md](../../DESIGN.md)
- 開発手順全般: [dev.md](dev.md)
