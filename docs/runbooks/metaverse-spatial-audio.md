# metaverse の空間音声の確かめ方

正本は [ADR 0067](../adr/0067-media-transport-moq.md)（#1033）。音声は話者と聞き手の端末を `/kukuri/media/1` で直接つなぎ、取り込み・Opus・再生・echo cancellation は Rust 側で行う。確かめるのは OS の標準の入出力の境界まで（入力へ入れた音が出力に出るか）で、人が聞いて確かめることはしない（#1800）。

## 自動の試験

`apps/desktop/tests/real-app/scenarios/spatial-audio.mjs`。配布と同じ build を 3 つ（聞き手 L・話者 S・O）同じ PC で動かし、L が Dome を作って稼働・入室し、S と O が入る。動かし方は [dev.md](dev.md) の「実際のアプリを複数動かす自動の試験」。CI では Linux package で、関係する PR ごとと毎晩に動く。instance ごとの仮想の入出力（`REAL_APP_AUDIO=pulse`、PulseAudio の null sink）を使う。

Windows の CI では動かさない（GPU の無い runner では、3 つの Dome の 3D の描画が CPU を埋める。2026-10-14 のユーザー判断）。Windows の音声の機器の経路は、[live の試験](live-media.md)で確かめる（同じ Rust の音声の部品）。

| 項目 | 確かめること | 観測 |
| --- | --- | --- |
| SA-1 | 入室した直後の mic は off | HUD の mic の button |
| SA-2 | mic を on にすると on。入力デバイスが無いと on にならず、聞き手は無音のまま | button・log・聞き手の出力 |
| SA-3 | 話者の声が聞き手の出力に出る。離れると、出力の大きさが減衰の式（1 m 以内は 1、以遠は 1 m / 距離）のとおりに減る（±20%） | 聞き手の出力の大きさ・画面が渡す位置 |
| SA-4 | 聞き手のスピーカーの音が聞き手の mic に入る部屋でも、話者に話者自身の声が戻らない（聞き手の声の 5% 未満） | 話者の出力の帯域ごとの大きさ |
| SA-5 | 通行可能な隣の Dome の話者の声が opening 越しに出る。接続を解除すると止まる | 聞き手の出力 |
| SA-6 | mute した話者の声だけが止まる。mute した話者の presence と移動は届く | 聞き手の出力・HUD の診断 |
| SA-7 | 話者が Dome を出ると 5 秒以内に無音。再起動の後の mic は off | 聞き手の出力・button |
| SA-8 | log の経路は `direct_p2p`。音声まわりの log に他の端末の識別子が出ない | 各 instance の log |

## 試験の音

- 話す音は、帯域の周波数を音ごとに無作為な位相で同時に鳴らす断続音（400 ms ごとに 150 ms）。途切れない音は雑音除去が弱め、1 つの周波数の音はエコーキャンセルが学びきれないか、遅れた自分の声から予測して消してしまうため（どれも実際の声では起きにくい）。
- SA-4 の部屋は、聞き手の取り込みを聞き手の出力の monitor へ移して作る。PulseAudio の戻しの module は遅延を保つために変換の比を揺らし続け、エコーキャンセルが追えない。
- PulseAudio は、desktop の音声の server（rtkit の下の PipeWire・PulseAudio）と同じく real-time の優先度で動かす（CI は runner の上限を上げてから起動する）。優先度が無いと、CPU が埋まったときに server が遅れて app の出力が作り直され、エコーキャンセルが数秒合わずに話者の声が戻る（#1822）。

## 既知の制約

- Linux の cpal は PipeWire・PulseAudio の環境に未修正の不具合がある（cpal #1390、PR #1387）。
- 版の違う kukuri の間では音声が届かない（moq-lite の版を kukuri の版ごとに固定する。ADR 0067 §1）。
