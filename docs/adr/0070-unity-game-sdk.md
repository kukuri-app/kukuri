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
| command | `set_public_channel_subscription` | #1982 | 公開スレッド（公開 channel。ADR 0068）の投稿・リアクションの知らせを購読する・やめる。desired として保存し、topic の購読と同じ上限に数える（CLI と SDK だけ。desktop・Web は開いている列で購読する）。保存は旧版の読まない `public-channel-subscriptions.json` に分け（旧版は `subscriptions.json` の channel を private channel として復元して起動に失敗するため。2026-10-31 のユーザー判断）、backup には含めない |
| command | `start_game_player` / `get_game_player` | #1996 | profile の game player を作る・読む（1 つの profile に 1 つのゲーム。§4.1） |
| command | `open_account_transfer` / `get_account_transfer_status` / `decide_account_transfer` / `cancel_account_transfer` | #1996 | desktop・Web から既存のアカウントを移行で受ける（ADR 0062 の既存の操作を登録簿へ載せる。リンクは秘密の入力。§4.1） |
| command | `create_account_transfer_invite` | #1998 | SDK の profile を移行元にして、ゲームのアカウントを desktop・Web へ移す招待を出す（リンクは秘密の出力。§4.2） |
| command | `merge_game_player` | #1998 | ゲームの結び付きを、profile にある別のアカウントへ付け替える（§4.2） |
| command | `get_cloud_account` / `set_cloud_account` | #1997 | Steam Cloud で同期する、ゲームで作ったアカウントの鍵と game player の file を読む・書く・消す（§4.3） |
| event | `events_lost` | #1975 | 知らせの event を捨てた件数 |
| event | `game_session_closed` | #1978 | session が閉じた（退出・room の終了・権限の失効・host の不在）。理由の code を持つ。参加者と host の両方の端末に届く（host の端末は #1979 から） |
| event | `game_session_opened` | #1979 | host の端末で session を受け付け、送受信を始めた。session ID・room ID・参加者の公開鍵を持つ |
| event | `game_session_packet` | #1979 | datagram または stream の message を受けた。どちらで届いたかを持ち、bytes を伴う |
| event | `game_session_path` | #1979 | 接続の経路（Direct P2P / Relay Supported P2P / Relay Fallback）か RTT が変わった。その時点の送受信の bytes も持つ |
| event | `game_asset_updated` | #1980 | 検証できる新しい release が届いた。この端末で適用中のアセットは取得・適用した後に file の path 付きで、まだ適用していないアセットは path 無しで届く |
| event | `post_received` | #1981 | 購読中の topic・channel の投稿・返信を手元へ新しく保存した（自分の投稿を含む）。投稿 ID・topic・channel・作者・返信先・thread の root を持ち、本文は持たない。作成から 30 秒を過ぎたもの（過去の頁の読込みで届いた古い投稿）と非表示の作者（mute・block）のものは送らない。溢れたときは `post` が null で、受け手は表示中の timeline・thread を読み直す |
| event | `live_video_frame` | #1982 | 視聴中の session の復号済みの映像（画素の形式は §8）。幅・高さ・frame の timestamp・そのときの音声の再生時刻・配信者からの段数（`hops`。1 は直接、2 以上は視聴者の転送を通る）を持ち、bytes を伴う。session ごとに最新の 1 枚だけを持ち、poll していない古い frame は捨てる |
| event | `live_watch_status` | #1982 | 視聴の状態（`watching`・`full`・`no_media`・`ended`）が変わった |

リアクションの受信は既存の `ReactionReceived`（対象の投稿を持つ）をそのまま使う。

#### C ABI

| 関数 | 働き | 待ち |
| --- | --- | --- |
| `kukuri_sdk_abi_version()` | ABI の版を返す。C# は読み込み直後に照合し、違えば理由を示して開始しない | なし |
| `kukuri_sdk_open(abi_version, config, out_handle)` | ABI の版と profile（名前と directory）を指定して handle を作る。版が違えば `ABI_MISMATCH` で handle を作らない。runtime の起動は背景で進み、完了を `startup_status_changed` の event で知らせる。起動に失敗したら（`profile_in_use` 等）、要求の応答がその error を返す。Sidecar の mode では runtime は既に動いており、接続の前の event は渡さないので（§8.2）、`get_desktop_startup_status` で状態を読む | なし |
| `kukuri_sdk_send(handle, request, secret)` | 要求を待ち行列に入れて即座に戻る。結果は poll で受ける | なし |
| `kukuri_sdk_send_binary(handle, kind, target, bytes)` | session の packet（datagram・stream）と、配信する映像・音声の bytes を渡す。`target` は session ID または Live の session ID。kind は 1 が datagram、2 が stream、3 が live の映像（`[幅 u16][高さ u16][下の行からの RGBA32]`、1280×720 まで）、4 がゲームの音（`[sample rate u32][channel 数 u16][interleave した f32]`）。数値は little endian。#1979 で足し、ABI の版を 2 にした（3・4 は #1982 で足した。版は変えない）。映像と音は配信を始める前と止めた後は `CLOSED` | なし |
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
  3. 同意すると account が作られ、ゲームだけで使い始められる（ゲーム用のアカウント。§4.1 の「はじめる」）。
  4. 既存の kukuri のアカウントを使う人は、§4.1 の「kukuriアカウントを使う」で、desktop・Web から既存の移行（ADR 0062）でアカウントを受ける。暗号化 export した文字列と passphrase の入力（`preview_account_key_import` と `import_account_key` の後に `switch_account`）は、端末を失ったときの復旧の上級の操作として残し、通常の開始の操作にしない（#1996）。誤った passphrase では既存の profile を変えない。
  5. Community Node を使う機能は、既存の CN 同意（`fetch_community_node_policies` / `accept_community_node_consents`）を経てから接続する。
- 取り込み画面は、取り込んだアカウントでこのゲームが投稿・署名できることを示す。ゲームのコードは profile と同じ OS の権限で動くためである。
- 秘密の値を Inspector の serializable field、scene・prefab、PlayerPrefs、診断の出力に保存しない。

