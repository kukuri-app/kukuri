# metaverse の空間音声の実機確認

正本は [ADR 0067](../adr/0067-media-transport-moq.md)（#1033）。音声は話者と聞き手の端末を `/kukuri/media/1` で直接つなぎ、取り込み・Opus・再生・echo cancellation は Rust 側で行う。

## 前提

- Tauri の desktop app（`audio-devices` を有効にした build）。Linux は build に `libasound2-dev`、実行に libasound が要る（deb は `libasound2t64 | libasound2` に依存する）。
- Windows と Linux（Ubuntu 22.04 / 24.04）の 2 台。同じ topic の Dome に入れること。隣接の確認には、Connection で ready な隣接 Dome が要る。
- mic と speaker。echo cancellation を確かめるときは headset ではなく speaker で再生する。

## 確認

1. 両端末で同じ Dome に入る。mic は off のまま始まる（自動で on にならない）。
2. 端末 A で mic を on にする。機器を開けない（Windows の OS の mic の設定で拒否している等）ときは on と表示しない。
3. 端末 B で A の声が聞こえる。A に近づくと大きく、離れると小さくなる（1 m 以内は等倍、以遠は 1 m / 距離）。
4. 両端末で同時に話す。speaker で再生しても、相手に自分の声が返らない。
5. ready な隣接 Dome から opening 越しに聞こえる。ready でない隣接 Dome からは聞こえない。
6. B で A を mute すると、A の声だけが止まる（移動・presence は変わらない）。
7. A が Dome を出ると、B の再生が 5 秒以内に止まる。再起動の後は mic が off から始まる。
8. 経路は `kukuri_connectivity` の log「media connection established」の `path`（`direct_p2p` / `relay_supported_p2p` / `relay_fallback`）で確かめる。話者の識別子と音声は log に出ない。

## 既知の制約

- Linux の cpal は PipeWire・PulseAudio の環境に未修正の不具合がある（cpal #1390、PR #1387）。途切れ・開けない場合は環境（sound server と版）とともに記録する。
- 版の違う kukuri の間では音声が届かない（moq-lite の版を kukuri の版ごとに固定する。ADR 0067 §1）。
