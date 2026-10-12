# ADR 0067: metaverse の空間音声と live 配信のメディア搬送

## Status

Accepted（#1031、2026-10-13 のユーザー判断 D1〜D4）。実装と検証の完了は #1033（metaverse の空間音声）と #1034（live の映像・音声）の各 AC で判定する。#1033 の実装で §1・§3・§4・§7・§9 の実装の形を補った（判断は変えていない）。

- D1: live の映像・音声は、配信者が数人へ直接送り、受け取った視聴者が次の視聴者へ転送する（§8）。
- D2: 音声の取り込み・圧縮・再生は Rust 側で行う（§9）。
- D3: builder preview の完了条件に #1033・#1034 を含めない（§10）。
- D4: metaverse の話者 1 人が同時に音声を送る相手は 64 人まで、聞き手は近い順に 16 人の話者を聞く（§4）。

観測の基準は main `e6177415f4b2c93ab5de5f009dae16769b8d90b9`、外部依存と実行基盤の確認日は 2026-10-13（実機での確認は #1033・#1034 で行う）。

## Context

### 現在の実装

- metaverse の空間音声（ADR 0043、#795）は webview の `getUserMedia` と `ScriptProcessorNode` で 16 kHz・PCM16 を取り、数値配列の JSON を署名つき `MetaverseRoomEventV1::SpatialAudioFrame` に入れる。app-api は access 判定と送信量の確認の後、topic（channel なら channel）の gossip へ `GossipHint::MetaverseRoomEvent` として流す。その topic を購読する全端末へ、Dome に居なくても届く。#1567 の hint 受付の上限は MetaverseRoomEvent を対象外にしている。
- 受信は room ごと 512 件の memory buffer に入り、画面が current と ready な隣接 Dome（最大 5 room）を 180 ms ごとに取り出す。届いた順に `AudioBufferSourceNode.start()` で即時に再生し、timestamp による再生時刻の調整、jitter buffer、同時に再生する stream の上限（ADR 0041 の 16）の適用は無い。
- 距離減衰の関数は TypeScript（`SpatialAudioModel.ts`）と Rust（`metaverse_audio.rs`）の両方にあり、Rust 側は test だけが使う。隣接 Dome の話者の位置は、音声 frame に載る話者自身の申告だけが聞き手に届く。
- live session（ADR 0005）は署名つきの状態・manifest、開始・終了の hint、視聴者の presence を持つが、映像・音声の取り込み・配信・再生は無い。
- Dome の host への P2P 経路（`/kukuri/dome-session/1`、ADR 0038）は、relay URL を含まない直接の候補を先に、relay URL を含む候補を後に試す。
- Linux の webview（WebKitGTK）は、permission の既定の処理が mic・camera を拒否する。kukuri は Tauri の `on_permission_request` を使っていないため、現行の mic の取得は Linux では拒否されるとみられる（実機未確認）。
- Web 版（ADR 0060）と Android の初回（#1193 D1）は live・game・metaverse・Dome を対象外にしている。

### 現状の問題

- 音声の量が、Dome の参加者ではなく topic の購読者全員と、topic 内の全 Dome の発話量に比例する。PCM16 の JSON は 1 話者あたり数百 kbps になる。
- 受信は 180 ms ごとの取り出しと即時再生のため、到着の揺れがそのまま途切れになり、同時話者の上限も効いていない。
- live に映像・音声の経路が無い。

### 外部依存（2026-10-13 確認）

