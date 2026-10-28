# ADR 0067: metaverse の空間音声と live 配信のメディア搬送

## Status

Accepted（#1031、2026-10-13 のユーザー判断 D1〜D4）。実装と検証の完了は #1033（metaverse の空間音声）と #1034（live の映像・音声）の各 AC で判定する。#1033 の実装で §1・§3・§4・§7・§9 の実装の形を補った（2026-10-13 のユーザー判断で採用）。#1034 AC-1 で映像の配置・codec・取り込む対象・画質の上限を決めた（D5〜D8）。

- D1: live の映像・音声は、配信者が数人へ直接送り、受け取った視聴者が次の視聴者へ転送する（§8）。
- D2: 音声の取り込み・圧縮・再生は Rust 側で行う（§9）。
- D3: builder preview の完了条件に #1033・#1034 を含めない（§10）。
- D4: metaverse の話者 1 人が同時に音声を送る相手は 64 人まで、聞き手は近い順に 16 人の話者を聞く（§4）。
- D5: live の映像は画面（webview）の `getUserMedia`・`getDisplayMedia`・WebCodecs で取り込み・圧縮・復号・描画し、Rust は署名・搬送・認可を行う（§9）。
- D6: 映像の codec は VP8（§9）。
- D7: 配信できる映像は camera と画面で、1 つの配信で同時に送るのはどちらか 1 つ（§9）。
- D8: 画質の上限は camera が 1280×720・30 fps・1.5 Mbps、画面が 1920×1080・15 fps・2.5 Mbps（§4）。
- D9: 配信の操作（camera／画面の選択・mic の on／off・開始・停止）は、主催者の配信中の session の card に置く。session の作成は変えない（§9）。#1839 AC-3 で shell の上部の bar へ移した（下の「旧記述の失効範囲」）。
- D10: 視聴者の映像・音声の再生は「参加」で始まり「退出」で止まる（§9）。
- D11: 配信者は配信中に mic だけを止める・戻すことができる（§9）。

#1034 の AC-2 は、配信者から直接届く視聴者（4 人まで、5 人目は満員）の AC-2a と、視聴者どうしの転送の AC-2b に分ける（2026-10-13 のユーザー判断。D9〜D11 も同日）。AC-2a の監査の後、同日のユーザー判断で次も決めた: 主催者が live の列を離れたら配信を止める。この端末で作っていない session（別の端末・Web・この版より前）は、配信を始めるときに理由を示して断る。Windows の WebView2 が camera の許可を保存すること（§9）は受け入れる。AC-2b の実装で §8 の実装の形（参加の応答・署名の付け方・親が参加し直す間の子の待ち）を補った。

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

- WebKitGTK（Ubuntu の更新版は 22.04 が 2.50.4（以後の更新は終了）、24.04 が 2.52.6）: `getUserMedia` は build に含まれるが、permission の既定は拒否で、Allow する処理が要る。capture の処理経路に echo cancellation が無い（source の読解）。WebCodecs は 2.44 以降既定で有効。既定の install で使える映像の codec は VP8 の encode・decode と VP9 の encode で、VP9 の decode と H.264 は universe の package（plugins-bad・libav）が要る見込み（#1034 AC-1 の調査。実測は Ubuntu 26.04 の §9）。
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
- 置き場所: 受け口・接続・上限は `kukuri-iroh-node`、認可要求と descriptor の型は `kukuri-core`、access 判定は既存の `kukuri-app-api`、機器と codec は `kukuri-desktop-runtime`（native。Web は live だけ、#1879）。新しい crate・trait は作らない。
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
  - live: 要求は視聴者が署名した、session の scope と配信者（owner）に束縛した 10 秒の proof（Dome と同じ形）と、対象の session。送る側は自分が配信中の session であることと、session が終了していないこと（検証済みの state）と、private なら current epoch の active な参加者であることを確かめる。公開 live の事前の topic 購読 lease を条件にしない。転送する視聴者は、配信中の代わりに自分が視聴中で子へ公開している（親から受けている）session であることを確かめ、要求した端末が自分の祖先なら `ancestor` で断る（§8）。block は送る側の端末が知っている状態で評価する。
