# Web クライアントの配信の runbook

インストール不要の Web クライアント（ADR 0056・0060）の配信の artifact を作り、静的な host に置く手順。本番の公開（URL・DNS・公開の時期）は、この手順とは別の操作として扱う（#1220 の Non-goals）。

## artifact を作る

```bash
cargo xtask web-build
```

出力は `apps/desktop/dist-web`（HTML・JS・CSS・WASM・`_headers`）。

必要なもの:

- Rust の wasm32 の target（`rustup target add wasm32-unknown-unknown`）。
- clang と llvm-ar（wasm の C の依存の build に使う）。Windows の手元には無いので、Linux（CI と同じ ubuntu）で作る。
- wasm-bindgen-cli。`Cargo.lock` の `wasm-bindgen` と同じ版にする。
- Node.js と pnpm。先に `apps/desktop` で依存を入れておく（`npx pnpm@10.16.1 install --frozen-lockfile`）。

中身:

- WASM は LTO の profile（root の `Cargo.toml` の `web-release`）で build し、wasm-bindgen で名前の section を除く。Cloudflare Pages の 1 file の上限（25 MiB）の下に収めるためで、機能は変わらない。2026-10-04 の測定では 20.3 MB だった（既定の release のままだと 33.8 MB）。
- Community Node の初期設定は、native の配布と同じ（`apps/desktop/src-tauri/distribution/community-nodes.json`）。
- `_headers`（`apps/desktop/web-public/_headers`）は、CSP と、`.wasm` の `Content-Type: application/wasm` を付ける（ADR 0060 §2）。
- metaverse の資源（`apps/desktop/public`）は含めない。Web では使わない。

build の後に、各 file が 25 MiB 以下であることを確かめ、超えたら失敗する。

## 試験で確かめていること

実ブラウザの E2E（`cargo xtask web-e2e`、CI の `linux-web-e2e`）は、同じ `_headers` の header を付けて配信した Web の build で動く。WASM の起動、鍵の export・import の Worker、画像の表示を通し、CSP の違反が無いことを確かめる。

E2E の build は配信の build と次の点が違う。

- 試験の Community Node を埋め込む。
- 速さと失敗時の調べやすさのため、LTO と名前の section の除去をしない。
- 試験の Community Node と relay は http・ws の 127.0.0.1 なので、fixture が CSP の connect-src にだけそれを足す。

## 置く先の条件

- HTTPS で配る（IndexedDB の永続化の要求や Web Crypto は、安全な context でだけ使える）。
- LP（`kukuri.app`）と別の origin に置く。同じ origin の第三者の script から、IndexedDB の鍵の保存を守るため（ADR 0059 §1）。
- `_headers` の header を付ける。Cloudflare Pages は `_headers` をそのまま読む。他の host では、同じ header を host の設定で付ける。
- Web クライアントが使う Community Node は、配信の origin を CORS の許可に入れる（`COMMUNITY_NODE_CORS_ALLOWED_ORIGINS`。本番の VM は Terraform の `cors_allowed_origins`）。入れないと、ブラウザが Community Node の API の応答を読めない。
- `.wasm` は `Content-Type: application/wasm` で配る（`_headers` の `/*.wasm` の規則）。違うと WASM の読み込みが遅い方法に切り替わる。COOP・COEP は付けない（ADR 0060 §2）。
- 第三者の script・analytics を足さない（CSP が拒み、外部送信表示にも無い）。cookie も使わない。host の機能のうち、script を自動で差し込むもの（Cloudflare の Web Analytics など）や cookie を足すもの（bot 対策など）は有効にしない。

## Cloudflare Pages に置く

Cloudflare の資格情報は、公開作業をする人が扱う。LP とは別の project にする。

```bash
npx wrangler pages project create <Web クライアントの project> --production-branch main
```

先に preview へ出して確かめる。

```bash
npx wrangler pages deploy apps/desktop/dist-web --project-name <Web クライアントの project> --branch preview
```

表示された preview の URL で、下の「置いた後に確かめる」を行う。本番の branch への公開と custom domain は、公開の判断の後に別に行う。

## 置いた後に確かめる

```bash
curl -sI https://<置いた先>/ | grep -i content-security-policy
```

```bash
curl -sI https://<置いた先>/assets/<WASM の file 名> | grep -i content-type
```

- CSP が `_headers` と同じで、WASM の `content-type` が `application/wasm` であること。
- ブラウザで開くと初回の同意の画面が出て、開発者 tool の console に CSP の違反が出ないこと。
- 初回の同意の後、Community Node の同意は「あとで」にして profile を設定し、設定の「アカウント」で鍵の export を試す（鍵の導出の Worker の確認）。console に CSP の違反が出ないこと。

## 外部送信とデータの説明

- Web の外部送信（配信元からの取得、STUN、relay の WebSocket）と保存（IndexedDB の鍵の保存など）は、`docs/legal/external-transmission-notice.md` と `docs/legal/app-data-flow-inventory.md` に書いてある。legal bundle の版は 8 のまま（#1220 AC-6、2026-10-04 ユーザー判断）。
- 配信の host を決めたり変えたりしたら、外部送信表示の「Web の配信元」の行が実際の host と合っているか確かめる。