- `moq-net`（旧名 moq-lite。0.3.11）は moq-lite protocol の実装で、broadcast・track・group・frame、購読ごとの優先度と最大遅延（古い group を捨てる）、session ごとの公開範囲（origin）を持つ。task を起こさず時計も読まない。2026-05 の初公開から 0.3 まで minor が上がり、直近 90 日で 36 回公開された。
- `hang`（0.21.12）は catalog と frame の容器（timestamp つき）の形式で、Opus・H.264・VP8/VP9 等を記述し、data track（JSON 等）も持てる。取り込み・圧縮は持たない。
- `moq-audio`（0.1.11）は cpal の取り込み・再生、純 Rust の Opus（`unsafe-libopus`）、再生の mixer（stream ごとの音量、最大 64 stream）、WebRTC の AEC3・雑音抑制・AGC2 の純 Rust 移植（`sonora`）による echo cancellation を持つ。Linux は build 時に libasound が要る。
- `iroh-moq`（n0-computer/iroh-live の 1 crate、0.1.0）は crates.io に無い。node 全体で 1 つの公開範囲を全 session に見せ、peer ごとの認可が無い。API の全面改修（PR #75）が未 merge で、iroh-live は Windows で一度も実行されていない。
- `web-transport-iroh` 0.7.0 が iroh の接続を moq-net の transport へ合わせる。公開版の moq-net 0.3.x は web-transport-trait 0.4 系なので、0.8 系とは組み合わせられない。
- iroh の要求はどれも 1.x で、kukuri の iroh 1.3 fork（`eef6af69`、#1032）で満たせる見込み（実 build は #1033 で行う）。
- moq の上流は、インターネット越しの fan-out を MoQ relay（group の cache、重複購読の統合、JWT 等の認可）で行う設計で、iroh の relay は fan-out をしない。

### 実行基盤（2026-10-13 確認。実機は未確認）

- WebKitGTK（Ubuntu の更新版は 22.04 が 2.50.4、24.04 が 2.52.6）: `getUserMedia` は build に含まれるが、permission の既定は拒否で、Allow する処理が要る。capture の処理経路に echo cancellation が無い（source の読解）。WebCodecs は 2.44 以降既定で有効で、Opus・VP8/VP9 は既定で入る GStreamer の package で使え、H.264 は universe の package が要る見込み。
- WebView2: `getUserMedia`・AudioWorklet・WebCodecs（software の Opus）が使える。許可は WebView2 の既定の確認画面で、利用者の判断が profile に保存される。Chromium の echo cancellation は自分が再生した音を参照に使う（WebView2 で既定かは未確認）。
- 取り込みと再生を webview と Rust に分けると、どちらの echo cancellation も相手の声の参照を持てない。成り立つのは、両方を Rust で行うか、両方を webview で行うかである。
- Rust: `cpal` 0.18（Linux は ALSA が既定、PipeWire・PulseAudio の host は feature。PipeWire host は 0.3.53 以上で、Ubuntu 22.04 の 0.3.48 では使えない。PipeWire・PulseAudio 周りに未修正の不具合がある）、純 Rust の Opus、`sonora`（Windows も CI 対象）。`moq-audio` はこの 3 つを組み合わせている。
- Tauri の IPC: `invoke` は custom protocol の POST で bytes をそのまま運ぶ。`Channel` は bytes の message を 1 KiB 未満なら `eval`、以上なら `eval` と fetch の往復で渡す。Linux・Windows の実測は無い。
- 映像の描画は webview で行う必要がある（decode 後の画像を IPC で毎 frame 運ぶと 720p で 1 枚約 3.7 MB になる）。OpenH264 を source から build して配る場合、H.264 の特許の実施料は配る側の責任になる。

## Decision

### 1. 部品・ALPN・時刻（固定項目 1）