#### 4.1 開始の入口とゲームのプレイヤー（#1996 AC-1）

2026-10-31 のユーザー判断で、次の 3 点を決めた。

- 既存のアカウントでゲームが署名する方法は、既存の移行（ADR 0062）で鍵を渡す形にする。
  - 選ばなかった案は 2 つある。用途を限った委任鍵は、投稿・リアクション・room を読むすべての経路で委任状を検証する変更が要る。そのうえ、旧版の client がゲームの投稿を本人のものと認めない。desktop に署名を頼む形は、desktop・Web を開いている間しかゲームで投稿・参加できない。
- 移行のリンクは、ゲームの「貼り付け」のボタンが clipboard から読む。
- 「開始済み」は「はじめる」の時点とする。

外部ログイン（#1997）と、引継ぎ・異なる鍵の統合（#1998）はこの節の外とする。

##### 入口

| 入口 | 操作 | 結果 |
| --- | --- | --- |
| 新規 | 同意（§4 の 1・2）→「はじめる」 | 同意で作られたゲーム用のアカウントに、game player を結び付けて始める。別サイトの登録・メールとパスワード・SNS のプロフィールの入力は要らない。プロフィールの入力を求める `profile_setup_required` は GUI の側の判定で、runtime は投稿・参加の前に求めない |
| 保存済み | 起動 | 保存した game player とそのアカウントで再開する（同意と「はじめる」を繰り返さない） |
| 既存の kukuri アカウント | 開始の画面の「kukuriアカウントを使う」→ desktop・Web の移行の画面で「リンクをコピー」→ ゲームで「貼り付け」→ 両方に出る 6 桁の確認コードの一致を両方で押す → 移行の完了の後に「はじめる」 | 受けたアカウントへ切り替え、そのアカウントに game player を結び付けて始める。鍵・export の文字列は扱わない |

- 「kukuriアカウントを使う」を押した後、貼り付けの前に、ゲームの画面で次のことを示す。
  - このゲームがそのアカウントで投稿・署名できること。
  - ゲームの端末に鍵が残り、desktop 側から後で取り消せないこと。解除はゲームの profile から外すことになる。
- desktop・Web の移行元の画面は、相手がゲームだと分からないので、既存の表示（ADR 0062 §8）のままとする。
- 開始の画面は「はじめる」を主の操作にし、「kukuriアカウントを使う」は任意の入口として並べる。アカウントの方式を選ぶ画面を全員に強制しない。

##### game player

- game player ID は、SDK の profile ごとに SDK が作るランダムな ID である。ゲームはセーブの参照に使う。セーブの内容はゲームが持ち、SDK は持たない。
- game player ID はアカウントの公開鍵とは別の値で、アカウントの公開鍵・端末の profile・外部の provider の主体 ID のどれでもない。後から既存のアカウントへまとめても（#1998）、既定では game player ID は変わらない。両方に進行があるときは、ゲームが渡す ID に付け替える（§4.2）。
- 1 つの profile は 1 つのゲーム（application ID）の game player だけを持つ。別の application ID を渡すと `game_player_application_mismatch` で断る。異なるゲームが profile とアカウントを暗黙に共用しないためである。
- game player は、作った時のアカウントに結び付く（binding）。保存先は profile の directory の `game-players.json`（1 件）で、network へは出さない。
- 保存は ProfileLease の中で行う。旧版の SDK はこの file を読まない（前からの profile を開く向きに影響は無い）。

##### 状態の表

| 今の状態 | 操作・出来事 | 次の状態 |
| --- | --- | --- |
| 同意前 | 同意する | 同意済み・未開始（ゲーム用のアカウントを作り、network I/O を始める） |
| 同意前 | 同意しない・閉じる | 同意前のまま（network I/O 0 件、アカウントを作らない） |
| 未開始 | 「はじめる」 | 開始済み（今のアカウントに game player を結び付ける） |
| 未開始 | 開始の画面を閉じる・再起動 | 未開始のまま（同じゲーム用のアカウントで開始の画面に戻る。新しいアカウントを作らない） |
| 未開始 | 「kukuriアカウントを使う」→ 確認コードの一致を両方が承認 → 移行の完了 | 未開始（ゲームが `switch_account` で受けたアカウントへ切り替える。前のゲーム用のアカウントは profile に残し、使わない） |
| 未開始・開始済み | 移行の拒否・期限切れ（5 分）・取消・届かない・移行の途中の再起動 | 前の状態のまま（アカウントも切り替えも変えない。ADR 0062 のとおり、確認の前に終わった移行は何も保存しない） |
| 開始済み | 再起動 | 開始済み（`start_game_player` が、結び付けたアカウントを active にして同じ game player で再開する） |
| 開始済み | 別のアカウントを移行で受けて `switch_account` を呼ぶ | 開始済みのまま切り替えない。`switch_account` は `integration_required` を返す。受けたアカウントは profile に加わる（統合の実行は #1998） |

開始の前に、自動で作ったアカウントで公開投稿・room への参加を始めない。「はじめる」の前に SNS・ロビーを開かない作りは、ゲームの開始の画面（AC-2 の sample）が担い、AC-2・AC-3 の確認に含める。

##### API・保存・互換

- 足す command:
  - `start_game_player`（application ID）: game player を作る、または保存済みのものを返す。返すのは game player ID とアカウントの公開鍵。
  - `get_game_player`（application ID）: 保存済みの game player を返す。無ければ null。ゲームは、起動時に開始の画面を出すかどうかをこれで決める。
- 登録簿へ載せる既存の移行の操作（今は GUI だけ）:
  - `open_account_transfer`: 移行先として開く。リンクは秘密の入力で渡し、応答・event・log に出さない。投稿の履歴は受けない（`history` は無し）。移行の完了は必須 bundle の保存だけで決まり、開始を投稿の数に比例して待たせない。
  - `get_account_transfer_status`: 完了（`completed`）なら、受けたアカウントの ID を返す。
  - `decide_account_transfer`
  - `cancel_account_transfer`
