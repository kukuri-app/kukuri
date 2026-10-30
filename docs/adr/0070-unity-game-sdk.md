# ADR 0070: Unity 先行の kukuri Game SDK

## Status

Accepted（#1974 AC-1、2026-10-29）。§8 の対応環境・映像方式・性能プロファイルは #1974 AC-2 で確定した（2026-10-29）。各柱の実装と検証の完了は #1975〜#1984 の各 AC で判定する。

- [Discussion #1972](https://github.com/kukuri-app/kukuri/discussions/1972) は第一弾のエンジンを Godot 4.x としていた。2026-10-29 のユーザー判断「Unity 案でいく」により、第一弾を Unity に置き換える。Discussion は当時の検討として残し、以後の判断はこの ADR を正本にする。
- エンジンに依存しない Rust の共通部分と、エンジンごとの binding を分ける方針は Discussion から引き継ぐ。Godot・Unreal の binding は後続で扱う。

## Context

Discussion #1972 は、P2P のアセット配布、ロビーとゲーム通信、ゲームの内外をつなぐ SNS、ライブ配信と観戦の 4 本柱を一つの SDK で提供し、公式サンプル Cyber Disc Arena で連動を見せる構想である。組み込み方式は、本番を FFI の native plugin、開発中を Local Sidecar とし、エンジン側の API を共通にする。

基準 commit（main `1975a56c6`）には次がある。

- `kukuri-desktop-runtime` の `ClientHost` は UI adapter に依存せず、一つの active account の runtime と event の転送を所有する。Tauri の app と `kukuri-cli` の常駐プロセスが共用している。
- `kukuri-cli` は command 登録簿（`CommandRegistry`）、dispatcher、版つきの要求・応答（`RequestEnvelope` / `ResponseEnvelope`）、型付き error、secret frame、起動前から profile ごとの状態を持つ `ClientSession` を持つ（ADR 0049）。profile の file lock は desktop-runtime の `ProfileLease` が取る。登録簿と dispatcher は transport に依存しないが、常駐プロセスと socket の client は Linux の Unix socket だけである。
- 投稿・返信・リアクション・カスタムリアクション・公開 channel・private channel・Live・Game Room の command は登録簿にある。
- Game Room（ADR 0006）は ScoreGame と Metaverse room で、対戦の同期 engine ではない。
- Live の映像は webview の WebCodecs（VP8）で圧縮・復号し、Rust は署名・搬送・認可・視聴者の転送を行う（ADR 0067 §8・§9）。Unity では webview の処理を使えない。
- media（`/kukuri/media/1`）と Dome session（`/kukuri/dome-session/1`）は、最初の双方向 stream で受けた認可要求を受け口が検証してから通信を始める。

## Decision

### 1. 対象

- 第一弾は Unity の LTS 一系列、Windows x64、URP。版と依存は §8 で一つに固定する。
- 本番は Rust の cdylib を Unity の process に読み込む FFI 方式とし、別の daemon を不要にする。配布物が必ず一つの file になることは保証しない（依存する DLL を含めて複数になりうる）。
- 開発中は同じ C# API のまま Local Sidecar（別 process の常駐プロセス）へ切り替えられる。
- 既存の kukuri desktop・Web との SNS と Live の相互運用を第一弾に含める。
- 中央のサービスを新しい正本にしない。初回 seed・保持 peer・relay の運用条件と費用負担は #1984 の導入手順で示す。

### 2. 構成と再利用

既存の処理を写さず、同じ owner を使う。新しく書くものは次の表の右列に限る。

| 責務 | 置き場 | 再利用する既存の処理 | 新しく書くもの |
| --- | --- | --- | --- |
| runtime・account・同意・購読・profile の所有 | `kukuri-desktop-runtime` | `ClientHost`、`DesktopRuntime`、`ProfileLease`（profile の file lock と種類の marker）、account 作成・暗号化 export/import、app 同意・CN 同意 | profile の種類 `GameSdk`（§4） |
| command の定義・検証・実行 | `kukuri-cli` の `registry` / `dispatcher` / `protocol` / `session` | 登録簿・dispatcher・型付き error・secret frame | §3 の表の新しい command を同じ登録簿へ登録する |
| C ABI | 新規 `crates/game-sdk`（package `kukuri-game-sdk`、`cdylib` と `rlib`） | `kukuri-cli` の dispatcher を process 内で呼ぶ | handle・要求の受付・結果と event の待ち行列・batch の所有と解放・panic の隔離 |
| Local Sidecar | `crates/game-sdk` の binary `kukuri-sdk-sidecar` | FFI と同じ `SdkClient`（`kukuri-cli` の dispatcher と profile の所有）を Sidecar の process で動かす | Windows の named pipe の中継（§8.2） |
| ゲーム通信 | `kukuri-iroh-node` | media の受け口の認可（最初の stream で認可要求、5 秒ごとの再評価）、Direct → Relay Supported の接続順 | 専用 ALPN の session（datagram と stream） |
| アセット配布 | `kukuri-core` / `kukuri-app-api` / blob・docs・hint | docs の署名つき record、blob の取得・保持、公開 blob の発見（ADR 0063） | 発行者の署名つき manifest の型と検証 |
| Live の映像 | `kukuri-desktop-runtime` の live media | `push_live_video` / `next_live_video`、署名・転送・認可 | libvpx による native の VP8 圧縮・復号（§8.3） |
| Unity | 新規 `sdk/unity/com.kukuri.gamesdk/`（UPM）と `sdk/unity/CyberDiscArena/`（sample project） | — | C# API・module・Prefab・sample |

- SDK の dispatcher は CLI と同じ登録簿を使う。秘密の出力を伴う command（private channel の招待・access token・grant・share の export、passphrase で暗号化した鍵の export）は、CLI の secret frame と同じく応答の JSON とは別の秘密の出力として返す（§3）。平文の秘密鍵を返す command は登録簿に無く、SDK も足さない。
- 既存の command の意味・認可・同意の guard は変えない。SDK だけの第二の SNS protocol や Live protocol は作らない。
- 抽出で旧い処理が不要になる場合は、呼出元の切替と旧処理の削除を同じ子 Issue の PR に含める。

### 3. 共通の要求・結果・event

FFI と Sidecar は同じ要求・応答の形と同じ dispatcher を使う。遅延や copy の回数が同じであることは保証しない。

#### 要求と応答

- 要求は `RequestEnvelope`、応答は `ResponseEnvelope` をそのまま使う。項目と意味は `kukuri-cli` の `protocol.rs` と同じで、SDK は項目を足さない。
- `request_id` は呼出側が付ける。応答は同じ `request_id` で返る。未完了の要求と同じ `request_id` は `INVALID_ARGUMENT` で断る。完了した要求と同じ `request_id` で送り直した要求は、別の要求として実行する（ADR 0049 の改訂と同じ。自動再送はしない）。
- 秘密の入力（import の passphrase と export 文字列、招待 token）は `kukuri_sdk_send` の `secret` で渡す。秘密の出力は、poll の batch の中で応答とは別の項目として返す。どちらも応答の JSON・event・log に出さない。C# は秘密の出力を受け取った呼出元だけに返し、保存しない。

#### SDK が新しく足す command と event

既存の command は登録簿のものをそのまま使う。4 本柱のために足すのは次だけで、名前と担当はここで固定する。

| 種類 | 名前 | 担当 | 働き |
| --- | --- | --- | --- |
| command | `list_action_rooms` | #1978 | 1 つの application ID の action session の room を、更新の新しい順に 1 頁 20 件ずつ返す。続きは応答の `next_cursor` で読む（CLI と SDK だけ。desktop・Web は表示しない） |
| command | `join_game_session` / `leave_game_session` | #1978 | action session の room の host へ参加・退出する。参加の応答に session ID を返す |
| command | `publish_game_asset` | #1980 | 発行者の profile で manifest に署名して公開する（Sidecar・CLI だけ。§5.3） |
| command | `list_game_assets` / `fetch_game_asset` | #1980 | 発行者の公開鍵と application ID で、指定した asset ID（1 回 32 件まで）の最新の release を読み、検証済みの file の path を得る |
| command | `set_live_broadcast` / `stop_live_broadcast` / `watch_live_media` / `stop_watching_live_media` / `set_live_volume` | #1982 | desktop-runtime にある Live の配信・視聴の操作を登録簿へ載せる |
| event | `events_lost` | #1975 | 知らせの event を捨てた件数 |
| event | `game_session_closed` | #1978 | session が閉じた（退出・room の終了・権限の失効・host の不在）。理由の code を持つ。参加者と host の両方の端末に届く（host の端末は #1979 から） |
| event | `game_session_opened` | #1979 | host の端末で session を受け付け、送受信を始めた。session ID・room ID・参加者の公開鍵を持つ |
| event | `game_session_packet` | #1979 | datagram または stream の message を受けた。どちらで届いたかを持ち、bytes を伴う |
| event | `game_session_path` | #1979 | 接続の経路（Direct P2P / Relay Supported P2P / Relay Fallback）か RTT が変わった。その時点の送受信の bytes も持つ |
| event | `game_asset_updated` | #1980 | 検証できる新しい release が届いた。この端末で適用中のアセットは取得・適用した後に file の path 付きで、まだ適用していないアセットは path 無しで届く |
| event | `post_received` | #1981 | 購読中の topic・channel の投稿・返信を手元へ新しく保存した（自分の投稿を含む）。投稿 ID・topic・channel・作者・返信先・thread の root を持ち、本文は持たない。作成から 30 秒を過ぎたもの（過去の頁の読込みで届いた古い投稿）と非表示の作者（mute・block）のものは送らない。溢れたときは `post` が null で、受け手は表示中の timeline・thread を読み直す |
| event | `live_video_frame` | #1982 | 視聴中の session の復号済みの映像（画素の形式は §8）。bytes を伴う |
| event | `live_watch_status` | #1982 | 視聴の状態（参加中・満員で転送待ち・映像なし・停止）が変わった |

リアクションの受信は既存の `ReactionReceived`（対象の投稿を持つ）をそのまま使う。

#### C ABI

| 関数 | 働き | 待ち |
| --- | --- | --- |
| `kukuri_sdk_abi_version()` | ABI の版を返す。C# は読み込み直後に照合し、違えば理由を示して開始しない | なし |
| `kukuri_sdk_open(abi_version, config, out_handle)` | ABI の版と profile（名前と directory）を指定して handle を作る。版が違えば `ABI_MISMATCH` で handle を作らない。runtime の起動は背景で進み、完了を `startup_status_changed` の event で知らせる。起動に失敗したら（`profile_in_use` 等）、要求の応答がその error を返す。Sidecar の mode では runtime は既に動いており、接続の前の event は渡さないので（§8.2）、`get_desktop_startup_status` で状態を読む | なし |
| `kukuri_sdk_send(handle, request, secret)` | 要求を待ち行列に入れて即座に戻る。結果は poll で受ける | なし |
| `kukuri_sdk_send_binary(handle, kind, target, bytes)` | session の packet（datagram・stream）と、配信する映像・音声の bytes を渡す。`target` は session ID または Live の session ID。kind は 1 が datagram、2 が stream（映像・音声の kind は #1982 が足す）。#1979 で足し、ABI の版を 2 にした | なし |
| `kukuri_sdk_cancel(handle, request_id)` | 実行前の要求を取り消す | なし |
| `kukuri_sdk_poll(handle, max_items, max_bytes, out_batch)` | 届いている応答・秘密の出力・event・bytes を上限まで batch に入れて返す。1 件だけで `max_bytes` を超えるものは、その poll の最初の 1 件なら単独で返す。届いていなければ batch を返さない（`out_batch` は null） | なし |
| `kukuri_sdk_batch_release(handle, batch)` | poll で受けた batch（中の JSON と bytes を含む）を解放する | なし |
| `kukuri_sdk_close(handle)` | 新しい要求を断り、未完了の要求を下の表の結果で終え、購読・task・DB lock を解放する。close の後も poll と batch の解放はでき、未完了だった要求の結果を受け取れる | 停止の完了まで（上限 5 秒） |
| `kukuri_sdk_free(handle)` | handle と未解放の batch の memory を解放する（close していなければ close する）。以後 handle は使えない | close と同じ |

- ABI の関数は同期的な状態 code を返す: `OK`、`INVALID_ARGUMENT`、`BACKPRESSURE`、`CLOSED`、`FAULTED`、`ABI_MISMATCH`。要求の成否は poll で受ける応答で返す。
- `kukuri_sdk_send_binary` には応答が無く、結果はこの状態 code だけで返す。上限を超える大きさ・不明な kind は `INVALID_ARGUMENT`、閉じた session・Live は `CLOSED`、stream の送り待ちが上限に達したら `BACKPRESSURE`（bytes は受け付けない）。datagram と映像は送り待ちが溢れても `OK` を返し、§3 の上限の規則で古いものを捨てる。
- 受けた packet は、`game_session_packet` の event（item の種類 3）の直後に bytes の項目（種類 4）を置いて返す。2 つは同じ batch に入れ、分けない。
- batch の中の JSON と bytes は、`kukuri_sdk_batch_release` を呼ぶまで有効で、呼んだ後は使えない。解放した batch・別の handle の batch を渡すと `INVALID_ARGUMENT` を返し、何も解放しない。
- handle は opaque な pointer とし、Rust の内部型を ABI の外へ出さない。
- `kukuri_sdk_close` 以外は network の待ちをしない。Unity の main thread から呼んでも止まらない。
- すべての関数で panic を捕まえる。panic した handle は `FAULTED` になり、以後の関数は `FAULTED` を返し、未完了の要求は `internal_error` で終わる。Unity の process は落とさない。

#### 上限

| 対象 | 上限 | 超えたとき |
| --- | --- | --- |
| 未完了の要求 | handle ごとに 256 件 | `kukuri_sdk_send` が `BACKPRESSURE` を返し、要求は受け付けない |
| 要求 1 件の JSON | 1 MiB（CLI の frame と同じ） | `INVALID_ARGUMENT` |
| 応答と秘密の出力の置き場 | 未完了の要求と同数を予約する | 溢れない。捨てない |
| 知らせの event（bytes を伴わないもの） | 1024 件 | 古い event から捨て、捨てた件数を `events_lost` で知らせる。受け手は状態を読み直す |
| 受けた datagram | session ごとに 64 件 | 古いものから捨てる |
| 受けた stream の message | session ごとに 256 KiB | 捨てない。上限に達したら stream の読み取りを止め、QUIC の flow control で送り手を待たせる。poll で空いたら読み取りを再開する |
| 受けた映像 | 視聴中の session ごとに 1 枚 | 新しい frame が来たら古い frame を捨てる |
| poll 済みで未解放の batch | handle ごとに合計 64 MiB | 解放されるまで、新しい映像と datagram は捨て、stream は読み取りを止めたままにする。応答は予約した置き場に残る |
| 1 回の poll | 呼出側の指定（最大 256 件・`max_bytes`）。Unity の既定は §8 | 残りは次の poll へ。1 件だけで `max_bytes` を超えるものは、次の poll の最初に単独で返す（大きな映像の frame も受け取れる） |
| 要求の期限 | 既定 30 秒、指定できる上限 5 分（CLI と同じ） | `timeout` |

- media・blob の本体を JSON に入れない。アセットは検証済みの file の path で渡し（§5.3）、映像と packet は bytes で渡す。
- 送る側の上限は §5.2（packet）と §8（映像）。

#### 取消・終了・切断の結果

| 状況 | 読み取り（Read） | 書き込み（Write・Destructive） |
| --- | --- | --- |
| 実行前に取り消した | `cancelled` | `cancelled` |
| 実行中に取り消した | 取り消せず、実際の結果を返す | 取り消せず、実際の結果を返す |
| 実行中に close した | `interrupted` | 完了を待つ（上限 5 秒）。待てなければ `operation_outcome_unknown` |
| close の後の要求 | `kukuri_sdk_send` が `CLOSED` | 同左 |
| Sidecar との接続が切れた | `interrupted` | `operation_outcome_unknown` |

- 書き込みの結果を知らせずに捨てない。成否が分からないものは `operation_outcome_unknown` として区別する。
- error の code は `kukuri-cli` の `error_code` に `cancelled` を加えたものを使う。

#### event

- handle が Ready になると、`events.watch` と同じ runtime の event（`RuntimeEvent`）を受ける。各柱が足す event（上の表）は同じ流れに載せる。
- event は状態が変わったことの知らせであり、取りこぼしても表示と操作が成り立つようにする。受け手は必要なら command で今の状態を読み直す。

### 4. profile・アカウント・同意

- SDK の profile は kukuri desktop・Web とは別の directory に置く。Unity の既定は `Application.persistentDataPath/kukuri/profiles/<profile 名>`。kukuri app の install と、app の DB の共有を前提にしない。
- 一つの profile は一つの runtime だけが所有する。FFI と Sidecar のどちらも、profile の state を開く前に `ProfileLease::acquire` で file lock を取る。同じ profile を同時に開こうとすると、後の側は `profile_in_use` で断られる。
- SDK の profile には新しい種類 `GameSdk` の marker を付ける。FFI と Sidecar は同じ種類で開くので、同じ profile を順に切り替えて使える。kukuri の GUI・CLI の profile を SDK から開くと `profile_kind_mismatch` で断る。私用の kukuri の profile を誤って開かないためである。
- 初回の流れは既存の入口をそのまま使う。
  1. `get_desktop_startup_status` が同意待ちを返す。同意前の network I/O は 0 件（`ClientHost::start_if_consented_with`）。
  2. ゲームが利用規約・プライバシーポリシー（`get_app_consent_status` の文書）と 18 歳以上の自己申告を表示し、`accept_app_consents` で同意する。
  3. 同意すると account が作られ、ゲームだけで使い始められる。
  4. 既存の kukuri のアカウントを使う人は、kukuri app で暗号化 export した文字列と passphrase を明示的に入力し、`preview_account_key_import` と `import_account_key` で取り込み、`switch_account` でそのアカウントを active にする（import だけでは active は変わらない）。これで SNS と同じ公開鍵になる。誤った passphrase では既存の profile を変えない。
  5. Community Node を使う機能は、既存の CN 同意（`fetch_community_node_policies` / `accept_community_node_consents`）を経てから接続する。
- 取り込み画面は、取り込んだアカウントでこのゲームが投稿・署名できることを示す。ゲームのコードは profile と同じ OS の権限で動くためである。
- 秘密の値を Inspector の serializable field、scene・prefab、PlayerPrefs、診断の出力に保存しない。

### 5. 4 本柱のデータ分類

SNS（§5.4）と Live（§5.5）は既存の分類を変えない。新しく分類するのは §5.1〜§5.3 の 3 つである。

#### 5.1 SDK の profile と要求の経路

- Feature 名: Game SDK の profile、要求・応答・event、bytes の受け渡し
- Durable / Transient: profile 内の account・製品データは Durable。handle・待ち行列・bytes・Sidecar の接続は Transient
- Canonical Source: profile directory の account registry と既存の各 feature の store。要求の形は `kukuri-cli` の登録簿
- Replicated?: 製品データだけが各 feature の既存の規則で複製される。handle と待ち行列は No
- Rebuildable From: 登録簿と runtime の event から作り直す
- Public Replica / Private Replica / Local Only: Local Only（製品データは各 feature の分類に従う）
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 既存の profile DB を使う。新しい表は作らない
- 必須 contract: FFI と Sidecar で同じ fixture（§10）の結果が同じ。上限（§3）で待ち行列が有界。secret が応答・event・log に出ない。同意前の network I/O 0 件
- 必須 scenario: 空の Unity project で FFI と Sidecar の最小 sample が同じ結果になる（#1977）

#### 5.2 ゲームの通信（ロビーと session）

- Feature 名: ゲームのロビー（room の metadata）と、room の session の packet
- Durable / Transient: room の metadata は Durable。session と packet は Transient
- Canonical Source: room の metadata は既存の Game Room（ADR 0006）と同じく docs の `game/<room_id>/state` と manifest blob。session は host の端末の受け口の状態
- Replicated?: metadata は room を置いた topic・channel の replica で複製する。packet は複製しない
- Rebuildable From: metadata は docs と blob から。session は作り直さない（切れたら参加し直す）
- Public Replica / Private Replica / Local Only: room を置いた公開 topic または private channel の公開範囲に従う。packet は Local Only の送受信
- Gossip Hint 必要有無: 既存の Game Room と同じ扱い（ADR 0006、ADR 0052 の個別反映）。hint を受けたことを参加の許可にしない
- Blob 必要有無: metadata の manifest に必要
- SQLite projection 必要有無: 既存の room 一覧の projection を使う
- 必須 contract: public・friend_only・friend_plus・invite_only の参加条件を host が確かめる。退出・終了・権限の失効の後は packet が届かない。一覧の取得量が room と履歴の総数に依存しない
- 必須 scenario: 2 つの Unity profile で作成 → 発見 → 参加 → 通信 → 退出（#1978・#1979）

この柱で決めたことは次のとおり。

- room は既存の Game Room に新しい種類（action session）を足して表す。application ID、host の endpoint、最大人数、状態を owner の署名つき manifest に持たせ、ScoreGame と Metaverse room の挙動は変えない。参加者は manifest に書かず、host の session が受け付ける。
  - 作成は既存の `create_game_room` に `action`（application ID と最大人数）を付けて行う。host の endpoint は作成した端末のものを入れる。状態の変更と終了は既存の `update_game_room`（scores は空）で行い、終了した room は更新できない。
  - application ID は 1〜128 byte の、空白と制御文字を含まない ASCII。最大人数は 1 以上。action の metadata は action session の room だけが持ち、他の種類の room が持っていれば検証で断る。
  - 一覧（`list_action_rooms`）は application ID ごとに、更新の新しい順に 20 件ずつ読む。索引は (topic, channel, application ID, 更新時刻, room ID) で、ScoreGame と Dome の一覧は application ID が空の範囲だけを読む。どちらの一覧も、他方の room や他のゲームの room の数に比例しない。終了した room も「終了」として一覧に残る（2026-10-30 のユーザー判断）。
  - desktop・Web の一覧には action session の room を出さない。Web はこの種類の行を保存しない（2026-10-30 のユーザー判断）。旧版の desktop は manifest の種類を読めずにその room を飛ばし、一覧の残りは表示される。
- 公開範囲は room を置く場所で決まる。公開 topic なら public、private channel なら既存の audience（friend_only・friend_plus・invite_only）をそのまま使う。
- session は専用の ALPN で host の端末へ接続し、最初の双方向 stream で、参加者が署名した 10 秒の proof（room と host に束縛）を送る。host は room の状態と公開範囲を確かめてから packet を受け付け、5 秒ごとに評価し直す（media と同じ形）。
  - ALPN は `/kukuri/game-session/1`。要求は topic、room ID、proof（`DomeSpatialAccessProofV1`）で、上限は 64 KiB。proof の署名は topic・channel と host に束縛する。room ID は要求に入れ、host が自分の room と照らす（要求を受け取るのは host だけ）。
  - host は次をすべて満たすときだけ受け付け、session ID を返す。拒否は理由の code だけを返す。
    - room が自分の action session で、終了していない（`room_not_found`・`room_ended`）
    - 公開範囲を満たす（`access_denied`）。公開 topic では gossip を止めていないこと、private channel では参加者が今の epoch の参加者であること。どちらも host からの block が無いこと。friend_only・friend_plus・invite_only の違いは channel に入るときの規則にある
    - host を含めて最大人数に達していない（`room_full`）
  - 接続できないときの code は `host_unreachable`。参加者の端末が同時に参加する session は 8 まで（`session_limit`）。
  - 接続は session の間ずっと保つ。host は評価し直しで続けられないと分かったら、理由の code（`room_ended`・`access_revoked`）を付けて閉じる。参加者が閉じれば退出（`left`）、理由が届かないまま切れたら `host_unavailable`。
  - host の端末で `leave_game_session` を呼ぶと、その参加者の session を `removed` で閉じる（参加者を外す。#1979 AC-2 の NGO の `DisconnectRemoteClient`）。
  - 参加者の端末には、閉じた理由とともに `game_session_closed` が届く。両方の端末で session の記録を消し、閉じた session の送受信は認可しない。
  - host の端末には、受け付けて送受信を始めたときに `game_session_opened` が、閉じたときに `game_session_closed` が届く。host の端末での理由は、評価し直しで閉じたならその code、参加者が閉じたか接続が失われたなら `left`。
- 座標などの最新の状態は QUIC の unreliable datagram、確定させる操作は reliable stream で送る。送る側の上限は次のとおりで、#1979 はこの値で実装する。
  - datagram: payload は 1 件 1100 bytes 以下で、さらに接続の `max_datagram_size` 以下。送り待ちは session ごとに 64 件で、溢れたら古いものから捨てる。
  - stream: message は 1 件 64 KiB 以下で、session の中の順に全部届く。送り待ちは session ごとに 256 KiB で、超えたら `BACKPRESSURE`。送り待ちには QUIC へ書いている途中の message も数える。受け手が読み取りを止めると QUIC の flow control で書けなくなり、送り手に `BACKPRESSURE` が返る。
  - stream は向きごとに 1 本の単方向 stream で、message ごとに長さ（u32）を前に置く。64 KiB を超える長さを受けたら接続を閉じる。
  - 経路は 1 秒ごとに確かめ、経路の分類が変わったとき、または RTT が 5 ms 以上かつ 2 割以上変わったときに `game_session_path` で知らせる（最初は開いたとき）。分類は、選ばれた経路が relay なら Relay Fallback、IP の経路で relay の経路も開いていれば Relay Supported P2P、IP の経路だけなら Direct P2P。送受信の bytes は接続の UDP の量。
  - 受ける側の上限は §3。
- 接続は Direct P2P → Relay Supported P2P の順で試し、実データが relay を通るときは Relay Fallback として区別して記録する。central な lobby の authority は置かない。

#### 5.3 署名つきアセット配布

- Feature 名: ゲームの公式アセット（スキン等）の発行と P2P 配布
- Durable / Transient: Durable（release 単位）
- Canonical Source: 発行者の鍵で署名した manifest（application ID、asset ID、release revision、blob hash、長さ、形式）を docs の record に置き、本体は blob
- Replicated?: Yes。発行者の制御領域（`author::<発行者>`）の record と、それを読んで適用した peer が保持した record・blob
- Rebuildable From: docs の署名つき manifest と blob
- Public Replica / Private Replica / Local Only: 発行者の制御領域（公開）。適用済みの release の記録と検証済みの file は Local Only
- Gossip Hint 必要有無: 更新の知らせに使う（ゲームの公開 topic の `SessionChanged`、object_kind `game-asset`）。hint だけでは適用しない
- Blob 必要有無: Yes（glTF binary・PNG・JPEG・Unity AssetBundle）
- SQLite projection 必要有無: 適用済みの release の記録を持つ（application ID と asset ID ごとに 1 行）
- 必須 contract: 発行者の公開鍵で検証できる manifest と、hash が一致する blob だけを適用する。別の鍵・改変・古い revision を拒否する。提供者がいないときは取得できないことを結果で返す
- 必須 scenario: 発行者・保持 peer・新しく取得する peer の 3 役で公開 → 再提供 → 取得 → 再起動なしの差し替え（#1980）

この柱で決めたことは次のとおり。

- 信頼する発行者の公開鍵は Unity の設定 asset に固定する。秘密鍵はゲームと UPM package に含めず、発行は開発者の Sidecar（CLI）からだけ行う。
- 検証した本体は profile の cache directory に file として書き、その path を Unity に渡す。bytes を待ち行列に積まない。
- 署名は発行元と内容が一致することを確かめるもので、アセットが無害であることの保証ではない。
- 2026-10-30 のユーザー判断（#1980）:
  - 形式は glTF binary（.glb）・PNG・JPEG と Unity の AssetBundle。AssetBundle は作った Unity の版と build の対象を manifest に記録し、読み込む側は自分と合わない bundle を読まない。
  - 1 件の上限は既存の blob の上限（3 GiB）。取得の期限は大きさに合わせて延ばす（1 端末の転送を、大きさを 1 MiB/s で送る時間。15 秒より短くしない）。
  - アセットの数に上限を置かない。asset ID は Unity Editor からの発行のときにゲームの設定 asset へ登録し、Player は登録された ID の record を読む。
  - この端末で適用中の版は cache の掃除で消さず、ほかの端末への提供元にする。新しい版を適用したら、古い版は通常の cache の規則に戻る。
  - 新しい版の取得は自動。更新の知らせを受けたら、適用中のアセットは背景で取得・検証・適用し、`game_asset_updated` で path とともに知らせる。
- record の key は発行者の制御領域の `game-assets/<application ID>/<asset ID>` で、値は署名した envelope（kind `game-asset-release`）。日の bucket に置かないので、新しい端末も 1 回の exact な読みで最新版に届く（件数に比例しない）。
  - 読み手は手元 → 発行者本人 → ゲームの公開 topic の参加者の順に読み、受けた record（最大 8 件）をすべて検証して revision が最大のものを採る。別の名義で古い署名済みの record を書かれても、新しい版より優先しない。
  - 適用は revision が大きいときだけ（`applied_game_assets` の行）。取得は `.partial` へ書き、長さを確かめてから使える名前にする（hash は転送で照合する）。
- ゲームの blob は公開 blob の DHT 発見（ADR 0063）の対象外のまま。取得は既知の端末とゲームの公開 topic の参加者から行う。

この節の旧案（「ゲームの公開 topic の replica」に manifest を置く）は、日の bucket を知らない端末が最新版に届かず件数に比例する走査が要るので失効した（#1980 AC-1）。

#### 5.4 SNS

既存の投稿・返信・リアクション・カスタムリアクション・公開 channel・private channel の分類と command をそのまま使う。SDK は投稿の形式を足さない。表示は既存の cursor・有限の取得で行い、全履歴の同期を前提にしない（#1981）。

- 投稿は `create_post`（スクリーンショットは `attachments` に file の path を渡す）、表示は `list_timeline`・`list_thread`、リアクションは `toggle_reaction` を使う。desktop・Web と同じ runtime の command なので、ID・公開範囲・block の規則は同じになる（#1981 AC-1）。
- 新着は `post_received` と `reaction_received` で知る。runtime の event は desktop・Web の画面にも届くが、画面は使わない（定期の読み直しで表示する）。

#### 5.5 Live

既存の Live（ADR 0005）と media の搬送（ADR 0067）の分類をそのまま使う。追加は、Unity の画面を既存と同じ VP8 の chunk にする処理、Unity の出力音をマイクの音と混ぜて既存の Opus の音声 track にする処理（音声の圧縮は既存の Rust 側の処理。ADR 0067 §9）、受けた chunk を Unity の texture にする処理だけで、新しいデータの種類は無い。映像の bytes は Transient で保存しない（#1982）。

### 6. Cyber Disc Arena の裁定

- room の host（作成者）の端末が、ディスクの所有・キャッチ・被弾・得点を一つに確定する。host の移譲はしない。host が抜けたら room は終わる。
- 参加者は入力を datagram で送り、キャッチと投げる操作の要求を stream で送る。host は状態を datagram で配り、確定した event を通し番号つきの stream で配る。
- 参加者は自分の動きを予測して表示し、host の状態に合わせて補正する。ディスクの所有は確定 event でだけ変わる。発射ベクトルの共有だけで端末間の物理が一致するとは仮定しない。
- 予測・補間・補正の実装と裁定の規則は #1983 が持つ。packet の配送は #1979 が持つ。

### 7. Unity 側の構成

- module ごとに asmdef を分ける: Core（必須）、Lobby、Realtime、Netcode（Unity Netcode for GameObjects に依存）、Assets（glTFast に依存）、Social、Live。使わない module の依存を持ち込まない。
- Netcode の module（`Runtime/Netcode`、#1979 AC-2）は、NGO 2.13.3 以上 3.0 未満が入った project だけで compile する（asmdef の version define）。NGO の Transport `KukuriNetcodeTransport` は、host が `RoomId` の room の session を client として受け付け、client が `SessionId` の host へつなぐ。
  - 配送: `Unreliable` と `UnreliableSequenced` は datagram、`Reliable`・`ReliableSequenced`・`ReliableFragmentedSequenced` は stream に載せる。datagram の先頭 1 byte で 2 つを区別し、`UnreliableSequenced` は 4 bytes の通し番号で古いもの・重複を捨てる。これ以外の値の配送は送らずに error を出す。
  - NGO の分割しない 1 batch の上限を 1088 bytes（datagram の上限から先頭の 5 bytes を引く）、分割する message の上限を 64 KiB（stream の 1 message）に合わせる。超えるものは NGO が送らない。
  - stream の送り待ちが上限で受け付けられなかった分は、相手ごとに 1 MiB まで手元に持って順に送り直す。超えたらその相手を切る。
  - host が開いた知らせより先に受けた packet は、session ごとに 64 件まで持ち、開いたら渡す。
  - 切断（host が外す・client が退出する・NGO を止める）は session をすぐ閉じる。その時点の送り待ちの分（NGO の切断の理由の message を含む）は届かないことがある。
- Assets の module（`Runtime/Assets`、#1980 AC-2）は、glTFast 6.20.0 以上 7.0 未満が入った project だけで compile する。`KukuriSkinLoader` は、信頼する発行者の公開鍵・application ID・asset ID の最新の検証済みの file を `fetch_game_asset` で得て表示する。`game_asset_updated` を受けたら読み直し、再起動せずに差し替える。
  - .glb は glTFast で読み、`TimeBudgetPerFrameDeferAgent` で 1 frame の処理を予算の中に抑える。PNG・JPEG は renderer の material の texture にする。AssetBundle は自分の Unity の版と build の対象が manifest と合うときだけ読み、最初の GameObject を置く。
  - 新しい版を読み終えてから入れ替え、前の版の GameObject・glTF の資源・texture・AssetBundle を解放する。読めなければ理由を `Failed` で知らせ、今のスキンを保つ。
- C# の結果（Task）と event の callback は Unity の main thread で、poll の中から呼ぶ。Rust の worker thread から GameObject や UI を触らない。
- 1 frame で poll に使う件数と時間には上限を置く（既定値は §8）。上限を超えた分は次の frame へ回す。
- mode（FFI / Sidecar）と profile 名は設定 asset で明示する。本番の Player の既定は FFI で、外部の daemon を自動で起動しない。
- 未使用の SNS の UI や Live の取り込みは、明示的に開始するまで動かない。

### 8. 対応環境・映像方式・性能プロファイル

#1974 AC-2 で確定した（2026-10-29）。2026-10-29 のユーザー判断は、Unity の版（6.3 LTS）、Live の音声（ゲーム音とマイクを一つの track にする）、性能の測定条件と閾値（§8.4）の 3 点である。ここに無い環境・版を対応済みとは扱わない。

#### 8.1 対応環境

| 対象 | 固定するもの |
| --- | --- |
| Unity | 6000.3.27f1（Unity 6.3 LTS）。6000.3 系の新しい patch へ上げるときは #1984 の CI で確かめてから上げる |
| package | URP 17.3.0、Netcode for GameObjects 2.13.3（6.3 に同梱の版。Netcode module だけが依存する）、glTFast 6.20.0（Assets module だけが依存する） |
| OS・graphics | Windows 11 x64、Direct3D 12（6.3 の Windows の既定）。Windows 10 と他の graphics API は確かめていないので対応に含めない |
| scripting | Editor は Mono。Player は IL2CPP を基準とし、Mono の Player も build・実行できる |
| C# | P/Invoke の pointer を扱う SDK の asmdef だけで unsafe を許可する。利用者の project 設定は変えない |
| native | Rust 1.98.1（repository の toolchain）、`x86_64-pc-windows-msvc` の cdylib。依存は OS の DLL だけにするため、CRT は静的に link する（Rust は `+crt-static`、libvpx は `--enable-static-msvcrt`） |

#### 8.2 Sidecar のローカル IPC

- Windows の named pipe を使う。名前は profile directory から決め（`\\.\pipe\kukuri-sdk-<profile directory の hash>`）、同じ profile の Sidecar が二つ動かないようにする。
- pipe は `FILE_FLAG_FIRST_PIPE_INSTANCE` で作り（同じ名前の pipe を先に作られていたら起動しない）、remote の client を拒否し、DACL は Sidecar を起動した user だけに読み書きを許す。TCP の listen はしない。
- Sidecar の process は FFI と同じ `SdkClient` を持ち、pipe は send・cancel・poll を中継するだけにする。待ち行列・取消・期限・event の上限の意味は FFI と同じコードが担い、同じ fixture（§10）を両方へ流して確かめる（#1976 AC-1）。`kukuri-cli` の bin は `kukuri-game-sdk` に依存できない（`kukuri-game-sdk` が `kukuri-cli` の lib に依存する）ので、Sidecar は `kukuri-game-sdk` の binary `kukuri-sdk-sidecar` に置く。Linux の CLI の常駐プロセスと protocol は変えない。
- frame は長さ（u32）つきの bytes。最初の frame（hello）で ABI の版と profile の名前・directory を照合し、合わなければ handler に届く前に理由の code（`protocol_mismatch`・`authorization_failed`）を返して切る。
- 接続は一度に一つ。Sidecar は接続を受けるとすぐ次の pipe を用意し、接続中に来た別の client には hello への返事で `profile_in_use` を返す（その client の要求は `profile_in_use` で終わる）。
- client の close は接続を切るだけで、Sidecar の runtime（identity・P2P の接続）は止めない。切断で応答を待っていた要求の結果は §3 の表のとおりで（読み取りは `interrupted`、書き込みは `operation_outcome_unknown`）、Sidecar は切断した接続の要求の応答を捨て、接続し直した client にも渡さない。接続し直した client には、接続していない間の event も渡さない（状態を読み直す）。
- Sidecar は `kukuri-sdk-sidecar run --profile <名前> --profile-dir <絶対 path>` で起動し、`stop`（同じ引数）か Ctrl+C で止める。`stop` は hello に止める印を付けて送り、client が接続中でも止まる。同じ profile の二つ目の Sidecar は pipe を作れずに終わる（終了 code 3）。Unity の Player は Sidecar を起動しない（§7）。
- 接続から hello までの期限は 5 秒。hello を送らない接続はその後に切り、他の client を締め出し続けない。開いてすぐ閉じた接続や接続の待ちの失敗では、その pipe を捨てて次の接続を待ち、Sidecar は止めない。直前に閉じた接続を片付けている短い間に来た client は `profile_in_use` で断られうるので、C# は少し待って接続し直す。
- 返事を書いた接続を `DisconnectNamedPipe` で切ると、client がまだ読んでいない返事が捨てられる。Sidecar は接続を終えるとき handle を閉じるだけにし、断りの返事も client に届ける。

#### 8.3 映像と音声

- codec は既存の Live と同じ VP8（ADR 0067 D6）。Rust 側で libvpx 1.17.0（BSD-3-Clause）を静的に link して圧縮・復号する。純 Rust の VP8 実装は実用段階のものが無いので使わない。
  - build: `configure --target=x86_64-win64-vs17 --disable-vp9 --enable-realtime-only --enable-static-msvcrt --as=nasm` の project を MSBuild で `WholeProgramOptimization=false` で build する（全体最適化の object は Rust の link と合わない）。1.17.0 の VS の project は `vp8/common/x86`・`vp8/encoder/x86` の asm と `vpx_ports` の `emms_mmx.asm`・`float_control_word.asm` を含めないので、これらは nasm で別に組み立てて同じ DLL に link する。source は `chromium.googlesource.com/webm/libvpx` の tag `v1.17.0` から取得し、手順と checksum を #1984 の CI に置く。
  - 圧縮の設定: realtime、CBR、`g_lag_in_frames = 0`、`VP8E_SET_CPUUSED = 10`、4 thread。
- 画質は既存の受信側の上限に合わせ、camera の上限（1280×720・30 fps・1.5 Mbps。ADR 0067 D8）で送る。既存の受信側はこれを超える frame と bitrate を捨てるためである。
- 取り込み: 配信する camera の RenderTexture を `AsyncGPUReadback`（RGBA32）で読み、callback で bytes を `kukuri_sdk_send_binary` へ渡す（SDK が copy する）。I420 への変換と圧縮は Rust の worker thread で行い、Unity の main thread で圧縮しない。圧縮前の送り待ちは 1 枚で、新しい frame が来たら古い frame を捨てる。
- 表示: 復号した I420 の 3 面を `live_video_frame` で渡し、Unity は R8 の texture 3 枚へ `SetPixelData` で上げ、shader で RGB にする。
- 音声: `AudioListener` と同じ GameObject の component の `OnAudioFilterRead` でゲームの出力音（`AudioSettings.outputSampleRate` の rate と channel 数の float）を受け、audio thread から `kukuri_sdk_send_binary` で渡す。Rust 側は 48 kHz に変換し、既存のマイクの取り込み（moq-audio）の音と混ぜて、既存の Opus の track として送る。送り待ちは 200 ms 分までで、溢れたら古いものから捨てる。
  - 今の moq-audio はマイクの取り込みから Opus の圧縮までを一続きに行うので、取り込みの後・圧縮の前に混ぜる口は #1982 が作る。
  - マイクを off にしても（機器は閉じる。ADR 0067 §9）、ゲーム音は送り続ける。

#### 8.4 性能の測定条件と閾値

| 項目 | 固定する条件 |
| --- | --- |
| 機器 | Ryzen 7 7800X3D / RTX 4080 SUPER / 64 GB / Windows 11 の 1 台。対戦者 2 人と視聴者 5 人の 7 つの Player（FFI）に加え、発行用の Sidecar と計測用の proxy を同じ PC で動かす |
| 解像度 | 対戦者 2 人は 1920×1080、視聴者 5 人は 1280×720（window） |
| 負荷 | 対戦 2 人。host の対戦者が試合の camera を配信し、視聴者 5 人が視聴する（直接の枠は 4 人なので、5 人目は転送で受ける）。試合中に Sidecar から 5 MiB の glb のスキンを 1 件発行し、対戦者 2 人が適用する |
| network | 全 process の P2P の packet を、計測用の UDP の proxy（#1984 が harness に置く）に通す。片道は packet ごとに 22.5〜27.5 ms の一様な遅延（往復の平均 50 ms、片道のゆらぎの幅 5 ms）で、方向ごとに 1% を落とす（2026-10-29 のユーザー判断）。relay と DHT は使わないので、この条件で測れる経路は Direct P2P だけになる |
| 時間と回数 | 準備の 10 秒の後に 3 分を測り、これを 5 回くり返す。memory の増加だけは、同じ負荷を 10 分続ける測定を別に 1 回行って判定する（2026-10-29 のユーザー判断）。家庭回線 2 拠点の手順と結果は #1984 の日本語の手順に記録し、参考値として PASS / FAIL に数えない（Relay Supported P2P と Relay Fallback の値はここでだけ出る） |
| 時計 | 同じ PC の `Stopwatch`（QueryPerformanceCounter）で、process をまたいで時刻を比べる |

閾値は 5 回すべてで満たすこと。frame time・入力の確定・asset の反映は対戦者 2 人とも、映像遅延は視聴者 5 人全員、memory は 7 つの Player すべてで判定する。送信量は 3 分間の平均で判定する。

| 項目 | 測り方 | 閾値 |
| --- | --- | --- |
| frame time | 対戦者の `Update` の間隔 | p95 ≤ 16.7 ms、p99 ≤ 25 ms |
| 入力の確定 | キャッチの操作の frame から、host の確定 event を受けた frame まで | p95 ≤ 100 ms |
| 映像遅延 | 配信者が frame に埋めた番号を視聴者の texture で読み、取り込みの時刻との差を取る | p95: 直接 ≤ 500 ms、転送 1 段 ≤ 800 ms |
| asset の反映 | `publish_game_asset` を送った時点（署名と blob の登録を含む）から、相手の Player でスキンを適用した frame まで（2026-10-29 のユーザー判断） | p95 ≤ 10 s。適用中の frame time の最大 ≤ 33 ms |
| 送信量 | 経路（Direct P2P / Relay Supported P2P / Relay Fallback）ごとの送信 bytes | 対戦 ≤ 200 kbps / 人、配信者の上り ≤ 10 Mbps |
| 切断からの復帰 | proxy で全 packet を 3 秒落とし、戻してから対戦の packet の往復と視聴の frame が再び届くまで | 対戦 ≤ 5 s、視聴 ≤ 20 s |
| SDK の memory | SDK の DLL の Rust 側が確保している bytes（確保量を数える global allocator で測る。2026-10-29 のユーザー判断） | ≤ 256 MiB / process。10 分の測定で、開始 1 分後からの増加が 5% 以内 |
| poll | 1 frame の poll（64 件まで）にかかった時間 | p99 ≤ 1 ms |

#### 8.5 poll の既定値

Unity の 1 frame の poll は 64 件・1 MiB まで、かつ 1 ms を超えたら次の frame へ回す。1 MiB を超える 1 件（720p の I420 は約 1.3 MiB）は §3 の規則で単独で受け取る。

#### 8.6 限定 probe の結果（2026-10-29）

probe のコードは repository に残していない（この節の値の根拠としてだけ使った）。

| 確かめたこと | 結果 |
| --- | --- |
| build | 6000.3.27f1 で URP 17.3.0・NGO 2.13.3・glTFast 6.20.0 を入れた project の Mono と IL2CPP の Windows x64 Player が build できた |
| C ABI の往復 | graphics は Direct3D 12。`kukuri-cli` の dispatcher を載せた cdylib を両方の Player から呼び、同意待ちの状態が返った。応答は次の frame の poll で届き、poll 1 回は p99 0.01 ms。DLL は 119 MB で、依存は OS の DLL と VCRUNTIME140 だけ（§8.1 で静的 CRT にする。映像の行の値は、libvpx を同じ DLL に link した構成で測った。静的 CRT の DLL は probe で確かめていないので、#1984 の CI と clean な環境で確かめる） |
| named pipe | 1 KiB の往復 p50 40 µs・p99 100 µs、8 MiB の往復 約 400 MB/s。切断・再接続を 3 回くり返して成功し、同じ名前の first instance の二重作成は拒否された |
| 映像 | 1280×720 の RenderTexture の readback は約 2 frame（p50 29〜33 ms）。RGBA → I420 は 1.5 ms、VP8 の圧縮は p50 2.1 ms・p95 3.9 ms、復号は p50 0.9 ms、I420 の 3 面の upload は p50 0.08 ms。取り込みから texture までは p50 33 ms。圧縮と変換は probe では main thread で行い、frame time の p95 が 18.5 ms に上がった（§8.3 で worker thread にする理由） |

### 9. 子 Issue の担当

| 子 Issue | 担当するもの | 担当しないもの |
| --- | --- | --- |
| #1975 | `crates/game-sdk` の C ABI・待ち行列・batch・panic の隔離、`events_lost`、profile の種類 `GameSdk` と account・同意の接続 | Sidecar、Unity、各柱の機能 |
| #1976 | Windows の Sidecar と IPC、attach・detach、複数 profile | C ABI、Unity |
| #1977 | UPM・C# API・mode の切替・Unity の lifecycle | 各柱の UI と挙動 |
| #1978 | action session の room、発見、公開範囲、`join_game_session` / `leave_game_session`、session の寿命と `game_session_closed`（退出・終了・失効で受け口が packet を受け付けなくなること） | packet の配送、裁定 |
| #1979 | `kukuri_sdk_send_binary` と batch の bytes の項目・未解放の batch の上限、session の datagram と stream、送受信の上限、`game_session_packet` / `game_session_path`、NGO の Transport adapter | session の寿命の判定、裁定、予測と補正 |
| #1980 | 発行・発見・取得・保持・検証、asset の command と `game_asset_updated`、Unity のスキンの読み込み | 課金・DRM |
| #1981 | SNS の command の利用、`post_received` と `ReactionReceived` による対象の解決、掲示板 Prefab、差分の表示 | Live の映像、3D の演出 |
| #1982 | Live の command の登録、Unity の映像・音声 adapter、`live_video_frame` / `live_watch_status`、既存 Live との相互配信、転送 | 3D の演出 |
| #1983 | Arena の操作・裁定・同期、4 本柱の連動、4 Prefab | 各柱の SDK 機能 |
| #1984 | FFI と Sidecar の適合、E2E、SLO の計測、CI、配布物、日本語の導入手順 | 各柱の単体試験 |

### 10. 適合 fixture

後続の実装は次の fixture を試験にする。FFI と Sidecar の両方で同じ結果になることを #1976 と #1984 で確かめる。ID は試験名と PR の説明で参照する。既存の command そのものの結果は、既存の試験が確かめる。

| ID | 対象 | 事前状態と操作 | 期待する結果 | 担当 |
| --- | --- | --- | --- | --- |
| F-ABI-1 | ABI の版 | C# の期待する版と DLL の版が違う | `ABI_MISMATCH`。handle を作らず、network I/O 0 件 | #1975・#1977 |
| F-ABI-2 | 起動 | 同意済みの profile で open | Ready の event。その後の要求が実行される | #1975 |
| F-ABI-3 | 相関 | 2 件の要求を続けて送る | それぞれの `request_id` で応答が返る | #1975 |
| F-ABI-4 | 重複 | 未完了の要求と同じ `request_id` で送る | `INVALID_ARGUMENT`。先の要求は続く | #1975 |
| F-ABI-5 | 満杯 | 未完了の要求が 256 件ある | 257 件目の send が `BACKPRESSURE`。既存の要求は失われない | #1975 |
| F-ABI-6 | 大きすぎる要求 | 1 MiB を超える JSON を送る | `INVALID_ARGUMENT` | #1975 |
| F-ABI-7 | 取消 | 実行前の要求を cancel | `cancelled` | #1975 |
| F-ABI-8 | 実行中の取消 | 実行中の要求を cancel | 実際の結果が返る | #1975 |
| F-ABI-9 | 期限 | 応答が期限までに来ない要求 | `timeout` | #1975 |
| F-ABI-10 | 終了 | close の後に send（send_binary は #1979） | `CLOSED` | #1975 |
| F-ABI-11 | 終了中の要求 | 読み取りと書き込みの実行中に close | 読み取りは `interrupted`。書き込みは結果か `operation_outcome_unknown`。知らせずに消えない | #1975 |
| F-ABI-12 | panic | handler が panic する | handle が `FAULTED`、未完了は `internal_error`、process は続く | #1975 |
| F-ABI-13 | event の溢れ | poll しないまま知らせの event が 1024 件を超える | 古い event を捨て、`events_lost` で件数が届く。応答は失われない | #1975 |
| F-ABI-14 | poll | 届いたものが無い | batch を返さず（null）即座に戻る | #1975 |
| F-ABI-15 | batch の解放 | batch を解放した後に、同じ batch と別の handle の batch を解放する | どちらも `INVALID_ARGUMENT`。他の batch は有効なまま | #1975 |
| F-ABI-16 | 未解放の上限 | batch を解放しないまま 64 MiB に達する | 新しい映像と datagram は捨て、応答は届く。解放すると再開する | #1979（bytes の項目を足すとき） |
| F-ABI-17 | 秘密の出力 | 招待 token の export を送る | token は秘密の出力だけに入り、応答の JSON・event・log に出ない | #1975 |
| F-PRO-1 | 同意前 | 新しい profile で open | 同意待ち。network I/O 0 件 | #1975 |
| F-PRO-2 | 再起動 | 作成 → close → open | 同じ公開鍵 | #1975 |
| F-PRO-3 | import | 正しい export と passphrase で import し、切り替える | SNS と同じ公開鍵になる | #1975 |
| F-PRO-4 | 誤った passphrase | import | 失敗し、既存の profile は変わらない | #1975 |
| F-PRO-5 | 二重所有 | FFI が開いている profile を Sidecar でも開く | `profile_in_use` | #1975・#1976 |
| F-PRO-6 | 別の種類 | kukuri の GUI・CLI の profile を SDK で開く | `profile_kind_mismatch`。profile は変わらない | #1975 |
| F-IPC-1 | 接続の検証 | 版が違う client、別 user の client | handler の前に拒否。公開 TCP の listen 0 件 | #1976 |
| F-IPC-2 | 切断 | 読み取りと書き込みの実行中に client が切れる | 読み取りは `interrupted`、書き込みは `operation_outcome_unknown`。自動の再実行 0 件 | #1976 |
| F-IPC-3 | attach の反復 | detach と attach を繰り返す | 同じ endpoint id。observer と task が増えない | #1976 |
| F-LOB-1 | 作成と発見 | profile A が room を作る | profile B の一覧に出る。late join でも出る | #1978 |
| F-LOB-2 | 終了 | host が room を終える | 一覧で終了と分かり、参加できない | #1978 |
| F-LOB-3 | 一覧の量 | 無関係な room と履歴を増やす | 1 回の一覧の取得量が増えない | #1978 |
| F-SES-1 | 公開範囲 | 4 種類の公開範囲で、条件を満たす人と満たさない人が参加する | 満たす人だけが参加でき、満たさない人は理由の code だけを受ける | #1978 |
| F-SES-2 | 退出後 | 退出・room の終了・権限の失効の後に packet を送る | 送った側は `game_session_closed` を受け、send_binary は `CLOSED`。相手に届かない | #1978 |
| F-SES-3 | 配送 | 固定の遅延・損失・順不同の下で datagram と stream を送る | datagram は欠けても新しいものが届き、stream は順に全部届く | #1979 |
| F-SES-4 | 大きすぎる packet | datagram・stream の上限を超える payload を送る | `INVALID_ARGUMENT`。送らない | #1979 |
| F-SES-5 | 受け手の遅れ | 受け手が poll しないまま stream の message が 256 KiB を超える | 捨てない。送り手は `BACKPRESSURE` を受け、poll 後に残りが順に届く | #1979 |
| F-AST-1 | 正しい release | 発行 → 取得 | 検証済みの path が届き、適用できる | #1980 |
| F-AST-2 | 不正 | 別の鍵・改変した blob・古い revision | 適用しない。今のスキンを保つ | #1980 |
| F-AST-3 | 提供者不在 | 保持 peer が誰もいない | 取得できないことを返す。成功と扱わない | #1980 |
| F-SNS-1 | 相互運用 | Unity で投稿・返信・リアクション、desktop で返信 | 同じ ID で両方に表示され、Unity は `post_received` を受ける | #1981 |
| F-SNS-2 | 公開範囲と block | private の非参加者・block した作者 | 既存の規則どおり読めない・表示しない | #1981 |
| F-LIV-1 | 明示開始 | 配信を始める操作の前 | 取り込みと送信 0 件 | #1982 |
| F-LIV-2 | 相互配信 | Unity → desktop・Web、desktop・Web → Unity | 映像と音声が届く | #1982 |
| F-LIV-3 | 転送 | 配信者 1・視聴者 5 | 直接の枠を超えた視聴者は転送で届く | #1982 |
| F-LIV-4 | 不正な受信 | 署名を検証できない frame、資格の無い視聴の要求 | frame は捨てて親から離れる。要求は理由の code だけで断る | #1982 |
| F-LIV-5 | 停止 | 配信・視聴を止める | 取り込み・codec・texture・転送を解放する | #1982 |

## Consequences

- SDK は CLI と同じ登録簿を使うため、command を足すと desktop の CLI と SDK の両方で使える。登録簿の変更は両方の試験で確かめる。
- Windows の Sidecar は CLI 全体の Windows 移植ではなく、SDK の開発用として `SdkClient` を中継する範囲に限る。
- 4 本柱の正本は既存の docs・blob・hint・投稿・Live のままで、SDK だけのデータの正本は作らない。新しく分類したのは §5.1〜§5.3（SDK の profile と要求の経路、ゲームの通信、アセット配布）である。
- Unity の版・映像方式・閾値は §8 で確定した。閾値は結果を見て緩めず、未計測のものを達成とは扱わない。libvpx の BSD-3-Clause と PATENTS の表示は #1984 の第三者の notice に含める。

## 参照

- ADR 0002（feature の分類の雛形）、ADR 0005（Live session）、ADR 0006（Game Room）、ADR 0018（private channel の世代と参加の token）、ADR 0049（CLI のローカル制御経路）、ADR 0063（公開 blob の発見）、ADR 0067（media の搬送）、ADR 0068（公開 channel）
- Issue #1973（親）、#1974〜#1984（子）