- protocol は moq-lite（`moq-net`）、形式は `hang` を使う。`iroh-moq` crate は使わない。理由は、peer ごとの認可が無いこと、crates.io に無く API の全面改修が未 merge であること、Windows で実行されていないこと。同じ働き（iroh の接続を moq-net の session にする部分）は kukuri の受け口が持つ。
- 専用の ALPN `/kukuri/media/1` を既存の `Router` に 1 つ登録する（moq の 15 個の ALPN は登録しない）。接続の上で使う moq-lite の版は kukuri の版ごとに固定し（`moq-net` の版の明示）、版の違う kukuri の間では media が届かない。旧版との互換は設けない。
- iroh の接続は `web-transport-iroh` の raw session を、`moq-tokio` の `transport::Session`（moq-net が要求する poll 型への変換。web-transport-iroh は async 型だけを実装する）で moq-net の transport へ渡す。kukuri の ALPN では moq-net が版を選べないため、session は moq-net の moq-lite の版の名前を protocol として返す。認可要求は moq-lite の handshake の前に、接続の最初の双方向 stream で送る（moq-lite の SETUP は token を運ばない）。受け口は相手ごとに moq-net の session を作り、その session の公開範囲（origin）には、認可した対象の broadcast だけを置く（§3）。
- 置き場所: 受け口・接続・上限は `kukuri-iroh-node`、認可要求と descriptor の型は `kukuri-core`、access 判定は既存の `kukuri-app-api`、機器と codec は `kukuri-desktop-runtime`（native だけ）。新しい crate・trait は作らない。
- broadcast は送る端末ごと・対象ごとに 1 つ。metaverse は話者の音声 track（hang の catalog つき）と、話者の位置の track（JSON の frame、最新の group だけを読む）。live は配信の映像 track と音声 track。
- 時刻: 送る側は取り込み時刻（単調時計、μs）を `hang` の frame の timestamp に入れる。受ける側は話者ごと（metaverse）、または broadcast ごと（live。音声の再生時刻を基準に映像を合わせる）に再生時刻を決める。
- 揺れの吸収: 受ける側は再生前に保持する量を話者ごとに既定 50 ms、上限は ADR 0041 client の jitter frame の数 × 20 ms（既定 8 で 160 ms）とする。順序の入れ替わりは moq-net の group の順と timestamp で並べ直し、上限より遅れた group は古いものから飛ばす。それでも再生前の保持が上限を超える分は書かずに捨てる。

### 2. データの扱い（固定項目 2）

- 音声・映像の bytes は Transient とし、docs・blob・SQLite に保存しない。保持は再生前の保持（jitter frame × 20 ms、既定 160 ms）と、送信・転送待ち（購読の最大遅延と、live は最新 1 group）と、moq-net の cache（読み書きの無い group を 2 秒で外し、判定の間隔を含めて長くても約 4 秒。各 track の最新の group は残る）だけで、いずれも 10 秒以下。
- gossip は既存の小さな hint（presence、`LiveSignal`、`SessionChanged`）に限る。音声を送っているかは presence に載せる（#1033）。
- log・metrics に音声・映像の bytes、話者・視聴者の識別子、block の一覧を出さない。出してよいのは拒否の code と件数、接続経路の分類（Direct P2P / Relay Supported P2P / Relay Fallback）。

### 3. 認可・停止（固定項目 3）

- 送る側（live では転送する視聴者も）が受け口で相手の認可要求を検証してから、その session の公開範囲に対象の broadcast を置く。拒否は理由の code だけを返して接続を閉じ、音声・映像・他の参加者の識別子を返さない。
  - metaverse: 要求は参加者が署名した `DomeSpatialAccessProofV1`（Spatial Context と話者の Dome の owner に束縛、10 秒）と、対象の Dome（instance・generation・session）。送る側は署名と期限を確かめ、自分の状態で `evaluate_dome_room_access`（owner から visitor への block、公開 topic は gossip を止めていないこと、private は current epoch の active な参加者）を評価し、自分がその Dome の session に参加中であることを確かめる。
  - live: 要求は視聴者が署名した、live session と scope に束縛した 10 秒の proof。送る側は session が終了していないこと（検証済みの state）と、private なら current epoch の active な参加者であることを確かめる。公開 live の事前の topic 購読 lease を条件にしない。