- 移行は、受けたアカウントを profile に足すだけで active を変えない（ADR 0062）。切り替えは、移行の完了の後にゲームが既存の `switch_account` で行う。
- SDK の profile に game player がある間は、`switch_account` は結び付けたアカウント以外へ切り替えず、`integration_required` を返す（SDK の dispatcher で判定する）。統合が要る状態を `switch_account` で処理しないためである。
- 保存の形の変更は `game-players.json` の追加だけで、既存の DB・backup・wire・`AccountTransferStatus` の型は変えない。

##### 再利用と削除

- 再利用するもの:
  - 同意の後のアカウントの自動の作成（ClientHost）
  - 移行の招待・接続・確認コード・必須 bundle（ADR 0062）
  - 複数のアカウントと切替え（ADR 0047）
- Minimal sample の、ready の後に出す export の文字列と passphrase の入力欄は、AC-2 で「復旧」の上級の操作へ移し、開始の画面から外す。

##### AC-2・AC-3 の固定の確認

- AC-2:
  - 空の profile から、同意 →「はじめる」で始まる。同意の前の network I/O は 0 件である。
  - 開始の画面を閉じる・再実行・再起動しても、公開鍵と game player ID が同じである。開始の画面で取り消すと（同意しない・閉じる）、既存のアカウントを上書きせず、新しいアカウントも作らない。
  - 別の application ID は断る。
  - 「はじめる」の前に、開始の画面の sample は SNS・ロビーを開かない。
  - SDK の試験と、Windows の実 Player の開始の画面の sample で確かめる。
- AC-3:
  - desktop の runtime を移行元にして招待を作る。SDK が「貼り付け」の経路（秘密の入力）で受け、確認コードの承認・完了・`switch_account` を経る。既存の公開鍵で始められることと、再起動しても同じであることを確かめる。
  - 拒否・期限切れ・取消では、元のアカウントと game player を保つ。
  - 開始済みで別のアカウントを受けると、`switch_account` が `integration_required` を返して切り替えない。
  - SDK の試験に加え、作者が管理する desktop と Windows の実 Player で、貼り付け → 確認 → 開始 → 再起動を確かめる。

#### 4.2 ゲーム用のアカウントの引継ぎと、既存のアカウントへのまとめ（#1998 AC-1）

2026-10-31 のユーザー判断で、次の 3 点を決めた。

- ゲーム用のアカウント A を既存のアカウント B へまとめるとき、A と B が同じ人だという関連付けは公開しない。
  - 選ばなかった案は、A と B の両方の鍵で署名した「同じ人」の記録を公開する形である。対応した client が A の過去の投稿を B の過去として並べられる一方、旧版の client は読まず、公開した記録は取り消しても見た人の手元に残る。
- 両方に同じゲームの進行があるときは、ゲームが決める。SDK は他の端末の進行を知らず、ゲームが自分のセーブで B の進行を見つけて、残す game player ID を渡す。
  - 選ばなかった案は、game player の結び付きを B の本人の端末どうしの同期に載せ、SDK が両方を示す形である。§4.1 で端末の外へ出さないとした結び付きを同期に載せる変更になる。
- A が付けたフォロー・ブロック・ミュートは、まとめるときに B で付け直す。フォローとブロックは公開なので、A と B の関係を推測される余地は残る（関連付けそのものは公開しない）。A が参加している private channel は写さず、B を招待し直してもらう。
- まとめた後、ゲームの中では A に切り替えない。A の管理（A が owner の private channel への招待、A の投稿の取り下げ、A のプロフィールの変更）をしたい人には、まとめる前に「kukuriで続ける」で A を desktop・Web へ移すよう案内する。ゲームの端末が担っていた A の private channel の鍵の更新は止まる。

同じ鍵のまま広げる移行（ADR 0062）と、別のアカウントの追加（ADR 0047）は既存の契約である。異なる鍵の A と B をまとめるのは、ゲームの結び付きを A から B へ付け替える新しい操作で、署名・投稿・権限の正本は変えない。外部ログインの連携（#1997）はこの節の外とする。まとめた後の連携の扱いは #1997 の AC-1 が §4.3 に書く。#1998 の AC-3 は、外部ログインの連携が無い profile で判定する。

##### 入口

| 入口 | 操作 | 結果 |
| --- | --- | --- |
| 「kukuriで続ける」（AC-2） | 開始済み・未開始のどちらでも。ゲームが移行元になって「リンクをコピー」→ desktop・Web の「別の端末から移す」に貼り付ける（desktop はリンクの起動でも受ける）→ 両方に出る 6 桁の確認コードの一致を両方で押す → desktop・Web で「このアカウントを使う」 | desktop・Web が A を受けて A に切り替え、A の会話を表示・返信できる。ゲームは A と同じ game player のまま続く |
| 「既存のアカウントへまとめる」（AC-3） | 開始済みのゲームで押す → 説明 → まとめる先を選ぶ → 残す進行を選ぶ → 確定。まとめる先は、profile に既にある結び付き以外のアカウント（前に受けたもの）から選ぶか、desktop・Web の「別の端末へ移す」のリンクを貼り付け、確認コードを両方で承認して受ける（§4.1 と同じ移行） | ゲームの結び付きを B へ付け替え、B に切り替える。以後は B で投稿・参加する。A のフォロー・ブロック・ミュートを背景で B に付け直す |

- 「既存のアカウントへまとめる」を押した後、貼り付けの前に、ゲームの画面で次のことを示す。
  - 以後はまとめた先のアカウントで投稿・参加し、ゲームの進行はそのアカウントに付く。
  - このゲームのアカウントの過去の投稿は、このゲームのアカウントのまま残る（まとめた先が書いたことにはならない）。他の人からは別の人に見え、フォロワーは移らない。
  - フォロー・ブロック・ミュートはまとめた先で付け直す。フォローとブロックは公開されるので、関係を推測されることがある。
  - 参加している private channel には、まとめた先のアカウントを招待し直してもらう。
  - まとめた後、このゲームでは前のアカウントを使えない。前のアカウントで作った private channel への招待、投稿の取り下げ、プロフィールの変更をしたいときは、先に「kukuriで続ける」で kukuri app へ移す。この端末が担っていた private channel の鍵の更新は止まる。
  - まとめた先のアカウントの鍵がこの端末に残り、kukuri app から後で取り消せない（§4.1 と同じ）。
  - Steam Cloud に保存していれば、保存をやめる（まとめた先の鍵は Steam に置かない。§4.3）。
