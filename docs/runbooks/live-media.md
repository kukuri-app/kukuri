# live の映像・音声の実機確認

正本は [ADR 0067](../adr/0067-media-transport-moq.md)（#1034）。映像は画面（webview）の `getUserMedia`・`getDisplayMedia`・WebCodecs（VP8）で取り込み・復号し、音声は Rust 側（[空間音声](metaverse-spatial-audio.md)と同じ機器の経路）で扱う。配信者の端末は視聴者 4 人まで直接送る（視聴者どうしの転送は #1034 AC-2b）。

## 前提

- Tauri の desktop app（`audio-devices` を有効にした build）を、Windows と Linux の端末に入れる。配信者 1・視聴者 2（1 名は途中参加）。
- camera・mic・speaker。Ubuntu 22.04 の webview は camera を列挙できない見込み（ADR 0067 §9）で、その場合は画面だけを確かめる。
- 試験の配信は `test` topic に作る。

## 確認

1. 配信者が session を作り、card の「映像の配信を始める」を押す前は、視聴者が参加しても「配信者はまだ映像を送っていません」と出る。
2. 配信者が camera を選んで始める。許可を拒否する（Windows の確認、Linux の portal）と配信を止め、拒否の理由を出す。許可すると自分の映像が出て、視聴者に映像と声が届く。
3. 視聴者で口の動きと声がずれない（目安: 映像の遅れ 45 ms・進み 125 ms 以内）。
4. 配信者が mic を off にすると声だけが止まり、映像は続く。on に戻すと声が戻る。
5. 配信者が画面に切り替える。画面の選択画面が出て、選んだ画面が視聴者に届く。配信者が kukuri の window を他の window の後ろに回しても映像が止まらないこと（Linux は frame を表示中の `<video>` から取るため、window を最小化したときの扱いを記録する）。
6. 2 人目の視聴者が途中で参加すると、2 秒以内に今の映像から始まる。
7. 視聴者が退出すると再生が止まる。配信者が「映像の配信を止める」か session を終えると、視聴者の映像と声が 5 秒以内に止まる。終了後は参加できない。
8. 配信者の app を再起動しても、配信は自動で始まらない。
9. 経路は `kukuri_connectivity` の log「media connection established」の `path`（`direct_p2p` / `relay_supported_p2p` / `relay_fallback`）で確かめる。視聴者の識別子と映像・音声は log に出ない。

## 既知の制約

- 直接の視聴者は 4 人まで。5 人目は満員と出て、空きを 5 秒ごとに待つ（転送は #1034 AC-2b）。
- 配信者が別の端末で作った session は、その端末からしか配信できない（manifest の配信者の端末の id）。
- 版の違う kukuri の間では映像・音声が届かない（ADR 0067 §1）。