- 継続: 送る側は 5 秒ごとに、接続の初めに proof で確かめた相手の公開鍵について、自分の状態（block、公開 topic の gossip、private の参加者）で access を評価し直し、自分がまだ同じ Dome の session に居ることを確かめる。拒否なら session を閉じる（ADR 0045 の KeepAlive と同じ間隔）。送る側は Spatial Context の状態を持つ参加者なので、proof を送り直させない。moq-lite では announce を取り消しても進行中の購読は止まらないため、止めるときは session を閉じる。
- mute・block した author は購読しない（decode・再生の前に捨てる）。topology・参加・presence は変えない。
- mic・camera・screen は利用者の操作の後、機器を開けた（OS の許可を含む）場合だけ送る。開けなければ送信は 0 で、開始を成功と表示しない。`moq-audio` は購読者がいる間だけ機器を開くため、mic が on の間は自分の端末も購読して機器を開いたままにし、最初の音が届いてから on と示す。再起動後は自動で開始しない。Windows で OS の設定が mic を拒否しているときの見え方は #1033 が実機で確かめ、送信 0 と表示を合わせる。
- 停止: 退出、Dome・session・generation の変更、access の失効、host の offline grace（ADR 0045）、live の終了・退出、account の切替、shutdown で、取り込み・送信・転送・購読・再生を止め、接続と保持中の media を捨てる。古い世代の遅れた結果は適用しない。
- 既存の owner との接続: media の需要は ADR 0055 §1 の「参加中の live/game/Dome」に属し、参加の終了で止まる。media の接続・購読・接続の試行は §4 の上限で数え、docs・blob の取得の受付（remote 取得 8 等）とは別に持つ。再生時刻は通信の再試行を決めない。接続の試行は需要（聞く話者の選択・視聴）がある間だけ、相手ごとに 5 秒に 1 回まで行う。

### 4. 上限（固定項目 4、D4）

| 対象 | 単位 | 上限 | 超えたとき | 回収・停止 |
| --- | --- | --- | --- | --- |
| 聞く話者（metaverse） | 聞き手の端末 | 16（ADR 0041 client の同時 stream） | 音声を送っている話者のうち、既存の距離関数で近い順に 16 人だけ購読する | 退出・順位の外・mute で購読を解除 |
| 話者が送る相手（metaverse） | 話者の端末 | 64（固定） | 新しい購読を `busy` で拒否する。その聞き手は空きができるまで聞こえない | 相手の退出・失効・購読の解除・30 秒無通信で解除 |
| 音声の送信量 | 話者ごと | 50 frame/s・32 KiB/s（ADR 0041 player） | 受ける側が捨てて拒否の件数に数える | 1 秒分の枠を経過時間に比例して補充する（正規の 20 ms の frame の到着の揺れとまとめ到着は捨てない） |
| 再生前の保持 | 話者ごと | 既定 50 ms、上限は client の jitter frame × 20 ms（既定 160 ms） | 上限より遅れた group は古いものから飛ばし、保持が上限を超える分は書かずに捨てる | 再生と同時 |
| 配信者が直接送る視聴者（live） | 配信者の端末 | 4（#1034 AC-1 が bitrate の計測で見直す） | 直接の子の一覧を返し、転送の参加へ回す（§8） | 子の退出・失効・30 秒無通信で解除 |
| 視聴者が転送する相手（live） | 視聴者の端末 | 2（同上） | 自分の子の一覧を返す | 同上 |
| 転送の参加の問い合わせ（live） | 1 回の参加 | 16 段、1 段 5 秒 | 参加できないと表示する | 参加の終了で破棄 |
| 視聴者の親（live） | 視聴者の端末 | 1 | — | 張り直しで置き換え |
| 接続の試行の記録 | 端末 | 64 件・30 秒 | 試行しない | 期限で削除 |
| catalog | broadcast | 1 つ、4 KiB | 受けない | broadcast の終了で削除 |
| live の track | broadcast | 映像 1・音声 1 | 受けない | 配信の終了で削除 |
| live の group の保持 | track・転送する端末 | 最新 1 group | 古い group を捨てる | 配信の終了で削除 |
| live の映像の codec・画質・bitrate・group の長さ・音声と映像のずれ | — | #1034 AC-1 で決める | — | — |

1 人に送るごとに上り約 50 kbps（Opus 24 kbps と 20 ms ごとの packet の overhead）で、話者の上りは最大約 3.5 Mbps。既定の定員（64 人）の Dome では全員に届く。上限 64 は budget の JSON の項目にしない（既存の完全 JSON 設定の形を変えない）。

### 5. 経路（固定項目 5）

