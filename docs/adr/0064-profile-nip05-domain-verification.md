# ADR 0064: プロフィールのドメインでの確認（NIP-05）

## Status

Accepted（Issue #1670、Scope revision r1、2026-10-09 ユーザー判断）。識別子の形・入力・互換（AC-1）を §1・§2 に、照会・保持・表示・開示（AC-2）を §3〜§6 に固定した。

## Context

アカウントの信頼性を示す手段として、Nostr の NIP-05（<https://github.com/nostr-protocol/nips/blob/master/05.md>）と同じ確認を入れる。NIP-05 はプロフィールに `name@domain` を書き、`https://<domain>/.well-known/nostr.json?name=<name>` が返す `names` の値が公開鍵と一致すれば、その識別子を示してよいとする仕組みである。kukuri の公開鍵は Nostr と同じ secp256k1 の x-only 公開鍵（hex 64 桁）なので、`nostr.json` の形をそのまま使える。NIP-05 自身が述べるとおり、これは本人確認ではなく、ドメインの持ち主がその鍵と名前の対応を示すことによる識別である。

照会は、表示した著者のドメインへの新しい外部送信になる。外部の取得の境界は、リンクプレビュー（ADR 0051）で固定したものを使う。

## Decision

### 1. 識別子と入力（AC-1）

- 署名つきプロフィール（`identity-profile` の content）に `nip05`（`name@domain`）を足す。署名者の申告であり、照会で確かめるまで表示しない。
- 入力は前後の空白を除き、英大文字を小文字にする。名前は `a-z0-9-_.` の 1〜64 文字。ドメインは `a-z0-9-` の label（1〜63 文字、先頭と末尾は `-` 以外）を点でつないだ 253 文字以下のホスト名で、label が 2 つ以上あり、最後の label に英字を含む（IP アドレスを除く）。英数字以外のドメインは `xn--` の形で入力する。形は core の `normalize_profile_nip05` と画面の `parseProfileNip05` が同じ規則で確かめる。
- 空にすると欄の無いプロフィールになる。形に合わない値は保存しない。編集画面は保存の前に理由を出して保存を止め、`set_my_profile` も署名・保存・docs への書込みの前に断る。
- 編集画面の欄の下に、ドメインに置く `nostr.json` の内容（`{"names":{"<名前>":"<自分の公開鍵>"}}`）を示し、コピーできるようにする。初回のプロフィール設定の画面には欄を出さない。
- CLI の `set_my_profile` は `nip05` を受け取り、profile の出力は `nip05` を含む。

### 2. 互換（AC-1）

- content の `nip05` は値が無ければ key を書かない。欄の無いプロフィールの content と envelope ID は欄を足す前と同じで、表示用の行から ID を求める `envelope_id_hint` も変わらない。
- 旧版のアプリは知らない key を無視して読む。`@domain` は出ない。旧版の端末でプロフィールを編集すると、旧版は自分の知る欄だけで content を作り直すので `nip05` は消える。
- 旧版の端末の SQL の行だけから profile を提供する経路（#1619）は、欄のある profile の ID を行から作り直せないので、その profile を提供しない。docs の replica の経路は変わらない。
- 保存は SQLite の `profiles.nip05`（migration `20261009120000_profile_nip05`）と、IndexedDB の profile の行（serde の JSON。欠けた値は空として読むので版を上げない）。書くだけの `profile_cache` と、`profile/latest` の `AuthorProfileDocV1`（読む側は `envelope_id` で envelope を読む）には持たない。

### 3. 照会（AC-2）

- 閲覧する端末が、表示する著者についてだけ直接照会する。Community Node は関与しない。
- 照会先は `https://<domain>/.well-known/nostr.json?name=<name>`（GET）。応答の JSON の `names` の `<name>` の値が著者の公開鍵（小文字の hex）と一致したときだけ確認できたとする。
- 契機は §5 の表示が画面内にあり、画面（document）が表示中のとき。
- デスクトップは Rust で取得し、ADR 0051 §2・§3 の境界（cookie・Authorization・Referer・system proxy を使わない、固定の User-Agent、非公開の名前とアドレスの拒否、検証したアドレスへの接続の固定、接続 3 秒・全体 6 秒）を使う。転送（3xx）には従わず、確認できなかったとする（NIP-05 の要件）。応答は 512 KiB まで。
- Web はブラウザの fetch で取得する（`credentials: 'omit'`、`redirect: 'error'`、`referrerPolicy: 'no-referrer'`、6 秒、応答は 512 KiB まで）。ドメインが CORS（`Access-Control-Allow-Origin: *`）を許可していなければ読めず、確認できなかったとする。ブラウザは要求に `Origin` を付ける。
- 同意の前（起動の状態が Ready の前）は照会しない。

