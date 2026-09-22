# Issue #1278: プロフィールの無限スクロール

## 対象

- Scope revision: `1278-2026-09-22-v1`
- 基準 commit: `b39d3b606d835aaae9580b13b85d3742e78cb2b6`
- リスク区分: B
- 対象: desktop の自分・他ユーザーのプロフィール公開投稿

## 実装結果

- 既存の `TimelineFeed` の sentinel とbutton fallbackをプロフィールへ接続し、`next_cursor`から固定件数ずつ取得する。
- 行が0件でもcursorを進め、非表示の投稿が続く範囲の先へ到達できる。
- 追加取得中の重複要求と、refresh後に届く古い応答を無視する。
- 追加取得失敗では表示済み投稿とcursorを保持し、observerによる自動再試行を止めて明示的な再試行へ切り替える。
- refreshはADR 0052の`hasReadPastHeadPage`を自分・他ユーザーのプロフィールにも適用する。

## AC / INVAR evidence

| 条件 | 実装・test |
| --- | --- |
| AC-1 / AC-2 | `loadMoreProfileTimeline`、`loadMoreAuthorTimeline`、`DesktopShellPage.profile.test.tsx`、`useDesktopShellSectionLoaders.test.tsx` |
| AC-3 | `hasReadPastHeadPage`と`mergeRefreshedVisiblePosts`のprofile適用、`profilePaginationModel.test.tsx` |
| AC-4 | 対象別request id・cursor・loading state、遅着応答とauthor分離のloader test |
| AC-5 | `TimelineFeed.loadMoreError`、失敗・明示retry test |
| AC-6 | 固定seed 100通りの`profilePaginationModel.test.tsx`とProfile Column統合test |
| INVAR-1 / INVAR-3 | backendと取得上限は変更せず、1回の取得は`VISIBLE_TIMELINE_LIMIT` |
| INVAR-2 | refresh merge・失敗時保持・stale response test |

## Validation

- `cargo xtask desktop-ui-check`: PASS
  - ESLint / TypeScript: PASS
  - Vitest: 243 files、1,978 tests PASS
  - Storybook build: PASS
  - Playwright browser: PASS
  - Playwright visual: PASS（Windowsではsnapshot比較を行わない到達確認）
- `profilePaginationModel.test.tsx`: 固定seed 100通り PASS
- `git diff --check`: PASS