- metaverse の音声と live の転送の各区間は、送る端末と受ける端末を直接つなぐ。relay URL を含まない候補（Direct P2P）を先に、relay URL を含む候補（Relay Supported P2P）を後に試し、実データが relay を通る場合は Relay Fallback として扱う。Direct の候補に relay URL を混ぜない。
- アプリの media の中継（Community Node 等の中継 server）は置かない。`cn-iroh-relay` は純粋な iroh relay のままとし、media の中継の責務を持たせない。

### 6. 互換と撤去（固定項目 6）

- metaverse は実験機能のため、旧 PCM の wire（`MetaverseRoomEventV1::SpatialAudioFrame`、`spatial_audio_frame`）と、PCM16 を前提にした budget の意味との互換を設けない。#1033 が新しい経路への切替と同時に削除する。
- timeline の wire・保存・取得は変えない。
- #1034 は #1033 が実装した受け口・認可・停止・時刻・音声の部分を使い、録画・VOD・複数配信者・adaptive rendition を足さない。

### 7. 依存（固定項目 7）

- #1032 の構成（iroh・iroh-base・iroh-dns・iroh-relay の fork `eef6af69364b5df1aa33c30e4c5731aad9d32046`、iroh-docs `e7233d14853cb4db9966e30050bac1e689cdeec8`、iroh-gossip の fork `23cd6cc76d51582b3678b3b743db72d51ce296d4`）を変えない。
- 追加（#1033 で両方の workspace の `Cargo.lock` に固定）: `moq-net` 0.3.11・`hang` 0.21.12・`moq-mux` 0.10.12・`web-transport-iroh` 0.7.0（既定の feature を外し `ring`。web-transport-trait 0.4 系）・`moq-tokio` 0.19.24（既定の feature を外し `iroh`。増える crate は 8 個）・`moq-audio` 0.1.11（既定の feature を外す。機器の `capture`・`playback`・`aec` は desktop-runtime の `audio-devices` で Tauri の app だけが有効にし、CLI・harness は codec だけを使う）。映像の crate は #1034 AC-1 で決める。
- Linux の build は libasound の開発 package、配布物は libasound を要する。
- aws-lc は既に desktop の build に入っている（str0m 経由）ため、web-transport-iroh の既定の TLS で新しい C の build は増えない。
- 輻輳制御と QUIC の stream 数の上限は endpoint の既定のままとし、変える必要は #1033・#1034 の計測で示す。
- moq 側の版を上げるときは、moq-lite の版（§1）と ALPN の版を同時に見直す。

### 8. live の届け方（D1）

配信者は直接 4 人まで送り、受け取った視聴者が次の視聴者へ 2 人まで転送する。Community Node 等の中継 server は使わない。

- 参加: 視聴者は live manifest の descriptor が示す配信者の端末へ認可要求を送る。空きがあれば配信者の子になる。空きが無ければ、配信者は直接の子の一覧（endpoint id）と、自分までの経路（祖先の列）を返す。視聴者は一覧から無作為に 1 人を選んで同じ問い合わせを繰り返す（§4 の 16 段まで）。自分が祖先の列に含まれる候補には付かない（循環の防止）。
- 転送: 子の session の公開範囲には、親から受けている broadcast を置く。途中参加は最新の group から始まり、全 group を送らない。
- 張り直し: 親との接続が切れたか、5 秒間 group が届かなければ、配信者から参加し直す（視聴している間だけ、5 秒に 1 回まで）。自分の子は付いたまま移る。
- 改ざん防止: 配信者は配信ごとの署名鍵を作り、その公開鍵を owner が署名した live manifest の descriptor に載せる。frame（または group）に配信者の署名を付け、視聴者は検証できないものを捨てて、その親から離れて参加し直す。転送する視聴者は中身を変えずに渡す。
- 段ごとの資格確認: 転送する視聴者も、子の認可要求を配信者と同じ規則（§3）で確かめる。
- 利用者への影響: 転送する視聴者の上り回線を、子の数 × 配信の bitrate だけ使う。段が深いほど遅延が増え、途中の視聴者が抜けると下流は張り直しまで止まる。視聴者は親と子の視聴者に endpoint と IP address が見える（既存の P2P の外部送信の範囲）。

### 9. 処理の場所（D2）