- 継続: 送る側は 5 秒ごとに、接続の初めに proof で確かめた相手の公開鍵について、自分の状態（block、公開 topic の gossip、private の参加者）で access を評価し直し、自分がまだ同じ Dome の session に居ることを確かめる。拒否なら session を閉じる（ADR 0045 の KeepAlive と同じ間隔）。送る側は Spatial Context の状態を持つ参加者なので、proof を送り直させない。moq-lite では announce を取り消しても進行中の購読は止まらないため、止めるときは session を閉じる。
  - 再確認の応答を待つ間も、認可済みの session の搬送と制御を進める。再確認は一つずつ実行し、拒否が確定したら直ちに閉じる。判定と5秒の周期は維持する。
- mute・block した author は購読しない（decode・再生の前に捨てる）。topology・参加・presence は変えない。metaverse の画面は、Dome とその隣に居る相手ごとに mute の状態を Rust の正本から引き、mute していないと分かった相手だけを話者の候補にする（画面の作者の情報は表示中の投稿などの作者しか持たないので使わない。#1807）。
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
| 配信者が直接送る視聴者（live） | 配信者の端末 | 4（#1034 AC-1 で確定。D8 の上限で、配信者の上りは最大約 10 Mbps） | 資格を確かめた相手に、直接の子の一覧と経路を添えて `full` で断る。視聴者は転送の参加へ回る（§8） | 子の退出・失効・30 秒無通信で解除。枠は認可で取り、接続の終了で空く |
| 視聴者が転送する相手（live） | 視聴者の端末 | 2（同上。転送する視聴者の上りは最大約 5 Mbps） | 同上（自分の子の一覧と経路） | 同上 |
| 転送の参加の問い合わせ（live） | 1 回の参加 | 16 段、1 段 5 秒 | 参加できないと表示し、画面が 5 秒ごとに試し直す | 参加の終了で破棄 |
| 視聴者の親（live） | 視聴者の端末 | 1 | — | 張り直しで置き換え |
| 経路（live、配信者から自分までの endpoint id） | 視聴者の端末 | 17 件（16 段と配信者） | 最も近い祖先から持つ | 参加し直しで置き換え |
| 親が公開を取り下げている間の待ち（live） | 視聴者の端末 | 90 秒（親が参加し直し終えるまでの上限: 停止の判定 5 秒、張り直しの間隔 5 秒、16 段 × 5 秒） | 親を失ったとみなして参加し直す | 親の公開し直し・接続の終了で解除 |
| 接続の試行の記録 | 端末 | 64 件・30 秒 | 試行しない | 期限で削除 |
| catalog | broadcast | 1 つ、4 KiB | 受けない | broadcast の終了で削除 |
| live の track | broadcast | 映像 1・音声 1 | 受けない | 配信の終了で削除 |
| live の group の保持 | track・転送する端末 | 最新 1 group | 古い group を捨てる | 配信の終了で削除 |
| live の映像の codec | 配信 | VP8（D6） | — | — |
| live の映像の画質（D8） | 配信 | camera: 1280×720・30 fps・1.5 Mbps。画面: 1920×1080・15 fps・2.5 Mbps | 配信者の encoder の設定で守る。受ける側は catalog が示す今の source の frame 数（30・15 /s）と bitrate のそれぞれ 1.25 倍（配信者の間引きと同じ）を、1 秒分の枠を経過時間に比例して補充して数え、超える分を捨てる | 配信の終了で破棄 |
| live の映像の group | track | keyframe ごと、2 秒 | 途中参加は最新の group（keyframe）から始める（遅れの許容 0 で購読し、最初の frame の後に下の 1 秒にする） | 配信の終了で削除 |
| 視聴の購読の最大遅延（live の映像） | 視聴者・転送する端末 | 1 秒 | 遅れた group は古いものから飛ばす | 視聴の終了で破棄 |
| 音声と映像のずれ（live） | 視聴者の端末 | 映像の遅れ 45 ms・進み 125 ms 以内を目標（ITU-R BT.1359 の検知の閾値） | 映像は Rust の音声の再生時刻に合わせて出し、遅れすぎた frame は捨てる | — |
| 画面へ送りかけの映像（Rust → 画面） | 視聴者の端末 | 1 秒分（30 chunk） | 次の keyframe まで捨てる | 画面の終了・視聴の終了で破棄 |
| encoder の待ち（画面 → Rust） | 配信者の端末 | 2 frame | 待ちが上限を超えた frame は encode しない | 配信の停止で破棄 |

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
- `moq-audio` は fork `KingYoSun/moq` の `77553ada566a9b6c3e6bdcf4143f7f9468793086` に固定する（#1805。上流へは PR を出さない）。0.1.11 は echo cancellation の参照の channel を一度も読まないため、再生の mix が canceller へ届かない。fork は 0.1.11 の commit に、channel を作った直後に 1 回読む修正を載せ、moq 系の依存（`hang`・`kio`・`moq-mux`・`moq-net`）を crates.io の同じ版にしたもの（workspace の path のままだと、git の依存では moq-net が crates.io の版と別の crate になる）。moq-audio の版を上げるときは、上流に同じ修正があるかを確かめる。
- #1879（Web の live）で、同じ fork の branch `kukuri/moq-0.1.11-wasm-transport` の `6573706f68cb7c8244e4e5855400ff80d2fc29a9` に進め、`moq-tokio`・`moq-mux` も fork から取る（上流へは PR を出さない）。moq-tokio は tokio の `full` を要求して wasm32 で build できないので、Web では poll 型への変換（`transport`）だけを含め、その変換の future は Web では Send にしない。moq-mux・moq-audio は `std::time` の `Instant`・`SystemTime` の `now` を使い、ブラウザでは実行時に止まるので、`web-time` の型を使う（native では std と同じ型）。moq-mux・moq-tokio の moq 系の依存も crates.io の同じ版にした。版を上げるときは、上流に同じ修正があるかを確かめる。
- Linux の build は libasound の開発 package、配布物は libasound を要する。
- #1944の製品対策では、同じforkの `54dde69f0abd6c00c1faca0dcf77d603b61282ba` に進める。再生参照の受け渡しを時刻付きの有界履歴に置き換え、取り込みとデバイスの再生・収録時刻で対応付ける。参照の受け渡しと履歴はそれぞれ最大256 block（stereo・48 kHz、1 blockは最大10 ms）とし、履歴は参照時に末尾が500 msより古いblockを除去する。micの開始・停止と出力streamの再作成で回収する。マイクのframeの送出、公開API、wire、保存形式、UIは変更しない。`tools/check_media_audio_contract.py` が固定したforkの回帰条件と共有native経路のunit testを実行する。
- #1955ではforkを `03bae00ab0db6a373142fb20bc95a40fdd6c1274` へ進める。収録時刻の推定が利用可能な再生参照より先の場合、完全な参照が2 frame以上あれば次の収録callback用に1 frame残す。
  収録時刻が後退していない間は、消費済みPCMより前へ再配置しない。デバイスbufferの変動で収録時刻が後退した場合は、保持した履歴への再配置を維持する。
  実際の再生・収録は待たせず、参照の上限・回収とSA-4の5%・10%基準を維持する。
