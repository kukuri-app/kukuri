# metaverse の空間音声の確かめ方

正本は [ADR 0067](../adr/0067-media-transport-moq.md)（#1033）。音声は話者と聞き手の端末を `/kukuri/media/1` で直接つなぎ、取り込み・Opus・再生・echo cancellation は Rust 側で行う。確かめるのは OS の標準の入出力の境界まで（入力へ入れた音が出力に出るか）で、人が聞いて確かめることはしない（#1800）。

## 自動の試験

`apps/desktop/tests/real-app/scenarios/spatial-audio.mjs`。配布と同じ build を 3 つ（聞き手 L・話者 S・O）同じ PC で動かし、L が Dome を作って稼働・入室し、S と O が入る。動かし方は [dev.md](dev.md) の「実際のアプリを複数動かす自動の試験」。CI では Linux package で、関係する PR ごとと毎晩に動く。instance ごとの仮想の入出力（`REAL_APP_AUDIO=pulse`、PulseAudio の null sink）を使う。

Windows の CI では動かさない（GPU の無い runner では、3 つの Dome の 3D の描画が CPU を埋める。2026-10-14 のユーザー判断）。Windows の音声の機器の経路は、[live の試験](live-media.md)で確かめる（同じ Rust の音声の部品）。

製品のAECには、入出力のバッファ遅延が変わる回帰条件もある（#1944）。`python tools/check_media_audio_contract.py` は、固定したmoq forkを専用の一時checkoutへ取得し、consumerと同じDSP・デバイス依存で関連するnative音声のunit testを実行する。git・Rust・Python・cargo-nextestを使い、Cargoの共有source cacheは変更しない。Linux package CIにも同じ入口を置く。

再現条件は、48 kHz・stereo・10 ms frame、元の帯域の断続音、4秒の学習と6秒の発話。デバイスの遅延を120 msから90／150 msへ1秒ごとに切り替え、固定seed 1・2の最後の4秒を5%・10%基準で判定する。AECを無効にした対照でも、自己音声の戻りを検出することを確認する。デバイス時刻で参照音を対応付け、マイクのframeの送出を保つ。

参照音は開始時とバッファ遅延の変化時に位置を合わせ、その間は再生したPCMの順序を保つ。0.5〜1 msの時刻の揺れも同じ回帰testで確認し、時刻ごとの補間や音声の切り捨てで参照波形を変えない。収録時刻と参照の先読み量を分け、先読み量の増減だけでは同じ参照音を再利用したり、飛ばしたりしないことも確認する。

| 項目 | 確かめること | 観測 |
| --- | --- | --- |
| SA-1 | 入室した直後の mic は off | HUD の mic の button |
| SA-2 | mic を on にすると on。入力デバイスが無いと on にならず、聞き手は無音のまま | button・log・聞き手の出力 |
| SA-3 | 話者の声が聞き手の出力に出る。離れると、出力の大きさが減衰の式（1 m 以内は 1、以遠は 1 m / 距離）のとおりに減る（±20%） | 聞き手の出力の大きさ・画面が渡す位置 |
| SA-4 | 聞き手のスピーカーの音が聞き手の mic に入る部屋でも、話者に話者自身の声が戻らない（聞き手の声の 5% 未満）。聞き手の声は話者に届く（話者の声の 10% 超） | 話者の出力の帯域ごとの大きさ |
| SA-5 | 通行可能な隣の Dome の話者の声が opening 越しに出る。接続を解除すると止まる | 聞き手の出力 |
| SA-6 | mute した話者の声だけが止まる。mute した話者の presence と移動は届く | 聞き手の出力・HUD の診断 |
| SA-7 | 話者が Dome を出ると 5 秒以内に無音。再起動の後の mic は off | 聞き手の出力・button |
| SA-8 | log の経路は `direct_p2p`。音声まわりの log に他の端末の識別子が出ない | 各 instance の log |

## 失敗時の診断

SA-4が失敗したときは、既存の失敗artifact `kukuri-real-app-linux` の `spatial-audio/speaker/sa4/` に診断を残す（#1944）。`measurement.json` に丸め前の値、判定窓の時刻・blockの受信時刻、1秒ごと最大12回の帯域の大きさとPulseAudioの入出力遅延を置く。`listener.s16le`・`speaker.s16le` は同じ窓の出力PCM（48 kHz・mono・s16le、各4秒と境界の20 msまで）。受信が一度に届いて保存上限で切れた場合は、`observed_blocks` と保存した `blocks` の件数が異なる。音声出力の再作成は各instanceの `app.log` の時刻と照合する。

成功時は音声を保存しない。診断には既存の録音bufferを使い、録音process・仮想デバイスを増やさない。遅延の取得は1要求500 ms・64 KiBまでとし、取得に失敗した場合はJSONの `error` に残す。診断の保存に失敗しても元のassertion失敗は維持し、保存できなかった理由を試験のlogへ出す。局所testは `node --test apps/desktop/tests/real-app/*.test.mjs`。

## 試験の音

- 話す音は、帯域の周波数を音ごとに無作為な位相で同時に鳴らす断続音（400 ms ごとに 150 ms）。途切れない音は雑音除去が弱め、1 つの周波数の音はエコーキャンセルが学びきれないか、遅れた自分の声から予測して消してしまうため（どれも実際の声では起きにくい）。
- SA-4 の部屋は、聞き手の取り込みを聞き手の出力の monitor へ移して作る。PulseAudio の戻しの module は遅延を保つために変換の比を揺らし続け、エコーキャンセルが追えない。

## 既知の制約

- Linux の cpal は PipeWire・PulseAudio の環境に未修正の不具合がある（cpal #1390、PR #1387）。
- 版の違う kukuri の間では音声が届かない（moq-lite の版を kukuri の版ごとに固定する。ADR 0067 §1）。
- 入出力バッファの遅延はデバイス時刻で対応付ける（#1944）。反響経路自体が変わるなど、デバイス時刻で説明できないずれがある場合は、両方が同時に話すとエコーキャンセルが相手の声も弱める制約が残る（#1822では数秒、1/4ほどまで弱まった）。
