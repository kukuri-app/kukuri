# LP の公開 runbook

`kukuri.app` の LP（`apps/lp/public`）を Cloudflare Pages で配信する手順。制作仕様の正本は [LP・告知素材の共通brief](../progress/2026-09-15-promo-lp-brief.md)。

LP は依存もビルドも無い静的ファイルで、`apps/lp/public` をそのまま配信する。

| パス | 内容 |
| --- | --- |
| `/` | 日本語 |
| `/en/` | 英語 |
| `/privacy/` | クライアント用プライバシーポリシー全文（日本語正文） |
| `/terms/` | クライアント用利用規約全文（日本語正文） |
| `/assets/` | CSS・JS・画像（画面の静止画と OGP は `assets/screens/`） |
| `/.well-known/nostr.json` | `@kukuri.app` の NIP-05 の名前と公開鍵の対応（ADR 0064）。名前を足すときはこの file に追記する |
| `_headers` | Cloudflare Pages のセキュリティヘッダーとキャッシュ。`nostr.json` には Web 版から読めるよう `Access-Control-Allow-Origin: *` を付ける |

## 画像を作り直す

画面の静止画（幅違いの版を含む）と OGP は、撮影済みの原素材から作る。撮影と実機の静止画の取り込み、出力の設定は [告知素材の制作 runbook](promo-production.md) の「媒体別の静止画を作る」を参照する。LP の分だけ作るときは id で絞る。

```bash
cd tools/promo && node scripts/render-stills.mjs lp- ogp-
```

## 手元で確認する

法務ページは`docs/legal/privacy-policy.md`と`docs/legal/terms-of-service.md`を正本とし、生成HTMLを直接編集しない。正文を変更したら同期する。英語LPも日本語正文へのリンクであることを明記する。

```bash
node apps/lp/scripts/sync-legal.mjs
node apps/lp/scripts/sync-legal.mjs --check
node --test apps/lp/scripts/legal.test.mjs
```

LP Contracts CIが本文と生成HTMLのずれを検査する。公開前には既存のrelease同期検査も実行する。

```bash
python -m http.server 4180 --bind 127.0.0.1 --directory apps/lp/public
```

`http://127.0.0.1:4180/` と `http://127.0.0.1:4180/en/` を開く。

## Cloudflare Pages へ公開する

Cloudflare の資格情報は公開作業をする人が扱う。

### 初回（wrangler を使う場合）

```bash
npx wrangler login
```

```bash
npx wrangler pages project create kukuri-lp --production-branch main
```

先に preview へ出して確認する。

```bash
npx wrangler pages deploy apps/lp/public --project-name kukuri-lp --branch preview
```

表示された preview URL で、日本語・英語、OS ごとの主ボタン、ダウンロードのリンク、スマートフォン表示を確認してから本番へ出す。

```bash
npx wrangler pages deploy apps/lp/public --project-name kukuri-lp --branch main
```

### ダッシュボードを使う場合

Workers & Pages → Create → Pages → Upload assets で、`apps/lp/public` フォルダーをそのまま上げる。

### `kukuri.app` を向ける

Pages プロジェクトの Custom domains で `kukuri.app` を追加する。`kukuri.app` の DNS は同じ Cloudflare アカウントにあるので、レコードは Pages が追加する。`api.kukuri.app` のレコードは変更しない。

公開後に次を確認する。

- `https://kukuri.app/` と `https://kukuri.app/en/` が開く
- 主ボタンが OS に合っている（Windows は Microsoft Store、Linux は Linux 版、Mac・スマートフォンはブラウザ版）
- ダウンロードのリンクが GitHub Release の配布物を指している
- `https://kukuri.app/terms/` と `https://kukuri.app/privacy/` がクライアント用全文を表示する。Community Node専用のリンクと混同しない
- OGP（`https://kukuri.app/assets/screens/ogp-ja.png`）が取得できる

## 版を上げるとき

LP が案内する release は `apps/lp/release.json` の 1 か所で管理する。ダウンロードのリンク・配布物の名前・本文の版表記は各 HTML に直接書いてあるので、`release.json` を書き換えてからスクリプトで両言語へ反映する。

1. 新しい release の配布物の名前が `kukuri_<version>_...` / `kukuri-cli_<version>_...` の形のままか確かめる（`gh release view <tag>`）。
2. `apps/lp/release.json` の `tag`・`version`・`commit`・`publishedAt` を更新する。
3. 反映して、検査する。

```bash
node apps/lp/scripts/sync-release.mjs
```

```bash
node apps/lp/scripts/sync-release.mjs --check
```

4. ダウンロードのリンクがすべて 200 を返すことを確かめる。
5. 画面の撮り直しが要るかを brief で確認する。画面に版は写っていないので、UI が変わっていなければ撮り直さない。
6. 再 deploy する（上の「Cloudflare Pages へ公開する」）。

`--check` は、HTML に `release.json` と違う版が 1 つでも残っていれば失敗する。

## CSS・JS を変えたとき

Cloudflare の配信キャッシュは `/assets/` を数時間保持する（2026-09-18 の確認で `max-age=14400`）。同じ URL のままだと、deploy 後も古い CSS・JS が返り続ける。HTML はキャッシュされないので、HTML から CSS・JS を内容のハッシュ付きの URL（`/assets/site.css?v=<ハッシュ>`）で参照し、変更のたびに URL が変わるようにしている。

`site.css` か `site.js` を変えたら、deploy の前に反映スクリプトを流す。ハッシュの付け直しも同じスクリプトが行う。

```bash
node apps/lp/scripts/sync-release.mjs
```

`--check` は、ハッシュが今の内容と合っていなければ失敗する。ハッシュは改行をそろえてから取るので、Windows と Linux の checkout で同じ値になる。

画像（`assets/screens/`）は URL にハッシュを付けていない。画像を差し替えたときは、Cloudflare のダッシュボードの Caching → Configuration → Custom Purge で、差し替えた画像の URL を purge する。