- 両方の所有の確認は、B は移行元の desktop・Web での確認コードの承認、A はゲームの画面での確定とする。A の鍵はゲームの profile にあり、確定はゲームの利用者にしかできない。
- desktop・Web の画面は既存の移行の画面のまま変えない（相手がゲームだと分からない。ADR 0062 §8）。

##### 状態の表

| 今の状態 | 操作・出来事 | 次の状態 |
| --- | --- | --- |
| A で開始済み・未開始 | 「kukuriで続ける」→ 承認 → 完了 | 同じ。desktop・Web に A が加わる。ゲームの結び付き・active は変えない |
| A で開始済み・未開始 | 「kukuriで続ける」の拒否・期限切れ・取消・届かない | 同じ（移行元は何も保存しない。ADR 0062） |
| A で開始済み | 「既存のアカウントへまとめる」で B を受ける前・受ける途中の拒否・期限切れ・取消・届かない・再起動 | 同じ（§4.1 の表のとおり） |
| A で開始済み・B を受けた | 残す進行の選択で取り消す・閉じる・再起動 | 同じ。B は profile に加わったまま使わない。`switch_account` は B への切替を `integration_required` で断る。あとで「まとめる」をやり直せる（B をもう一度受けなくてよい） |
| A で開始済み・B を受けた | 確定（`merge_game_player`） | B で開始済み。結び付きを B へ付け替え（game player ID は残す進行のもの）、B を active にする。A のフォロー・ブロック・ミュートの付け直しを背景で始める |
| B で開始済み・付け直しの途中 | 再起動 | 同じ。`start_game_player` が B で再開し、付け直しを続きから進める |
| B で開始済み・付け直しの途中 | もう一度まとめる（`merge_game_player`） | 断る（`merge_in_progress`）。付け直しが終わってからやり直せる |
| B で開始済み | A への `switch_account` | 断る（`integration_required`）。A は profile に残り、鍵も消さない |

確定の前のどの失敗・取消も、A の結び付き・active・投稿・ゲームの進行を変えない。

確定（`merge_game_player`）は次の順に進む。Steam Cloud に保存していれば（§4.3）、1 の前に `game-account.json` を消す。

1. 付け直しの file（A と B の account ID、確定の時刻と、対象の種類ごとの進んだ位置）を書く。
2. 結び付きの file を B へ書き換える（temp からの rename）。確定はこの rename で決まる。
3. B へ `switch_account` する。

「付け直しの途中」は、付け直しの file があり、その B が結び付きのアカウントと一致するときとする。どちらの file も Write の command（dispatcher が直列に実行する）だけが書く。ただし、背景の付け直しは付け直しの file の進んだ位置だけを書き、結び付きの file には触れない。付け直しの途中のもう一度のまとめは断るので、付け直しの file を command と背景が同時に書き換えることはない。

途中で止まったときは次のとおりになる。

- 2 が失敗したら、その場で付け直しの file を消して失敗を返す（結び付きは A のまま）。
- 1 の後・2 の前に process が止まると、結び付きは A のまま。付け直しの file は B が結び付きと一致しないので途中とは扱わず、次の起動で捨てる。
- 2 の後・3 の前に止まる、または 3 が失敗すると、応答は失敗でも確定は済んでいる。ゲームは `get_game_player` で結び付きを確かめる。active は次の `start_game_player` が結び付けた B へ合わせる（§4.1）。

SDK はセーブを持たず、何も消さない。ゲームが B の進行の ID を渡す場合は、確定の前に `get_game_player` で今の ID を読み、選ばれなかった側の進行として自分のセーブに残す。

##### 扱う対象

| 対象 | まとめた後 |
| --- | --- |
| A の過去の投稿・返信・リアクション | A の ID・署名・著者のまま変えない。再署名・再投稿しない。A の公開鍵で引き続き読める。取り下げ・編集は A を持つ desktop・Web で行う |
| 以後の投稿・room への参加 | B が署名する |
| A のフォロー・ブロック | A の有効なもの（解除していないもの）だけを、B で同じ相手へ付け直す（B の新しい署名つきの記録。A の記録は消さない） |
| A のミュート | B のミュートに足す（手元だけの設定） |
| 他の人が A に付けたフォロー・ブロック | 変えない（他の人の記録を書き換えない） |
| A が参加している private channel | B へ写さない。B を招待し直してもらう。A の参加資格と鍵は A に残す（他の参加者から見て A を外さない）。B が自分で参加している private channel はそのまま |
| A の鍵 | profile に残す。ゲームでは使わない。A への切替は断る。この端末が担っていた A の private channel の鍵の更新は止まる |
| game player ID | 残す進行の ID（既定は今の ID。ゲームが B の進行を選んだときはゲームが渡す ID） |
| A と B の対応 | どこにも公開しない。付け直しの途中だけ、付け直しの file に A の account ID を持つ |

付け直しは次のとおりに進める。