- aws-lc は既に desktop の build に入っている（str0m 経由）ため、web-transport-iroh の既定の TLS で新しい C の build は増えない。
- 輻輳制御と QUIC の stream 数の上限は endpoint の既定のままとし、変える必要は #1033・#1034 の計測で示す。
- moq 側の版を上げるときは、moq-lite の版（§1）と ALPN の版を同時に見直す。

### 8. live の届け方（D1）

配信者は直接 4 人まで送り、受け取った視聴者が次の視聴者へ 2 人まで転送する。Community Node 等の中継 server は使わない。

- 参加: 視聴者は live manifest の descriptor が示す配信者の端末へ認可要求を送る。空きがあれば配信者の子になる。空きが無ければ、配信者は直接の子の一覧（endpoint id）と、自分までの経路（祖先の列）を返す。視聴者は一覧から無作為に 1 人を選んで同じ問い合わせを繰り返す（§4 の 16 段まで）。自分が祖先の列に含まれる候補には付かない（循環の防止）。
  - 応答の形（#1034 AC-2b）: 許可は `ok` に改行と `{"route":[…]}`（配信者から応答した端末までの endpoint id の列）、満員は `full` に改行と `{"route":[…],"children":[…]}` を続ける（応答は 4 KiB まで、経路は最も近い 17 件まで）。一覧と経路は資格を確かめた相手にだけ返す。転送する視聴者は、要求した端末が自分の経路に含まれれば `ancestor` で断る。
  - proof は 10 秒なので段ごとに作り直す。配信者に断られた（映像が無い・資格が無い）・届かないときはその状態を示し、転送する視聴者に断られた・届かないときは同じ一覧の別の候補へ進む。
