# ADR 0070: Unity 先行の kukuri Game SDK

## Status

Accepted（#1974 AC-1、2026-10-29）。§8 の対応環境・映像方式・性能プロファイルは #1974 AC-2 で確定する。各柱の実装と検証の完了は #1975〜#1984 の各 AC で判定する。

- [Discussion #1972](https://github.com/kukuri-app/kukuri/discussions/1972) は第一弾のエンジンを Godot 4.x としていた。2026-10-29 のユーザー判断「Unity 案でいく」により、第一弾を Unity に置き換える。Discussion は当時の検討として残し、以後の判断はこの ADR を正本にする。
- エンジンに依存しない Rust の共通部分と、エンジンごとの binding を分ける方針は Discussion から引き継ぐ。Godot・Unreal の binding は後続で扱う。

## Context

Discussion #1972 は、P2P のアセット配布、ロビーとゲーム通信、ゲームの内外をつなぐ SNS、ライブ配信と観戦の 4 本柱を一つの SDK で提供し、公式サンプル Cyber Disc Arena で連動を見せる構想である。組み込み方式は、本番を FFI の native plugin、開発中を Local Sidecar とし、エンジン側の API を共通にする。

基準 commit（main `1975a56c6`）には次がある。

- `kukuri-desktop-runtime` の `ClientHost` は UI adapter に依存せず、一つの active account の runtime と event の転送を所有する。Tauri の app と `kukuri-cli` の常駐プロセスが共用している。
- `kukuri-cli` は command 登録簿（`CommandRegistry`）、dispatcher、版つきの要求・応答（`RequestEnvelope` / `ResponseEnvelope`）、型付き error、secret frame、profile の所有（`ClientSession`）を持つ（ADR 0049）。登録簿と dispatcher は transport に依存しないが、常駐プロセスと socket の client は Linux の Unix socket だけである。
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
| runtime・account・同意・購読 | `kukuri-desktop-runtime` | `ClientHost`、`DesktopRuntime`、account 作成・暗号化 export/import、app 同意・CN 同意 | なし |
| command の定義・検証・実行 | `kukuri-cli` の `registry` / `dispatcher` / `protocol` / `session` | 登録簿・dispatcher・型付き error・secret frame・profile lock | 4 本柱で足りない command（§5）を同じ登録簿へ登録する |
| C ABI | 新規 `crates/game-sdk`（package `kukuri-game-sdk`、`cdylib` と `rlib`） | `kukuri-cli` の dispatcher を process 内で呼ぶ | handle・要求の受付・結果と event の待ち行列・buffer の所有と解放・panic の隔離 |
| Local Sidecar | `kukuri-cli` の常駐プロセス | Linux の常駐プロセスと同じ dispatcher・profile 所有 | Windows のローカル IPC（§8）と binary frame |
| ゲーム通信 | `kukuri-iroh-node` | media の受け口の認可（最初の stream で認可要求、5 秒ごとの再評価）、Direct → Relay Supported の接続順 | 専用 ALPN の session（datagram と stream） |
| アセット配布 | `kukuri-core` / `kukuri-app-api` / blob・docs・hint | docs の署名つき record、blob の取得・保持、公開 blob の発見（ADR 0063） | 発行者の署名つき manifest の型と検証 |
| Live の映像 | `kukuri-desktop-runtime` の live media | `push_live_video` / `next_live_video`、署名・転送・認可 | native の VP8 圧縮・復号（§8 で方式を決める） |
| Unity | 新規 `sdk/unity/com.kukuri.gamesdk/`（UPM）と `sdk/unity/CyberDiscArena/`（sample project） | — | C# API・module・Prefab・sample |

- SDK の dispatcher は CLI と同じ登録簿を使う。ただし `secret_output` の command（鍵の export、device backup 等）は SDK の要求として受け付けない。一般の API に秘密鍵や backup を出さないためである。
- 既存の command の意味・認可・同意の guard は変えない。SDK だけの第二の SNS protocol や Live protocol は作らない。
- 抽出で旧い処理が不要になる場合は、呼出元の切替と旧処理の削除を同じ子 Issue の PR に含める。

### 3. 共通の要求・結果・event

FFI と Sidecar は同じ要求・応答の形と同じ dispatcher を使う。遅延や copy の回数が同じであることは保証しない。

#### 要求と応答

- 要求は `RequestEnvelope`（`protocol_version`、`request_id`、`command`、`profile`、`payload`、`timeout_ms`）、応答は `ResponseEnvelope`（`ok`、`data` または `error`）をそのまま使う。
- `request_id` は呼出側が付ける。応答は同じ `request_id` で返る。同じ `request_id` の再送は別の要求として実行する（ADR 0049 の改訂と同じ。自動再送はしない）。
- 秘密の入力（import の passphrase と export 文字列）は CLI と同じ secret の経路で渡し、応答・event・log に出さない。

#### C ABI

| 関数 | 働き | 待ち |
| --- | --- | --- |
| `kukuri_sdk_abi_version()` | ABI の版を返す。C# は読み込み直後に照合し、違えば理由を示して開始しない | なし |
| `kukuri_sdk_open(config, out_handle)` | profile directory を指定して handle を作る。runtime の起動は背景で進み、完了を event で知らせる | なし |
| `kukuri_sdk_send(handle, request, secret)` | 要求を待ち行列に入れて即座に戻る。結果は poll で受ける | なし |
| `kukuri_sdk_send_binary(handle, kind, target, header, bytes)` | ゲームの packet・映像・音声の bytes を渡す（§5 の各柱） | なし |
| `kukuri_sdk_cancel(handle, request_id)` | 実行前の要求を取り消す | なし |
| `kukuri_sdk_poll(handle, max_items, max_bytes, out_batch)` | 届いている応答・event・bytes を上限まで取り出す。届いていなければ空で戻る | なし |
| `kukuri_sdk_buffer_release(handle, buffer)` | poll で受けた bytes を解放する | なし |
| `kukuri_sdk_close(handle)` | 購読・task・buffer・DB lock を解放して handle を閉じる | 停止の完了まで（上限 5 秒） |

- ABI の関数は同期的な状態 code を返す: `OK`、`INVALID_ARGUMENT`、`BACKPRESSURE`、`CLOSED`、`FAULTED`、`ABI_MISMATCH`。要求の成否は poll で受ける応答で返す。
- handle は opaque な pointer とし、Rust の内部型を ABI の外へ出さない。
- `kukuri_sdk_close` 以外は network の待ちをしない。Unity の main thread から呼んでも止まらない。
- すべての関数で panic を捕まえる。panic した handle は `FAULTED` になり、以後の関数は `FAULTED` を返し、未完了の要求は `internal_error` で終わる。Unity の process は落とさない。

#### 上限

| 対象 | 上限 | 超えたとき |
| --- | --- | --- |
| 未完了の要求 | handle ごとに 256 件 | `kukuri_sdk_send` が `BACKPRESSURE` を返し、要求は受け付けない |
| 要求 1 件の JSON | 1 MiB（CLI の frame と同じ） | `INVALID_ARGUMENT` |
| 応答の置き場 | 未完了の要求と同数を予約する | 溢れない。応答は捨てない |
| event と bytes の待ち行列 | 1024 件、合計 16 MiB（poll 済みで未解放の bytes を含む） | 古い event から捨て、捨てた件数を `events_lost` event で知らせる。受け手は状態を読み直す |
| 1 回の poll | 呼出側の指定（最大 256 件）。Unity の既定は §8 | 残りは次の poll へ |
| 要求の期限 | 既定 30 秒、指定できる上限 5 分（CLI と同じ） | `timeout` |

- media・blob の本体を JSON に入れない。アセットは検証済みの file の path で渡し（§5.3）、映像と packet は bytes で渡す。
- 映像の bytes は session ごとに最新の 1 枚だけを待たせ、新しい frame が来たら古い frame を捨てる。packet の上限は §5.2。

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

- handle が Ready になると、`events.watch` と同じ runtime の event（`RuntimeEvent`）を受ける。各柱が足す event は同じ流れに載せる。
- event は状態が変わったことの知らせであり、取りこぼしても表示と操作が成り立つようにする。受け手は必要なら command で今の状態を読み直す。

### 4. profile・アカウント・同意

- SDK の profile は kukuri desktop・Web とは別の directory に置く。Unity の既定は `Application.persistentDataPath/kukuri/profiles/<profile 名>`。kukuri app の install と、app の DB の共有を前提にしない。
- 一つの profile は一つの runtime だけが所有する（`ClientSession` の lock）。FFI と Sidecar が同じ profile を同時に開こうとすると、後の側は `profile_in_use` で断られる。
- 初回の流れは既存の入口をそのまま使う。
  1. `get_desktop_startup_status` が同意待ちを返す。同意前の network I/O は 0 件（`ClientHost::start_if_consented_with`）。
  2. ゲームが利用規約・プライバシーポリシー（`get_app_consent_status` の文書）と 18 歳以上の自己申告を表示し、`accept_app_consents` で同意する。
  3. 同意すると account が作られ、ゲームだけで使い始められる。
  4. 既存の kukuri のアカウントを使う人は、kukuri app で暗号化 export した文字列と passphrase を明示的に入力し、`preview_account_key_import` と `import_account_key` で取り込む。SNS と同じ公開鍵になる。誤った passphrase では既存の profile を変えない。
  5. Community Node を使う機能は、既存の CN 同意（`fetch_community_node_policies` / `accept_community_node_consents`）を経てから接続する。
- 取り込み画面は、取り込んだアカウントでこのゲームが投稿・署名できることを示す。ゲームのコードは profile と同じ OS の権限で動くためである。
- 秘密の値を Inspector の serializable field、scene・prefab、PlayerPrefs、診断の出力に保存しない。

### 5. 4 本柱のデータ分類

SNS と Live は既存の分類を変えない。新しく分類するのは次の 5 つである。

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
- 公開範囲は room を置く場所で決まる。公開 topic なら public、private channel なら既存の audience（friend_only・friend_plus・invite_only）をそのまま使う。
- session は専用の ALPN で host の端末へ接続し、最初の双方向 stream で、参加者が署名した 10 秒の proof（room と host に束縛）を送る。host は room の状態と公開範囲を確かめてから packet を受け付け、5 秒ごとに評価し直す（media と同じ形）。
- 座標などの最新の状態は QUIC の unreliable datagram、確定させる操作は reliable stream で送る。datagram の payload は経路の上限（iroh の `max_datagram_size`）を超えて送らず、送り待ちは session ごとに 64 件（古いものから捨てる）、stream の送り待ちは 256 KiB（超えたら `backpressure`）とする。数値の最終値は #1979 の計測で確かめる。
- 接続は Direct P2P → Relay Supported P2P の順で試し、実データが relay を通るときは Relay Fallback として区別して記録する。central な lobby の authority は置かない。

#### 5.3 署名つきアセット配布

- Feature 名: ゲームの公式アセット（スキン等）の発行と P2P 配布
- Durable / Transient: Durable（release 単位）
- Canonical Source: 発行者の鍵で署名した manifest（application ID、asset ID、release revision、blob hash、長さ、形式）を docs の record に置き、本体は blob
- Replicated?: Yes。ゲームの公開 topic の replica と、blob の保持 peer
- Rebuildable From: docs の署名つき manifest と blob
- Public Replica / Private Replica / Local Only: Public replica。適用済みの release の記録と検証済みの file は Local Only
- Gossip Hint 必要有無: 更新の知らせに使う。hint だけでは適用しない
- Blob 必要有無: Yes（glTF・texture）
- SQLite projection 必要有無: 適用済みの release の記録を持つ（application ID と asset ID ごとに 1 行）
- 必須 contract: 発行者の公開鍵で検証できる manifest と、hash が一致する blob だけを適用する。別の鍵・改変・古い revision を拒否する。提供者がいないときは取得できないことを結果で返す
- 必須 scenario: 発行者・保持 peer・新しく取得する peer の 3 役で公開 → 再提供 → 取得 → 再起動なしの差し替え（#1980）

この柱で決めたことは次のとおり。

- 信頼する発行者の公開鍵は Unity の設定 asset に固定する。秘密鍵はゲームと UPM package に含めず、発行は開発者の Sidecar（CLI）からだけ行う。
- 検証した本体は profile の cache directory に file として書き、その path を Unity に渡す。bytes を待ち行列に積まない。
- 署名は発行元と内容が一致することを確かめるもので、アセットが無害であることの保証ではない。

#### 5.4 SNS

既存の投稿・返信・リアクション・カスタムリアクション・公開 channel・private channel の分類と command をそのまま使う。SDK は投稿の形式を足さない。表示は既存の cursor・有限の取得で行い、全履歴の同期を前提にしない（#1981）。

#### 5.5 Live

既存の Live（ADR 0005）と media の搬送（ADR 0067）の分類をそのまま使う。追加は Unity の画面と音声を既存の VP8 の chunk にする処理と、受けた chunk を Unity の texture にする処理だけで、新しいデータの種類は無い。映像の bytes は Transient で保存しない（#1982）。

### 6. Cyber Disc Arena の裁定

- room の host（作成者）の端末が、ディスクの所有・キャッチ・被弾・得点を一つに確定する。host の移譲はしない。host が抜けたら room は終わる。
- 参加者は入力を datagram で送り、キャッチと投げる操作の要求を stream で送る。host は状態を datagram で配り、確定した event を通し番号つきの stream で配る。
- 参加者は自分の動きを予測して表示し、host の状態に合わせて補正する。ディスクの所有は確定 event でだけ変わる。発射ベクトルの共有だけで端末間の物理が一致するとは仮定しない。
- 予測・補間・補正の実装と裁定の規則は #1983 が持つ。packet の配送は #1979 が持つ。

### 7. Unity 側の構成

- module ごとに asmdef を分ける: Core（必須）、Lobby、Realtime、Netcode（Unity Netcode for GameObjects に依存）、Assets（glTFast に依存）、Social、Live。使わない module の依存を持ち込まない。
- C# の結果（Task）と event の callback は Unity の main thread で、poll の中から呼ぶ。Rust の worker thread から GameObject や UI を触らない。
- 1 frame で poll に使う件数と時間には上限を置く（既定値は §8）。上限を超えた分は次の frame へ回す。
- mode（FFI / Sidecar）と profile 名は設定 asset で明示する。本番の Player の既定は FFI で、外部の daemon を自動で起動しない。
- 未使用の SNS の UI や Live の取り込みは、明示的に開始するまで動かない。

### 8. 対応環境・映像方式・性能プロファイル

#1974 AC-2 で、次を確定してこの節に書く。確定までは後続の実装の受入基準に使わない。

- Unity・URP・Netcode for GameObjects・glTFast の版、Mono（Editor）と IL2CPP（Player）の build 条件。
- Windows の Sidecar のローカル IPC の方式。
- Unity の映像の codec の実装（VP8 の圧縮・復号）と texture の受け渡し方式、Live の音声の取り込み元。
- リファレンス PC と解像度、同時負荷（対戦 2 人・視聴 5 人・アセット更新 1 件）、network の条件、測定時間と反復回数、合否の閾値。
- poll の 1 frame あたりの件数・時間の既定値。

### 9. 子 Issue の担当

| 子 Issue | 担当するもの | 担当しないもの |
| --- | --- | --- |
| #1975 | `crates/game-sdk` の C ABI・待ち行列・buffer・panic の隔離、profile と account・同意の接続 | Sidecar、Unity、各柱の機能 |
| #1976 | Windows の Sidecar と IPC、attach・detach、複数 profile | C ABI、Unity |
| #1977 | UPM・C# API・mode の切替・Unity の lifecycle | 各柱の UI と挙動 |
| #1978 | action session の room、発見、公開範囲、参加 session | packet の配送、裁定 |
| #1979 | session の datagram と stream、上限、NGO の Transport adapter | 裁定、予測と補正 |
| #1980 | 発行・発見・取得・保持・検証、Unity のスキンの読み込み | 課金・DRM |
| #1981 | SNS の command の利用、掲示板 Prefab、差分の表示 | Live の映像、3D の演出 |
| #1982 | Unity の映像・音声 adapter、既存 Live との相互配信、転送 | 3D の演出 |
| #1983 | Arena の操作・裁定・同期、4 本柱の連動、4 Prefab | 各柱の SDK 機能 |
| #1984 | FFI と Sidecar の適合、E2E、SLO の計測、CI、配布物、日本語の導入手順 | 各柱の単体試験 |

### 10. 適合 fixture

後続の実装は次の fixture を試験にする。FFI と Sidecar の両方で同じ結果になることを #1976 と #1984 で確かめる。ID は試験名と PR の説明で参照する。

| ID | 対象 | 事前状態と操作 | 期待する結果 | 担当 |
| --- | --- | --- | --- | --- |
| F-ABI-1 | ABI の版 | C# の期待する版と DLL の版が違う | `ABI_MISMATCH`。handle を作らず、network I/O 0 件 | #1975・#1977 |
| F-ABI-2 | 起動 | 同意済みの profile で open | Ready の event。その後の要求が実行される | #1975 |
| F-ABI-3 | 相関 | 2 件の要求を続けて送る | それぞれの `request_id` で応答が返る | #1975 |
| F-ABI-4 | 満杯 | 未完了の要求が 256 件ある | 257 件目の send が `BACKPRESSURE`。既存の要求は失われない | #1975 |
| F-ABI-5 | 取消 | 実行前の要求を cancel | `cancelled` | #1975 |
| F-ABI-6 | 終了 | close の後に send | `CLOSED` | #1975 |
| F-ABI-7 | 終了中の書き込み | 書き込みの実行中に close | 結果か `operation_outcome_unknown`。知らせずに消えない | #1975 |
| F-ABI-8 | panic | handler が panic する | handle が `FAULTED`、未完了は `internal_error`、process は続く | #1975 |
| F-ABI-9 | event の溢れ | poll しないまま event が 1024 件を超える | 古い event を捨て、`events_lost` で件数が届く。応答は失われない | #1975 |
| F-ABI-10 | poll | 届いたものが無い | 空で即座に戻る | #1975 |
| F-ABI-11 | secret | `secret_output` の command を送る | 拒否。秘密の値は応答・event・log に出ない | #1975 |
| F-PRO-1 | 同意前 | 新しい profile で open | 同意待ち。network I/O 0 件 | #1975 |
| F-PRO-2 | 再起動 | 作成 → close → open | 同じ公開鍵 | #1975 |
| F-PRO-3 | import | 正しい export と passphrase | SNS と同じ公開鍵になる | #1975 |
| F-PRO-4 | 誤った passphrase | import | 失敗し、既存の profile は変わらない | #1975 |
| F-PRO-5 | 二重所有 | FFI が開いている profile を Sidecar でも開く | `profile_in_use` | #1975・#1976 |
| F-IPC-1 | 接続の検証 | 版が違う client、別 user の client | handler の前に拒否。公開 TCP の listen 0 件 | #1976 |
| F-IPC-2 | 切断 | 書き込みの実行中に client が切れる | `operation_outcome_unknown`。自動の再実行 0 件 | #1976 |
| F-IPC-3 | attach の反復 | detach と attach を繰り返す | 同じ endpoint id。observer と task が増えない | #1976 |
| F-LOB-1 | 作成と発見 | profile A が room を作る | profile B の一覧に出る。late join でも出る | #1978 |
| F-LOB-2 | 終了 | host が room を終える | 一覧で終了と分かり、参加できない | #1978 |
| F-LOB-3 | 一覧の量 | 無関係な room と履歴を増やす | 1 回の一覧の取得量が増えない | #1978 |
| F-SES-1 | 公開範囲 | 4 種類の公開範囲で、条件を満たす人と満たさない人が参加する | 満たす人だけが参加でき、満たさない人は理由の code だけを受ける | #1978 |
| F-SES-2 | 退出後 | 退出・room の終了・権限の失効の後に packet を送る | 相手に届かない | #1978・#1979 |
| F-SES-3 | 配送 | 固定の遅延・損失・順不同の下で datagram と stream を送る | datagram は欠けても新しいものが届き、stream は順に全部届く | #1979 |
| F-SES-4 | 大きすぎる packet | datagram の上限を超える payload を送る | 送らずに `validation_failed` | #1979 |
| F-AST-1 | 正しい release | 発行 → 取得 | 検証済みの path が届き、適用できる | #1980 |
| F-AST-2 | 不正 | 別の鍵・改変した blob・古い revision | 適用しない。今のスキンを保つ | #1980 |
| F-AST-3 | 提供者不在 | 保持 peer が誰もいない | 取得できないことを返す。成功と扱わない | #1980 |
| F-SNS-1 | 相互運用 | Unity で投稿・返信・リアクション、desktop で返信 | 同じ ID で両方に表示される | #1981 |
| F-SNS-2 | 公開範囲と block | private の非参加者・block した作者 | 既存の規則どおり読めない・表示しない | #1981 |
| F-LIV-1 | 明示開始 | 配信を始める操作の前 | 取り込みと送信 0 件 | #1982 |
| F-LIV-2 | 相互配信 | Unity → desktop・Web、desktop・Web → Unity | 映像と音声が届く | #1982 |
| F-LIV-3 | 転送 | 配信者 1・視聴者 5 | 直接の枠を超えた視聴者は転送で届く | #1982 |
| F-LIV-4 | 停止 | 配信・視聴を止める | 取り込み・codec・texture・転送を解放する | #1982 |

## Consequences

- SDK は CLI と同じ登録簿を使うため、command を足すと desktop の CLI と SDK の両方で使える。登録簿の変更は両方の試験で確かめる。
- Windows の常駐プロセスは CLI 全体の Windows 移植ではなく、SDK の開発用 Sidecar として必要な範囲に限る。
- 4 本柱の正本は既存の docs・blob・hint・投稿・Live のままで、SDK だけのデータの正本は作らない。新しく分類したのは action session の room、session の packet、アセットの manifest の 3 つである。
- Unity の版・映像方式・閾値は §8 が確定するまで未定であり、未計測のものを達成とは扱わない。

## 参照

- ADR 0002（feature の分類の雛形）、ADR 0005（Live session）、ADR 0006（Game Room）、ADR 0049（CLI のローカル制御経路）、ADR 0063（公開 blob の発見）、ADR 0067（media の搬送）、ADR 0068（公開 channel）
- Issue #1973（親）、#1974〜#1984（子）
