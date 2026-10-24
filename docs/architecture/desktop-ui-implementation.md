# Desktop UI implementation architecture

## 目的

この文書は、`DESIGN.md`の製品・視覚契約をdesktop frontendへ実装するときの配置と依存方向を定義する。数値、利用者向け状態、品質基準の正本ではない。

以下のstate・性能・検証の短い要約は[DESIGN.md](../../DESIGN.md)と[ADR 0014](../adr/0014-uiux-dev-flow.md)への配置上の案内であり、正本の変更時に同期する。

## 表示言語の配置

`src/i18n/index.ts` は同梱resourceの登録だけを行い、import時の検出・保存を行わない。`main.tsx` が `i18n/bootstrap.ts` を待ってからAppを描画する。保存値→OS候補→navigator候補→英語の優先順位と候補の正規化は `i18n/locale.ts`、端末保存と即時適用は `i18n/changeLocale.ts` が所有する。

OS取得のfrontend wrapperは `lib/api/systemLocale.ts`、読み取り専用IPCはTauri `commands/system_locale.rs` に置く。OSの取得は初回だけで、runtimeや同意ファイルに依存しない。browserとStorybookではnative IPCを使わない。

`components/LocaleSelect.tsx` は同意画面と表示設定が共有する表示・操作component。選択値、保存失敗、pendingのstateは利用側が渡す。同意送信中の変更抑止は `App.tsx::ConsentGate`、通常設定は `DesktopShellPage` が所有する。HTMLの言語同期はshell外のAppにも適用する。製品契約は [DESIGN.md](../../DESIGN.md#43-アプリ初回同意)、確認手順は [ADR 0014](../adr/0014-uiux-dev-flow.md) を参照する。

## Style bundle

Metaverseの接続取得は`useDomeConnections`がscopeと遅着responseを管理する。入室中はsessionと接続paneが同じsnapshotを使い、隣接のhost／access／asset確認は`useDomeTransitionNeighbors`が所有する。入室外は接続paneに取得controllerを一つ置く。`DomeConnectionModel`は確認済みcomponentと方向を導出し、`DomeConnectionMap`と`DomeConnectionPanel`が表示・明示操作を担当する。専用styleは`metaverse-connections.css`を既存Metaverse stylesheetの後に読み込む。

`apps/desktop/src/styles/index.css`がproductionとStorybookで共通の入口であり、local stylesheetを次の順で読み込む。この順序はcascade contractである。

1. `tokens.css`: 実行されるcustom property、theme、Tailwind alias
2. `base.css`: reset、element default、shell外の共通面
3. `shell-phase1-part1.css`〜`shell-phase1-part4.css`: shell semantic class本体。4ファイルは元の連続cascadeを番号順に維持する分割であり、domain境界ではない
4. `shell-scoped-overrides.css`: `.shell-phase1`配下だけに適用する上書き
5. `column-span-workspace.css`: desktopの可変span Column Canvas
6. `mobile-column-workspace.css`: 759px以下の1 Column＝1 viewportとmobile input ownership
7. `metaverse-camera.css`: Metaverse専用のアバター操作状態・視点調整ボタン・中央の目印
8. `metaverse-layout.css`: Metaverseの実カラム幅に応じたフォーム・HUD・chatの配置。既存componentのDOM／stateを保持する
9. `metaverse-hud.css`: Metaverseのカテゴリメニュー・詳細タブ・非表示paneとチャット入口の配置

`css-vars.test.ts`は`index.css`のlocal `@import`を直接列挙し、同梱bundle内の未定義`var()`参照を検出する。stylesheetを追加・削除するときにtest側へ別の手動一覧を追加しない。

### Primitiveとsemantic style

- `apps/desktop/src/components/ui/`: Tailwind utility、CVA、`tokens.css`でprimitiveとvariantを実装する。app固有のsemantic classを所有しない。
- `apps/desktop/src/styles/`: `.post-card`、`.composer`、`.shell-*`等、複数componentから参照するapp固有のsemantic classを置く。component内に大きなinline styleを複製しない。
- 新規styleは既存primitiveまたはvariantで表現できるか確認し、表現できないapp固有layoutだけをsemantic layerへ追加する。

### Portalとscoped override

RadixのDialog、Tooltip、DropdownMenu、Popoverは`body`直下に描画され、`.shell-phase1`scopeの外にある。同じsemantic classをportalとshell内で異なる値にする必要がある場合だけ、base値を`part*`、shell内の上書きを`shell-scoped-overrides.css`へ置く。

`shell-scoped-overrides.css`は現役のproduction layerである。重複に見える宣言を統合する場合は、portalとshellの実効値、cascade、視覚回帰を先に確認する。

## Stateとdata flow

`apps/desktop/src/shell/`は単一のZustand storeを保ちながら、責務を次に分ける。

| Area | 責務 |
|---|---|
| `slices/` | domain別のstate shapeと初期値。複数sliceを単一storeへ合成する |
| `storeSelectors.ts` | 複数hookが共有する名前付き購読slice。`useShallow`を前提にする |
| `presentation.ts` | storeに依存しない表示変換の純関数 |
| `data/`、`data/loaders/` | 取得hookとsection別loader。effectはtrigger判定とlifecycleだけを持つ |
| `viewModels/` | section別projection hookと、その合成facade |
| `actions/` | API副作用を持つ操作flow |
| `page/` | 画面合成とpage固有のDialog、focus、preview lifecycle |
| `routing/` | canonical URLと`RouteState`の同期adapter |

分類軸は意図的に異なる。slice名、action名、view model名を揃えるためだけの移動は行わない。

- selectorなしの全store購読を追加しない。単一component専用の小さな購読はcomponent内、共有購読は`storeSelectors.ts`へ置く。
- data取得とprojectionをpresentation componentへ戻さない。表示上の都合でdomain stateやIPC shapeを変更しない。
- global listener、interval、subscriptionは所有するhookまたはserviceでcleanupする。
- routingは共有target、workspace persistenceはlocal layoutを扱う。責務の詳細はADR 0031に従う。

画像・動画のブックマークでは、`shell/page/MediaAssetActions`が保存操作・メタデータDialog・focusを、`shell/page/MediaBookmarkPicker`が一覧の取得・選択・preview URLの所有を持つ。shellから注入したアカウントのAPIを`MediaBookmarksContext`で共有し、投稿とviewerの表示componentから同じ操作を使う。

## Review surfaceと検証

### Metaverseのカテゴリ合成

`MetaverseRoomView`が開閉・選択カテゴリ・focusを所有し、`MetaverseCategories`がサークルと詳細タブを描画する。`DomeCustomizationControls`は開閉より長い寿命でdraftを持ち、設定・共有物・保存操作の表示をrender propで合成する。別カテゴリへの切替でcontrollerを再mountしない。

`DomeHostingPanel`も取得と既存actionを一つだけ所有し、稼働管理・共有物・診断の表示を`MetaverseRoomPanel`へ渡す。入室中は同一のView内、入室外は既存補助面に合成する。`DomeConnectionPanel`の既存pollingと`DomeManagementPanel`の明示管理flowを再利用する。backend API、Zustand store、wire shapeへ表示stateを追加しない。

- `apps/desktop/src/stories/foundations/`: `tokens.css`を描画する確認面。設計値の正本ではない。
- component story: variant、状態、overflow、keyboard／pointer確認の入口。
- Vitest: component logic、state、CSS／design contract。
- Playwright browser test: component境界をまたぐ操作、layout、locale、state継続。
- Playwright visual test:主要surfaceのLinux／Chromium baseline。
- Tauri／WebView実機: fullscreen、native input、resource縮退等、browserだけで表現できない挙動。

変更種別ごとの必須成果物と終了条件はADR 0014、実行commandは`docs/runbooks/dev.md`を参照する。