- 転送: 子の session の公開範囲には、親から受けている broadcast を置く。途中参加は最新の group から始まり、全 group を送らない。
  - 視聴者は配信者の署名を確かめた frame を、中身（署名を含む）・timestamp・group の区切りを変えずに自分の broadcast（catalog は親の写し）へ置き直して公開する（#1034 AC-2b）。親を替えても同じ broadcast に置き続けるので、子の購読は続く（親を替えた直後は次の keyframe から置く）。
- 張り直し: 親との接続が切れたか、5 秒間 group が届かなければ、配信者から参加し直す（視聴している間だけ、5 秒に 1 回まで）。自分の子は付いたまま移る。
  - 親からの frame が 2.5 秒途絶えたら、親を失ったとみなす前に子への公開を取り下げる。子は親が取り下げている間は 5 秒の判定をせずに待ち（§4 の 90 秒まで。親との接続が閉じたら参加し直す）、親が公開し直したら購読し直す。上流の 1 人が止まっても参加し直すのはその子だけで、下流がそろって配信者へ参加し直さない（#1034 AC-2b）。
  - 配信者の画面は、画面が変わらない間も 1 秒ごとに前の frame を圧縮し直して送り、視聴者が親の停止と取り違えないようにする。
- 改ざん防止: 配信者は配信ごとの署名鍵を作り、その公開鍵を owner が署名した live manifest の descriptor に載せる。frame（または group）に配信者の署名を付け、視聴者は検証できないものを捨てて、その親から離れて参加し直す。転送する視聴者は中身を変えずに渡す。
  - 署名鍵は owner のアカウントの鍵と session id から導き（blake3 の `derive_key`、context `kukuri.app 2026-10-13 live media signer v1`）、x-only 公開鍵の hex を manifest の `media_signing_key` に載せる（ADR 0005 の追補）。frame ごとに、track 名・timestamp（μs）・payload の SHA-256 へ Schnorr で署名し、payload の先頭 64 byte に置く。音声は `moq-audio` が配信と同じ clock で非公開の broadcast へ書いたものを読み、署名を付けて配信の音声の track へ写す。鍵の無い manifest の session は media の無い session として扱う（#1034 AC-2b）。
- 段ごとの資格確認: 転送する視聴者も、子の認可要求を配信者と同じ規則（§3）で確かめる。
- 利用者への影響: 転送する視聴者の上り回線を、子の数 × 配信の bitrate だけ使う。段が深いほど遅延が増え、途中の視聴者が抜けると下流は張り直しまで止まる。視聴者は親と子の視聴者に endpoint と IP address が見える（既存の P2P の外部送信の範囲）。

### 9. 処理の場所（D2）

- 音声は Rust 側（`moq-audio`）で取り込み・圧縮（Opus、20 ms）・decode・mix・再生を行い、echo cancellation（`sonora`）は再生中の音を参照に使う。画面側の音声の取り込み・再生（`getUserMedia`・WebAudio）は削除する。live の音声も同じ経路を使う。
  - 既知の制約（#1822、2026-10-15）: 両方が同時に話し、スピーカーから mic への経路の遅れが揺れると、echo cancellation が相手の声も数秒弱める（試験の環境で 1/4 ほどまで。遅れが一定なら弱めない）。再生の参照に再生の時刻を付けて渡すなどの改善は、今は行わない。
  - #1944（2026-10-26のユーザー判断）で、上の見送りのうちデバイス時刻による入出力バッファ遅延への対策を採用した。再生・収録時刻を参照の対応付けに使う。反響経路自体の変化など、デバイス時刻で説明できないずれによる声の弱まりは、引き続き既知の制約とする。