- B の移行で受けた必須 bundle の反映（ADR 0062 §5）が終わってから始める（B の DB の隣の移行の置き場が無くなったことで判定する）。B が持っていたフォロー・ブロックが store に入る前に付け直すと、重複した記録ができ、B が解除していた相手を上書きするためである。
- A の手元の DB を読取り専用の接続で開き、フォロー・ブロック・ミュートのそれぞれを、相手の公開鍵の順に 64 件ずつ読む。フォロー・ブロックは主キー `(subject_pubkey, target_pubkey)`、ミュートは主キー `author_pubkey` で、続きの位置から読める。ミュートの続きの位置の照会は新しく作る。
- 相手が B 自身の行は飛ばす。確定より前に B が同じ相手へ付けたフォロー・ブロック（解除済みを含む）か、今のミュートがあれば付け直さない（ミュートは解除で記録が消えるので、解除済みは数えられない）。確定より後でも解除した記録（B が別の端末で外したものを含む）は付け直さない。確定より後の有効な記録は、止まる前に途中まで書いた付け直し（手元にだけ書いて、公開と本人の端末どうしの同期の前に止まったもの）かもしれないので、次の起動で書き直す（同じ状態の記録が増えるだけ）。1 行の付け直しを core が拒否したとき（相手の公開鍵が不正など、やり直しても同じ結果になるもの）は、その行を飛ばして次へ進む（同じ行で止まり続けない）。それ以外の失敗（runtime の停止・切替の途中、書込みの失敗など）では位置を進めずに止め、次の起動で同じ行から進める。
- 進んだ位置を付け直しの file に保存し、再起動で続きから進める。全部終えたら付け直しの file を消す。
- 付け直しの task は profile ごとに 1 つで、動いている間は新しく始めない（`start_game_player` を繰り返し呼んでも 2 つ目を作らない）。
- 確定の応答と開始を、A のフォロー等の件数に比例して待たせない。

##### API・保存・互換

- 登録簿へ足す command:
  - `create_account_transfer_invite`（AC-2）: 今の SDK の profile を移行元にして招待を出す。リンクは Secret の出力で返し、応答の data・event・log に出さない。状態・承認・取消は §4.1 の command を使う。
  - `merge_game_player`（AC-3）: application ID・まとめた先の account ID・任意の game player ID を受ける。結び付きが無い、account ID が今の結び付きと同じ・profile に無い、game player ID が UUID でない、のどれかなら断る。付け直しの途中なら `merge_in_progress` で断る。結果は `start_game_player` と同じ形。
- `start_game_player`・`get_game_player` の出力に、付け直しの途中かどうか（`copying`、真偽）を足す。
- `game-players.json` の形は変えない（版 1 のまま。版 1 の SDK もまとめた後の結び付きを読める）。付け直しの途中の A の account ID と進んだ位置は、別の `game-player-merge.json` に置く。
- どちらの file も Local Only で、network・公開の topic・log へ出さない（§5.1 の profile と同じ分類）。
- 既存の DB・backup・wire・投稿・フォローの記録の形は変えない。付け直しは既存の `follow_author`・`block_author`・`mute_author` と同じ記録を B で作る。

##### 再利用と担当

- AC-2 は、既存の移行（ADR 0062）の移行元を SDK の登録簿へ載せ、Minimal sample に「kukuriで続ける」を足す。desktop・Web は既存の「別の端末から移す」で受ける。
- AC-3 は、§4.1 の移行で B を受ける経路を使い、`merge_game_player` と付け直し、Minimal sample の「既存のアカウントへまとめる」と残す進行の選択を足す。
- 既存の投稿の署名・認可・表示、private channel の招待は変えない。

##### AC-2・AC-3 の固定の確認

- AC-2:
  - SDK の profile で A を作って開始し、試験の topic に投稿する。SDK が招待を出し、desktop の runtime が受けて両方で承認する。desktop は A に切り替え、ゲームの端末に到達できる状態で、その topic の timeline から A の投稿を同じ投稿 ID で読み、返信する（履歴を移さないので、desktop のプロフィールには出ない）。ゲームは同じ game player と公開鍵のまま、その返信を受ける。
  - 拒否・期限切れ・取消では、ゲーム側の結び付き・active・アカウントの一覧が変わらない。
  - 端末の間の投稿の履歴の移行（ADR 0062 §6）を待たない。
  - SDK の試験に加え、Windows の実 Player と作者が管理する desktop で、コピー → 貼り付け → 承認 → desktop での返信 → ゲームでの続きを確かめる。
- AC-3:
  - A で開始し、投稿・フォロー・ブロック・ミュートをし、desktop の別のアカウント C が owner の private channel に参加する。A は B もフォローする。desktop の runtime が B を持ち、B でも投稿し、B が参加する private channel を持つ。ゲームが B を受けてまとめる。
  - 既定（今の game player ID を残す）と、ゲームが渡す ID を残す場合の両方で、結び付き・active・game player ID が決めたとおりになる。
  - 残す進行の選択で取り消すと何も変わらず、profile にある B を選んでやり直せる。付け直しの途中で再起動すると続きから進み、付け直しの途中のもう一度のまとめは `merge_in_progress` で断る。
  - A の投稿の ID・署名・著者は変わらず読める。B で新しく投稿・返信でき、desktop で読める。B が A のフォロー・ブロック・ミュートの相手を持ち、B 自身はフォロー・ミュートしない。A が参加していた C の private channel に B は入っておらず、C に招待されれば入れる。A の参加資格は残っている。B が自分で参加していた private channel には参加し続けられる。A への切替は `integration_required` で断る。
  - SDK の試験に加え、Windows の実 Player と作者が管理する desktop で、まとめる → 再開 → B での投稿 → desktop での表示を確かめる。

#### 4.3 Steam Cloud によるアカウントの保存と、別の端末での再開（#1997 AC-1）

2026-10-31 のユーザー判断で、次の 5 点を決めた。