- 音声は Rust 側（`moq-audio`）で取り込み・圧縮（Opus、20 ms）・decode・mix・再生を行い、echo cancellation（`sonora`）は再生中の音を参照に使う。画面側の音声の取り込み・再生（`getUserMedia`・WebAudio）は削除する。live の音声も同じ経路を使う。
- metaverse の距離減衰: 聞き手の端末の Rust が、話者の位置の track と、画面から受ける聞き手の位置・ready な隣接 Dome の opening の位置から既存の距離関数で gain を求め、話者ごとの音量に反映する。gain の関数は Rust の 1 か所に残し、TypeScript の gain の関数は #1033 で削除した（画面は話者を近い順に並べるための距離と opening の位置だけを持つ）。
- 映像の取り込み・圧縮の場所と codec は #1034 AC-1 が Linux と Windows の試作で決める。比べる案は、(a) Rust で取り込み・圧縮し、画面の WebCodecs で decode・描画、(b) 画面の `getUserMedia`・`getDisplayMedia`・WebCodecs で取り込み・圧縮・描画。どちらも描画は画面で行い、映像は Rust の音声の再生時刻に合わせる。H.264 の特許の扱いと、Linux の webview で使える codec（VP8/VP9 は既定の package、H.264 は追加の package）を判断に含める。

### 10. builder preview（D3）

#1033・#1034 は builder preview の完了条件に含めず、並行して進める。builder preview の計画（`docs/progress/2026-04-16-mvp-builder-preview-plan.md`）は変更しない。

## Feature Data Classification

### metaverse の空間音声

- Feature 名: metaverse spatial audio（moq-lite）
- Durable / Transient: Transient
- Canonical Source: なし（話者の端末の取り込み）
- Replicated?: No（購読中の聞き手へ実時間で送るだけ）
- Rebuildable From: なし
- Public Replica / Private Replica / Local Only: Local Only（memory）
- Gossip Hint 必要有無: No（音声を送っているかは既存の presence に載せる）
- Blob 必要有無: No
- SQLite projection 必要有無: No
- 必須 contract: 「対象別の contract」の #1033 の行
- 必須 scenario: Linux と Windows の実機 2 台で、同時発話・隣接 Dome・mute・退出を確かめる（手順は [runbook](../runbooks/metaverse-spatial-audio.md)）

### live 配信の映像・音声

- Feature 名: live media（moq-lite、視聴者の転送）
- Durable / Transient: media は Transient。track の descriptor（配信者の endpoint id、配信ごとの署名鍵の公開鍵、track の codec）は Durable（live manifest の一部、owner 署名と revision つき）
- Canonical Source: descriptor は ADR 0005 の docs state と manifest blob。media は配信者の端末
- Replicated?: descriptor は Yes、media は No
- Rebuildable From: descriptor は docs と blob
- Public Replica / Private Replica / Local Only: descriptor は session の scope に従う。media は memory
- Gossip Hint 必要有無: 既存の `SessionChanged`・`LiveSignal` だけ
- Blob 必要有無: manifest の JSON だけ
- SQLite projection 必要有無: 既存の live の projection。media は持たない
- 必須 contract: 「対象別の contract」の #1034 の行
- 必須 scenario: Linux と Windows で、配信者 1・視聴者 2 以上（1 名は途中参加、1 名は転送を受ける）の camera・mic・screen、同期、終了

## 対象別の contract