- metaverse の距離減衰: 聞き手の端末の Rust が、話者の位置の track と、画面から受ける聞き手の位置・ready な隣接 Dome の opening の位置から既存の距離関数で gain を求め、話者ごとの音量に反映する。gain の関数は Rust の 1 か所に残し、TypeScript の gain の関数は #1033 で削除した（画面は話者を近い順に並べるための距離と opening の位置だけを持つ）。
- 映像（#1034 AC-1、D5〜D7）: 配信者の画面が `getUserMedia`（camera）か `getDisplayMedia`（画面）のどちらか 1 つを取り込み、`VideoEncoder`（VP8、`latencyMode: realtime`）で圧縮し、chunk を Tauri の IPC（raw の `invoke`）で Rust へ渡す。Rust は署名・moq の公開・転送・認可だけを行う。視聴者の画面は、受けた chunk を raw の応答で取りに来て（`next_live_video`。届くまで最大 1 秒待ち、送りかけは §4 の 1 秒分まで）、画面の `VideoDecoder` で復号して描画する。`ipc::Channel` は 1 KiB 以上の raw の payload を画面側の上限の無い queue に積むため使わない（#1034 AC-2a）。映像の frame は Rust の音声の再生時刻（再生中の音声の timestamp）に合わせて出す。映像の取り込みでは mic を使わず、live の音声は上の Rust の経路で取り込む。
  - 復号済みframeの保持は30枚まで。音声時計に対して表示できるframeが2枚以上ある場合は最古を捨て、それ以外は最新の到着を捨てて次の表示候補を残す。音声の進みを待つframeが、到着のたびに追い出されて描画が止まることを防ぐ。同期の基準と、reset・closeでの全解放は維持する。
  - 配信の操作（D9〜D11、#1034 AC-2a）: 主催者の card で camera か画面を選んで始める。broadcast は最初の keyframe で公開し、取り込みが拒否されたら配信を止めて送信 0。mic は取り込みが始まってから開き（取り込みが拒否されたら開かない）、配信中の off は `moq-audio` の `stop`（機器を閉じ、track と catalog は残す）。視聴者は catalog の更新を追って途中で足された音声の track を購読し、音声の再生時刻は最後に再生へ渡した位置から経過時間で進める（mic が止まっても映像を止めない）。視聴者は空きが見つからない・映像の無い間・止まった後に 5 秒ごとに試し直す（視聴中に親を失ったときは runtime が参加し直す。§8）。主催者の card が画面から消えると（列を閉じる・別の画面へ移る）、取り込みと配信を止める（再開は card の操作だけ）。視聴者は manifest の端末へ購読するので、配信できるのはこの端末で作った session だけで、それ以外は始めるときに理由を示して断る。
  - frameの取り出し（#1950、2026-10-27の性能改善）: windowに`MediaStreamTrackProcessor`があればそこで読み、Workerにある場合は取得済みtrackのcloneをWorkerへ移す。Workerの取得bufferは1枚、画面からのread要求ごとに1枚だけを移譲する。画面は最新frameを1枚保持して1秒ごとのheartbeatにも共用し、差替え・停止で解放する。停止ではWorkerと取得元trackも終了する。codec・送信queue・時刻のownerは画面側の既存処理に保つ。取得APIが無い環境では、既存のpreviewとcanvasによる取り出しを使う。
    - WebKitGTKでは、既存のmedia設定時に`MediaStreamTrackProcessing`だけを有効にする。[Worker用API](https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/Modules/mediastream/MediaStreamTrackProcessor.idl)を使い、UIでのvideo→canvas→VideoFrame変換を避ける。設定API（2.42以降）は実行中のlibraryから解決し、APIが無い場合は既存経路を維持する。camera・画面の許可は引き続き`LiveCaptureGate`で判定する。
    - Linux起動時、`/dev/dri/renderD*`・`/dev/dxg`・`/dev/nvidiactl`がすべて無ければ、`WEBKIT_DISABLE_DMABUF_RENDERER=1`を設定する。`card0`等の表示専用deviceや空の`dri`ディレクトリはGPUの判定に含めない。GPUのない環境でソフトウェア描画の転送待ちが入力を遅らせることを避ける。[WebKitの転送方式の選択](https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebKit/UIProcess/gtk/AcceleratedBackingStore.cpp)に従い、描画内容・角丸・影・mediaの品質は変えない。GPUデバイスがある場合と、利用者が`WEBKIT_DISABLE_DMABUF_RENDERER`または`WEBKIT_DMABUF_RENDERER_FORCE_SHM`を明示した場合は変更しない。Ubuntu 22.04のWebKitGTK 2.50.4（AppImageが同梱する版）でもこの選択が最も速い（2026-10-29実測、`live-overlay-push`の発行待ちを含む入力p95）。DMA-BUFの転送を有効にすると500〜660ms、共有メモリを強制すると530〜670msになり、無効にした既定は67〜136msだった。`WEBKIT_DISABLE_COMPOSITING_MODE`とSkiaのCPU描画の指定では変わらない。2.50は、GPUがないと文字が変わるたびにwindow全体を描き直す。2.52.6は変わった部分だけを描き直す。環境変数の設定はGTK初期化・スレッド生成前のmainだけで行う。
  - 許可: Tauri 2.12 の `on_permission_request` で、利用者が配信を始めた後だけ、その source（camera か画面）の取り込みを許す。それ以外は platform の既定のまま（Linux は拒否、Windows は WebView2 の確認）とし、移行用の QR の読み取り（#1628）の camera を変えない。ただし Windows の WebView2 は Allow を profile に保存するため、一度 camera で配信した後は、camera の要求（#1628 を含む）に確認が出ない（送るには配信の操作が要る）。Linux の WebKitGTK は既定で `getUserMedia`・`getDisplayMedia` を持たないので `enable-media-stream` を有効にする。Windows の画面は WebView2 の選択画面、Linux は xdg-desktop-portal（camera は Camera portal、画面は ScreenCast portal の選択画面が開始のたびに出る）。
  - 制約: Ubuntu 22.04 の webview は camera を列挙できない見込み（WebKitGTK 2.50 以降は PipeWire 0.3.64 以上を要し、22.04 は 0.3.48。未実測）。その場合は camera を選べないことを示し、画面の配信だけにする。Linux の VP8 は GStreamer の good で encode・decode できる（deb は host の plugin を使い、good は WebKitGTK の依存で入る。AppImage は build 機の base・good・gl・x の plugin を同梱する、#1795）。H.264 は deb では追加の package（libav か plugins-bad）が要り、AppImage では使えない。AppImage は pipewire の plugin を同梱しないため、camera・画面の取り込みは使えない見込み（#1034 で扱う）。
  - 不採用の案（Rust で取り込み・圧縮し、画面で復号・描画）: 上流の `moq-video` 0.1.11（2026-05 初版、kukuri と同じ moq の版）で camera・画面・H.264 の encode を賄えるが、encoder は H.264 だけで、Linux の視聴に追加の package が要り、OpenH264 を source から build して同梱すると配る側が AVC の特許の pool の対象になる。VP8 にするには libvpx の binding を kukuri が持つことになる。Wayland の画面の取り込みは、kukuri の `ashpd` の runtime の設定と衝突する。
  - 実測（2026-10-13、試作は破棄。Windows は Edge 155＝WebView2 runtime 154 と同じ系統、Linux は Ubuntu 26.04・WebKitGTK 2.52.6・GStreamer 1.28 の headless）:

    | 項目 | Windows | Linux |
    | --- | --- | --- |
    | VP8 の encode（合成の映像、1 frame） | 720p 約 2 ms、1080p 約 3.5 ms | 720p 約 9 ms、1080p 約 23 ms |
    | VP8 の decode（1 frame） | 約 1 ms | 約 1〜2 ms |
    | 取り込み→encode の遅延（fake・mock の camera、720p、p50／p95） | 1／3 ms | 7〜8／9 ms |
    | H.264 | 往復する | `avc` 形式（description つき）なら往復する。Annex B は decode に失敗 |
    | AV1 | 往復する | decode に失敗 |