- 最初の外部ログインは Steam とし、連携と再開は Unity（ゲームの中）だけに作る。desktop・Web とは既存の移行（§4.1・§4.2）で行き来する。
- アカウントの鍵と game player は、Steam Cloud に置く。SDK は profile の中に鍵と game player の file を書くだけで、ゲームの Steamworks の Auto-Cloud が、その file をその Steam 利用者の分として同期する。新しい端末では Steam が file を戻し、SDK はそこから同じ鍵と game player で再開する。Steam アカウントの本人確認は Steam が担い、SDK・Community Node は Steam の認証券を扱わない。ゲームの作者は検証の服務を運用しない。
- 復旧コードは使わず、Steam のログインだけで戻す。Steam（Valve）や、Steam アカウントを乗っ取った人も、kukuri の鍵を得られる。利便性と UX のために、この信頼を受け入れる。
- Steam Cloud に鍵を置けるのは、ゲームで作ったアカウントだけとする。「kukuriアカウントを使う」（§4.1）で受けたアカウントや、まとめた先（§4.2）は、desktop・Web で普段使う主のアカウントなので置かない。その人は、別の PC でも desktop・Web からの移行で受ける。
- 実際の Steam（Steamworks の AppID）での実機確認は、AC-2・AC-3 の判定から外す（Issue #1997 の受入条件の変更）。Steam Cloud の同期は「`game-account.json` を別の directory へ写す」ことで模し、実際の Steam で動くことは未確認として記録する。

選ばなかった案は次のとおり。

- Community Node が Steam の認証を検証し、復旧コードで暗号化した鍵を預かる形。Community Node の役割を増やしすぎる。
- NIP-05 のように Steam のプロフィールへ公開鍵を書く形。同じ人だと示せるが、公開鍵からは鍵を戻せず、旧端末なしの再開ができない。
- 受けた既存のアカウントの鍵も Steam Cloud に置ける形。Steam アカウントを乗っ取られると、主のアカウントも使われる。

##### ゲームで作ったアカウント

「ゲームで作ったアカウント」は、Local Only の `game-account-origin.json` に記録した account ID（1 件）とする。登録簿の順では決めない（最も古いアカウントを外すと、受けたアカウントを指してしまうため）。記録するのは次の 2 つの時点だけである。

- 同意（`accept_app_consents`）でアカウントを作った直後。このとき登録簿にはそのアカウントしか無い。
- Steam Cloud の file から戻したとき。戻したアカウントで書き換える。

受けたアカウント（移行・export の取り込み）と、`create_account` で後から足したアカウントは含めない。この記録の前の版で同意した profile は記録が無く、保存を有効にできない（SDK は未配布で、そうした利用者はいない）。

##### 状態

`get_cloud_account`（application ID）は次のどれかを返す。比べるのは `game-account.json` と手元の結び付きの、鍵と game player ID の両方である。

| 値 | 条件 |
| --- | --- |
| `off` | file が無い |
| `on` | 手元の結び付きがあり、file の鍵と game player ID が両方とも結び付きと同じ |
| `restorable` | 手元の結び付きが無く、同じ application ID の読める file がある |
| `other_account` | 手元の結び付きがあり、file の鍵か game player ID のどちらかが違う（別の端末で保存した） |
| `unreadable` | file が読めない（壊れている、知らない版、別の application ID） |

##### 入口と状態の表

「Steam Cloud に保存」は、Steam で動くゲームが、開始済みの profile で有効にする。SDK は Steam の有無を見ず、有効にするかはゲームが決める（Steam の API が使えるときだけ有効にする、など）。

| 今の状態 | 操作・出来事 | 次の状態 |
| --- | --- | --- |
| 開始済み・`off`・結び付きがゲームで作ったアカウント | 有効にする（`set_cloud_account` で `enabled: true`） | `on`。profile の `game-account.json` に鍵と game player を書く。Steam が終了の後に同期する |
| 開始済み・`off`・結び付きがゲームで作ったアカウント以外（受けたアカウント・まとめた先・後から足したアカウント） | 有効にする | 断る（`not_game_account`）。file を書かない |
| 開始済み・`on` | §4.2 のまとめの確定 | `off`。確定の 1 の前に `game-account.json` を消す（まとめた先は置かない）。消した後に止まっても、結び付きは A のままで、あとで有効にし直せる |
| 開始済み・`on` | 無効にする（解除。`enabled: false`） | `off`。`game-account.json` を消す（Steam が同期の後に Steam Cloud からも消す）。手元のアカウントと進行は残す |
| 開始済み・`other_account` | 有効にする・無効にする | 断る（`integration_required`）。file も手元の結び付きも変えない（まとめは §4.2） |
| `unreadable` | 有効にする・無効にする | 断る（`cloud_account_unreadable`）。file に触れない |
| 未開始・`restorable`・active がゲームで作ったアカウント | 「はじめる」（`start_game_player`） | `on`。file の鍵をアカウントとして足し、file の game player ID で結び付けて切り替え、`game-account-origin.json` に記録する。同意で作ったアカウントは使わずに残す |
| 未開始・`restorable`・active がゲームで作ったアカウント以外（「kukuriアカウントを使う」で受けた、`create_account` で足した、など） | 「はじめる」 | 利用者が選んだ active で §4.1 のとおり始める。file は戻さず、触れない（次は `other_account`） |
| 未開始・`unreadable` | 「はじめる」 | file を読まずに §4.1 のとおり始める（登録不要の経路）。file に触れない |
| どれでも | Steam の失敗・利用停止・ログインの取り消し | 手元の profile の鍵・アカウント・進行は消さない。file は SDK が消すまで残る |

- 再訪（同じ端末）は §4.1 のとおり手元の結び付きで再開し、file から戻さない。
- 同じ Steam 利用者が同じゲームを 2 つの端末で別々に始めると、後から有効にしようとする側は `other_account` で断る（先に保存した側の file が Steam から戻っているため）。Steam の同期の衝突（両方の端末で同時に書く）は、Steam の画面で利用者が選ぶ。
- 新しい端末で戻すと、同意で作ったアカウントが profile に 1 つ使われずに残る。作るのは profile ごとに同意の 1 回だけで、公開の記録は作らず、同じ profile での再開では増えない（INVAR-3）。
- ゲームの画面で有効にする前に、次のことを示す。
  - 同じ Steam アカウントでログインすれば、別の PC でもこのアカウントと進行の参照で再開できる。
  - Steam アカウントを使える人は、このゲームの kukuri のアカウントも使える。
  - 保存をやめると、別の PC での再開はできなくなる（この PC のアカウントと進行は残る）。すでに Steam に渡った写しは、やめても取り消せない。