### 4. 保持と上限（AC-2）

- 照会の結果は画面側の memory にだけ持つ（デスクトップと Web で共通）。key は（著者の公開鍵, 識別子）で、128 件まで、確認できたものは 10 分、それ以外は 60 秒。保持中の key は再送しない。
- 同時に 4 件、待ちは 32 件まで。超えた分は照会せず名前だけを出し、次に画面へ入ったときに照会する。
- 結果は表示だけに使う。フォロー・ブロック・通知・信頼評価・折りたたみなどの判断と、それらが指す公開鍵は変えない（NIP-05 の「公開鍵を主に参照する」）。

### 5. 表示（AC-2）

- 対象: 投稿カードの著者名（再投稿で元の投稿を主に出すときの元の著者を含む）、著者の詳細、自分のプロフィールの見出し。
- 形: 名前の後ろに `@<domain>`。ドメインは識別子の小文字の ASCII のまま（英数字以外のドメインは `xn--` の形で出る）。確認できたときだけ出し、確認できない・照会中・欄が無いときは名前だけを出す。
- 通知・DM・返信の引用・再投稿者の表示・返信先・Dome には出さない。

### 6. 開示（AC-2）

照会は新しい外部送信なので、legal bundle を 10 に上げて再同意を求める。外部送信表示に、送信先（表示した著者のプロフィールのドメイン）、契機（§3）、送信・観測され得る項目（IP address、HTTP／TLS の request metadata、path と照会した名前、デスクトップの固定の User-Agent、Web の `Origin`）、送らないもの（cookie、Authorization、Referer、公開鍵、account・topic・投稿の ID）、保持（§4）を書く。

## Consequences

- 著者のドメインの運営者は、その著者が表示された端末の IP address と時刻を知りうる（リンクプレビューと同じ）。悪意のあるプロフィールが追跡用のドメインを指すこともできるので、外部送信表示に書く。
- Web はドメインが CORS を許可しているときだけ確認できるので、デスクトップと Web で表示が異なりうる。
- 識別子はドメインの持ち主による対応の表明であり、本人確認ではない。

## Data classification

ADR 0002 の template に従う。

- Feature 名: プロフィールのドメインでの確認（NIP-05）
- Durable / Transient: 識別子は durable（署名つきプロフィールの欄）。照会の結果は transient（memory）。
- Canonical Source: 識別子は著者の署名つきプロフィールの envelope（author replica）。対応はドメインの `/.well-known/nostr.json`。
- Replicated?: 識別子はプロフィールと同じ範囲へ複製する。照会の結果は複製しない。
- Rebuildable From: 識別子は envelope から作り直せる。照会の結果は照会し直せばよい。
- Public Replica / Private Replica / Local Only: 識別子は public replica。照会の結果は local only（memory）。
- Gossip Hint 必要有無: 既存の `ProfileUpdated` だけ。
- Blob 必要有無: なし。
- SQLite projection 必要有無: `profiles.nip05`。Web は IndexedDB の profile の行。
- 必須 contract: §1 の形（core と画面の同じ規則）、§2 の互換（欄の無いプロフィールの ID が変わらない）、§3 の取得境界、§4 の上限。
- 必須 scenario: 編集・署名・他の端末の行（app-api の test）、照会の結果と表示（AC-2 の test）。
- 新しい外部送信: §6。

## References

- Issue #1670
- NIP-05: <https://github.com/nostr-protocol/nips/blob/master/05.md>
- ADR 0051（外部 URL の取得境界）、ADR 0060 §3（Web の capability）、ADR 0061（本人の端末間の profile の item）