- Web（#1879。desktopと同じ条件でliveの配信と視聴を出す。2026-10-19の判断にあった列・作成・公開スレッドの印・大きな表示の開発者モード限定は、2026-10-26のユーザー判断（#1934、[ADR 0060 §3](0060-web-client-entry-capability-and-verification.md)）で失効し、すべて通常モードで使える。game・metaverse・DomeはWebでは使えないまま）:
  - ブラウザの node も `/kukuri/media/1` を受け、desktop と同じ認可・転送・署名・上限で配信者と転送する視聴者になる。moq-lite の型はブラウザでは Send でないので、受け口は main thread の task で動かし（`confine_local`）、runtime は live の状態を作った thread だけから触る包み（`SendWrapper`。Web の build は thread を使わない）に入れて持つ。空間音声の認可要求は断る。
  - 映像: 取り込み・圧縮・復号は desktop と同じ画面の処理で、chunk は web-runtime の `push_live_video` へ渡す。camera・画面の取り込みの許可はブラウザの確認に任せる（Tauri の `on_permission_request` にあたる処理は無い）。
  - 音声: Rust（WASM）が、ブラウザの `getUserMedia`（echo cancellation・雑音抑制・自動の音量はブラウザの機能）の音を AudioWorklet（`live_audio_worklet.js`）で 20 ms ずつ受け、`moq-audio` で Opus に圧縮して、desktop と同じく署名して配信の音声の track へ写す。mic の off は mic の取り込みを閉じ、track と catalog は残す。再生は、Rust が復号した PCM を AudioBuffer にして前の音に続けて鳴らし（鳴らし始める前の保持は既定 50 ms、上限は §1 と同じ）、音量は GainNode で掛ける。音声の再生時刻（映像の時刻合わせ）は、AudioContext の時計で、まだ鳴らしていない量を引いて求める。映像の chunk は web-runtime の `next_live_video` で取りに来る。再生の AudioContext は視聴を始めるときに作る。ブラウザは利用者が操作した後の page にだけ音を出させるので、参加の操作から始めた視聴は鳴る（自動試験は自動再生を許す起動の引数で動かすので、この制限の下の動きは確かめていない）。

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
- 必須 scenario: Linux の CI で、配布と同じ build の自動の試験（話者 2・聞き手 1。距離の減衰・エコーキャンセル・隣接 Dome・mute・退出を、OS の入出力の境界で確かめる。手順は [runbook](../runbooks/metaverse-spatial-audio.md)、#1800）