- §4.2 のまとめの前の説明に、Steam Cloud への保存をやめることを足す。

##### 新しい端末での profile

Steam の Auto-Cloud は起動の前に file を戻すので、新しい端末では、profile の印（`.kukuri-profile.json`）が無いまま `game-account.json` だけが置かれた directory になる。`GameSdk` の profile は、印が無く `game-account.json` だけがある directory を、空の directory と同じく新しい profile として採用する（他の file があれば、今の判定のまま断る）。

##### 外部の主体の照合

- 照合するのは Steam で、SDK は Steam ID を受け取らず、保存もしない。file は Steam Cloud の利用者ごと・ゲーム（AppID）ごとの領域にあり、その Steam 利用者のログインで戻る。表示名やメールは使わない。
- `game-account.json` は application ID を持ち、別のゲームの file は戻さない（`unreadable`）。
- Steam の公開のプロフィールや kukuri の公開の記録に、両者の対応を書かない。

##### API・保存・互換

- 足す command:
  - `get_cloud_account`（application ID）: 上の「状態」の値を返す。
  - `set_cloud_account`（application ID・`enabled`）: 上の表のとおり書く・消す・断る。消すのは `on` の file だけ。
- `start_game_player` は、上の表のとおり `restorable` の file から戻す。
- 鍵の登録は、ClientHost に、秘密鍵を受けて登録簿へアカウントを足す公開の入口を足す。中身は移行で受けたアカウントの登録（`register_transferred_account`）と共用し、同じ公開鍵なら二重に足さない。必須 bundle は無く、空の DB のアカウントとして足す（鍵の保存は他のアカウントと同じく keyring か、keyring を使わない設定の file）。
- `game-account.json`: 版、application ID、game player ID、アカウントの秘密鍵。
  - 既定ではアカウントの鍵は OS の keyring にあり、file には置かない。有効にすると、平文の鍵の写しを profile の file に置く。書込みは temp からの rename で、Unix では持ち主だけが読める権限にする。Windows は継承した利用者の ACL に頼り、同じ OS 利用者の process は読める（§4 の「ゲームのコードは profile と同じ OS の権限で動く」と同じ）。
  - 応答・event・log に鍵を出さない。
- `game-account-origin.json`: ゲームで作ったアカウントの account ID（1 件）。Local Only。
- desktop・Web・Community Node・既存の DB・backup・wire は変えない。旧版の SDK は `game-account.json` を読まない。

##### 外部への送信の分類と、前の契約の置き換え

- `game-account.json` は、SDK にとっては Local Only の file である。有効にすると、ゲームの Auto-Cloud の設定によって Steam Cloud（Steam 利用者ごと・AppID ごと）へ複製される。送るのはゲームで、ゲームの作者が Steam の規約と利用者への説明を持つ（§5.1）。
- §4.1 の「`game-players.json` は network へは出さない」は、有効にしたときだけ、game player ID と鍵の写しが `game-account.json` として Steam Cloud へ出ることに置き換わる。`game-players.json` そのものは出さない。
- §4.2 の「結び付きを B の端末どうしの同期に載せない」判断は変えない。Steam Cloud に置くのは、ゲームで作ったアカウントの結び付きだけである。

##### 運用

- ゲームの作者は、Steamworks の Auto-Cloud に、SDK の profile の `game-account.json` の path と、利用者ごとの容量（file 1 件・数 KB）を設定する。費用は Steam Cloud のもので、kukuri 側の新しい服務・費用・中央の正本は無い。手順は導入の文書（`docs/runbooks/dev.md`）に書く。
- Steam で配らないゲームは、この保存を使わない。#1996 の登録不要の開始と、desktop・Web との移行だけになる。

##### 担当

- AC-2: `get_cloud_account`・`set_cloud_account`、ゲームで作ったアカウントの判定、§4.2 のまとめでの消去、Minimal sample の「Steam Cloud に保存」。
- AC-3: 新しい profile での採用と `start_game_player` での復元、解除の後に戻せないこと。

##### AC-2・AC-3 の固定の確認

実際の Steam での確認は判定から外したので、Steam Cloud の同期は `game-account.json` を写すことで模す。Close の記録には、実際の Steam で動くことを未確認と書く。

- AC-2:
  - 同意 →「はじめる」→ 有効にすると `on` になり、再起動しても同じ公開鍵と game player ID のまま。
  - ゲスト（保存なし）で始めた profile で後から有効にしても、game player ID が変わらない。
  - 「kukuriアカウントを使う」で受けたアカウントで始めた profile では、有効にできない（`not_game_account`）。
  - `other_account`（別の鍵、または同じ鍵で別の game player ID）の file がある profile では、有効にも無効にもできず、file と結び付きが変わらない。
  - §4.2 のまとめで `off` になる。
- AC-3:
  - 有効にした profile の `game-account.json` だけを、一度も開いていない directory へ写す。その directory を profile として開き、同意 → `restorable` →「はじめる」で同じ公開鍵と game player ID で再開する。鍵も token も他の file も写さない。
  - 戻した端末で無効にして有効にし直せる（戻したアカウントもゲームで作ったアカウント）。
  - 無効にすると file が消え、写す file が無い新しい profile では新しいアカウントで始まる。元の profile のアカウントと進行は残る。
  - 読めない file では、通常の開始に戻り、file に触れない。
  - SDK の試験と、Windows の実 Player の 2 つの profile で確かめる。

### 5. 4 本柱のデータ分類

SNS（§5.4）と Live（§5.5）は既存の分類を変えない。新しく分類するのは §5.1〜§5.3 の 3 つである。

#### 5.1 SDK の profile と要求の経路