| Issue | contract | 確かめること |
| --- | --- | --- |
| #1033 | `metaverse_audio_starts_only_after_explicit_toggle_and_device_open` | 許可が無い・機器を開けない場合の送信 0、再起動後に mic off |
| #1033 | `metaverse_audio_plays_bursty_and_reordered_frames_in_timestamp_order` | 同じ話者の 180 ms 相当のまとめ到着と順序の入れ替わりを時刻順に再生し、上限を超えた分だけ捨てる |
| #1033 | `metaverse_audio_subscribe_is_denied_without_dome_access` | 資格なし・失効・owner の block で拒否し、音声と他の参加者の識別子を返さない |
| #1033 | `metaverse_audio_mute_drops_before_decode` | mute した author の decode・再生が 0 |
| #1033 | `metaverse_audio_budget_rejects_and_bounds_ledgers` | 送信量・購読数（16）・送る相手（64）・記録の上限と、超えたときの拒否 |
| #1033 | `metaverse_audio_stops_on_leave_access_change_and_restart` | 退出・失効・offline grace・再起動で取り込み・送信・再生が止まり、古い世代の結果を適用しない |
| #1033 | `metaverse_audio_gain_uses_opening_distance_for_ready_neighbors` | 既存の距離減衰、ready でない隣接 Dome の再生 0 |
| #1033 | `metaverse_audio_relay_fallback_keeps_direct_candidates_relay_free` | Direct の候補に relay URL が無く、Relay Fallback で届く |
| #1034 | `live_media_requires_explicit_start_and_permission` | 許可が無い場合の送信 0、開始を成功と表示しない |
| #1034 | `live_media_late_joiner_starts_from_latest_group` | 途中参加が最新の group から始まり、全 group を送らない |
| #1034 | `ended_live_session_rejects_media_subscribe` | 終了後・資格なしの購読の拒否（転送する視聴者でも）、停止後の送信 0 |
| #1034 | `live_media_fanout_keeps_upstream_bounded` | 配信者 4・視聴者 2 の上限、問い合わせの 16 段、祖先への接続の拒否 |
| #1034 | `live_media_drops_frames_without_broadcaster_signature` | 転送した視聴者が変えた frame を捨て、参加し直す |
| #1034 | `live_media_rejoins_after_parent_loss_within_rate_limit` | 親の喪失・5 秒の停止で張り直し、5 秒に 1 回まで |

## #1032〜#1034 の責務と削除先

- #1032（完了）: iroh 1.3 fork への移行。この ADR の依存の基準。
- #1033: §1〜§7・§9 の受け口・認可・停止・時刻・上限と、metaverse の音声の取り込み〜再生。削除: `MetaverseRoomEventV1::SpatialAudioFrame`・`MetaverseSpatialAudioFrameV1`・wire の `spatial_audio_frame`・`preflight_spatial_audio_event`・`useSpatialAudio.ts` の PCM の送信と即時再生・`resamplePcm16`・`SpatialAudioModel.ts` の距離関数・CLI schema と IPC 型の該当部分。ADR 0041 の音声の行と ADR 0043 の音声の節を実装に合わせる。
- #1034: AC-1 で映像の方式・codec・上限（§4 の live の行の値を含む）を決め、AC-2 で live の配信・視聴・転送・途中参加・終了を実装する。#1033 の部分を使い、live manifest に track の descriptor を足す（ADR 0005 の追補）。試作のコードは完成形に残さない。

## 旧記述の失効範囲

- ADR 0043「Presence and audio」の音声の部分（音声を `MetaverseRoomEventV1` で presence と共用する、16 kHz・mono・PCM16・1 frame 最大 320 sample）は、#1033 の merge でこの ADR に置き換わる。presence の扱い、距離減衰、mute、access の規則は ADR 0043 のまま。
- ADR 0041 の「Player / spatial audio / signed PCM16 frame」の測り方と、「Client / spatial audio / mute/drop oldest」の超えたときの扱いは、#1033 の merge で §4 に置き換わる。
- `docs/metaverse/p2p-room-mvp.md` の「voice」を対象外とする記述は、空間音声についてこの ADR へ移る。

## Consequences

- 音声の量が、topic の購読者全員ではなく、聞き手が選んだ話者との接続だけに比例し、話者・聞き手ごとの上限を持つ。
- live は、視聴者が増えても配信者の上りが一定で、中継 server の費用が掛からない。代わりに視聴者の上り回線を使い、段の数だけ遅延が増える。
- Linux でも mic と echo cancellation が使える。Windows ではマイク許可の確認画面が出なくなり、OS の mic の設定に従う。
- 版の違う kukuri の間では音声・映像が届かない。
- moq 側の版の更新が速いため、依存の更新は moq-lite の版と ALPN の見直しを伴う。