### live 配信の映像・音声

- Feature 名: live media（moq-lite、視聴者の転送）
- Durable / Transient: media と hang の catalog（track の codec・画質）は Transient。descriptor（配信者の端末の endpoint id と、配信ごとの署名鍵の公開鍵）は Durable（live manifest の任意の項目、owner 署名と revision つき。ADR 0005 の追補）
- Canonical Source: descriptor は ADR 0005 の docs state と manifest blob。media は配信者の端末
- Replicated?: descriptor は Yes、media は No
- Rebuildable From: descriptor は docs と blob
- Public Replica / Private Replica / Local Only: descriptor は session の scope に従う。media は memory
- Gossip Hint 必要有無: 既存の `SessionChanged`・`LiveSignal` だけ
- Blob 必要有無: manifest の JSON だけ
- SQLite projection 必要有無: 既存の live の projection。media は持たない
- 必須 contract: 「対象別の contract」の #1034 の行
- 必須 scenario: Linux と Windows の CI で、配布と同じ build の自動の試験（配信者 1・視聴者 2 と、転送を受ける 5 人目。camera と画面・mic・同期・途中参加・終了・転送を、OS の入出力の境界と視聴者の canvas で確かめる。手順は [runbook](../runbooks/live-media.md)、#1800）

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
| #1034 | `live_media_presents_video_against_audio_clock` | 音声の再生時刻に合わせて映像を出し、ずれが上限を超える frame を捨てる |
| #1034 | `live_media_bounds_frames_waiting_for_the_screen` | 画面へ送りかけの映像が 1 秒分を超えたら次の keyframe まで捨て、受ける側の frame 数・bitrate の上限を超える分を捨てる |

## #1032〜#1034 の責務と削除先