- Feature 名: Game SDK の profile、要求・応答・event、bytes の受け渡し
- Durable / Transient: profile 内の account・製品データは Durable。handle・待ち行列・bytes・Sidecar の接続は Transient
- Canonical Source: profile directory の account registry と既存の各 feature の store。要求の形は `kukuri-cli` の登録簿
- Replicated?: 製品データだけが各 feature の既存の規則で複製される。handle と待ち行列は No
- Rebuildable From: 登録簿と runtime の event から作り直す
- Public Replica / Private Replica / Local Only: Local Only（製品データは各 feature の分類に従う）。ただし、Steam Cloud への保存を有効にした `game-account.json`（鍵の写しと game player）は、ゲームの Auto-Cloud が Steam Cloud（Steam 利用者ごと・AppID ごと）へ複製する外部送信で、送るのはゲームである（§4.3）
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
- Social の module（`Runtime/Social`、#1981 AC-2）の `KukuriBoard` は、1 つの topic（または private channel）の投稿を `list_timeline`・`list_thread` で 20 件ずつ読む uGUI の掲示板で、投稿・返信は `create_post` を使う。新着は `post_received` で知り、表示中の先頭の頁だけを読み直す（読んだ続きの頁は保つ）。1 回の読込みは件数に依存しない（履歴が増えても 20 件）。開いている間だけ topic を購読し、閉じる（無効にする・scene を変える）と、自分で始めた購読だけを外す（`get_sync_status` で開く前に購読していなかった topic）。投稿先と公開範囲を画面に示す。
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
  - build: `configure --target=x86_64-win64-vs17 --disable-vp9 --enable-realtime-only --as=nasm` の project を MSBuild で `WholeProgramOptimization=false` で build する（全体最適化の object は Rust の link と合わない）。1.17.0 の VS の project は `vp8/common/x86`・`vp8/encoder/x86` の asm と `vpx_ports` の `emms_mmx.asm`・`float_control_word.asm` を含めないので、これらは nasm で別に組み立てて同じ library に入れる。source は `chromium.googlesource.com/webm/libvpx` の tag `v1.17.0` から取得する。手順は `sdk/unity/build-libvpx.ps1`（#1982）で、Game SDK の feature `vp8` が `KUKURI_LIBVPX_DIR` の出力を link する。今の SDK の DLL は CRT を動的に link するので、libvpx も同じ（`--enable-static-msvcrt` を付けない）。§8.1 の静的な CRT へ移すときと、checksum を CI に置くのは #1984。
  - 圧縮の設定: realtime、CBR、`g_lag_in_frames = 0`、`VP8E_SET_CPUUSED = 10`、4 thread。
- 画質は既存の受信側の上限に合わせ、camera の上限（1280×720・30 fps・1.5 Mbps。ADR 0067 D8）で送る。既存の受信側はこれを超える frame と bitrate を捨てるためである。
- 取り込み: 配信する camera の RenderTexture を `AsyncGPUReadback`（RGBA32）で読み、callback で bytes を `kukuri_sdk_send_binary` へ渡す（SDK が copy する）。I420 への変換と圧縮は Rust の worker thread で行い、Unity の main thread で圧縮しない。圧縮前の送り待ちは 1 枚で、新しい frame が来たら古い frame を捨てる。
- 表示: 復号した I420 の 3 面を `live_video_frame` で渡し、Unity は R8 の texture 3 枚へ `SetPixelData` で上げ、shader で RGB にする。
- 音声: `AudioListener` と同じ GameObject の component の `OnAudioFilterRead` でゲームの出力音（`AudioSettings.outputSampleRate` の rate と channel 数の float）を受け、mic が on なら Unity の `Microphone` の音を足して、audio thread から `kukuri_sdk_send_binary` で渡す（speaker へ出す音は変えない）。Rust 側は mono にして、mic と同じく非公開の broadcast で 48 kHz の Opus に圧縮し、配信者の署名を付けて既存の音声の track へ写す。送り待ちは 200 ms 分までで、溢れたら古いものから捨てる。
  - mic の音は Unity が混ぜる（#1982）。moq-audio の mic の取り込みは取り込みから Opus の圧縮までを一続きに行い、取り込んだ sample を外へ出さないので、Rust 側で混ぜるには fork の変更が要る。Unity で混ぜれば変更が要らず、ゲームの音を mic の機器の echo cancellation（moq-audio が再生する音だけを消す）が消すことも無いので、消える機能は無い。この節の旧案（「Rust 側で既存の mic の取り込みと混ぜる口を作る」）は失効した。
  - SDK の配信は runtime の mic を開かない（`set_live_broadcast` の `microphone` は false）。マイクを off にしても（機器は閉じる。ADR 0067 §9）、ゲーム音は送り続ける。
- 視聴: 視聴する側の音声は、既存の Live と同じく Rust が OS の出力で鳴らす（feature `vp8` は desktop-runtime の `audio-devices` を含む）。`live_video_frame` の frame は、desktop・Web と同じく音声の再生時刻に 125 ms を足した時刻までで最も新しいもの。視聴が `watching` 以外（満員・映像なし・終了）で終わったら、desktop・Web と同じく 5 秒おいて視聴をやり直し、`stop_watching_live_media` まで続ける（配信者は最初の keyframe までは視聴を断る）。
- AV 同期の判定（2026-10-31 のユーザー判断）: 描く frame の時刻 − 音声の再生時刻が p95 で −200〜+125 ms に入る（遅れの側は、音声を鳴らす前に持つ 160 ms と 1 frame 分に余裕を足した値）。#1982 AC-1 は手元の 2 つの profile で 1280×720 を配信して frame time（§8.4 の閾値）とあわせて判定し、§8.4 の全条件での計測は #1984 で行う。
- 同じスレッドの複数の live: `KukuriLiveViewer` は、視聴を始めるときに読んだ開始 post への返信（`post_received`）とリアクション（`reaction_received`）だけを `ReplyReceived`・`ReactionReceived` で渡す。映像は session ごとに分かれる。返信とリアクションは開始 post に属したままで、live や別のスレッドへ移さない（#1982 AC-2）。
- codec の相互運用: desktop・Web の配信（WebCodecs の VP8）の出力を libvpx で復号する試験を、Chromium の WebCodecs で作った fixture で置く（`crates/game-sdk/tests/fixtures/`）。

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