- #1032（完了）: iroh 1.3 fork への移行。この ADR の依存の基準。
- #1033: §1〜§7・§9 の受け口・認可・停止・時刻・上限と、metaverse の音声の取り込み〜再生。削除: `MetaverseRoomEventV1::SpatialAudioFrame`・`MetaverseSpatialAudioFrameV1`・wire の `spatial_audio_frame`・`preflight_spatial_audio_event`・`useSpatialAudio.ts` の PCM の送信と即時再生・`resamplePcm16`・`SpatialAudioModel.ts` の距離関数・CLI schema と IPC 型の該当部分。ADR 0041 の音声の行と ADR 0043 の音声の節を実装に合わせる。
- #1034: AC-1 で映像の方式・codec・上限（§4 の live の行の値を含む）を決めた（D5〜D8、試作のコードは破棄）。AC-2a で配信者から直接届く視聴者の配信・視聴・途中参加・終了を、AC-2b で視聴者どうしの転送と配信者の署名を実装する。#1033 の部分（受け口・認可・音声の機器）を使い、live manifest に配信者の端末の id を足す（ADR 0005 の追補）。

## 旧記述の失効範囲

- D9 の配信の操作の置き場所と、2026-10-13 のユーザー判断「主催者が live の列を離れたら配信を止める」、§9 の「主催者の card が画面から消えると取り込みと配信を止める」は、#1839 AC-3（2026-10-16 の合意）で次に置き換わる。
  - 配信の開始（camera か画面）は主催者の card（Live の列・投稿の中）に置く。取り込み・圧縮・送信は、画面の表示から離れた 1 つの controller が持ち、配信中の操作（camera／画面の切替・mic・停止）は配信中の間だけ shell の上部の bar に出す。この端末から同時に配信するのは 1 つの Live だけ。
  - 配信が止まるのは、bar の停止、Live の終了（どの画面の「終了」も同じ処理を通る）、shell を離れるとき（アカウントの切替・app の終了）だけ。列を閉じる、別の画面・topic へ移る、投稿の card が画面外へ出ることでは止めない。
  - 明示の開始、取り込みが拒否されたときの送信 0、再起動の後に自動では始めないことは変えない。
- §9 の視聴（1 つの card が `watch`・`next_video`・`stop_watching` を直接呼ぶ）は、#1839 AC-4 で次に置き換わる。
  - 同じ Live の表示（Live の列・開始 post・紹介 post）は、画面の 1 つの視聴の需要を共有する。最初の表示で `watch` し、映像は表示ごとの canvas へ同じものを配り（復号と描画は表示ごと）、最後の表示が消えたら `stop_watching` する。runtime の購読・音声の再生・転送は Live ごとに 1 つ。
  - 音量は Live ごとにこの端末で設定し（0〜1、0 で無音）、runtime が音声の出力に掛ける。端末には最近変えた 64 件まで残し、共有の状態には書かない（2026-10-17 のユーザー判断 4）。

- ADR 0043「Presence and audio」の音声の部分（音声を `MetaverseRoomEventV1` で presence と共用する、16 kHz・mono・PCM16・1 frame 最大 320 sample）は、#1033 の merge でこの ADR に置き換わる。presence の扱い、距離減衰、mute、access の規則は ADR 0043 のまま。
- ADR 0041 の「Player / spatial audio / signed PCM16 frame」の測り方と、「Client / spatial audio / mute/drop oldest」の超えたときの扱いは、#1033 の merge で §4 に置き換わる。
- `docs/metaverse/p2p-room-mvp.md` の「voice」を対象外とする記述は、空間音声についてこの ADR へ移る。

## Consequences

- 音声の量が、topic の購読者全員ではなく、聞き手が選んだ話者との接続だけに比例し、話者・聞き手ごとの上限を持つ。
- live は、視聴者が増えても配信者の上りが一定で、中継 server の費用が掛からない。代わりに視聴者の上り回線を使い、段の数だけ遅延が増える。
- Linux でも mic と echo cancellation が使える。Windows ではマイク許可の確認画面が出なくなり、OS の mic の設定に従う。
- 版の違う kukuri の間では音声・映像が届かない。
- moq 側の版の更新が速いため、依存の更新は moq-lite の版と ALPN の見直しを伴う。
