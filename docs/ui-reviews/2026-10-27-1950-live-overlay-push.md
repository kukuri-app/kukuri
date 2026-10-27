# 2026-10-27 Live添付のPush

- Status: current
- Supersedes: None（通常の画像表示は[AC-1の記録](2026-10-27-1950-live-overlay-images.md)を参照）
- Superseded by: None
- PR: #1950 AC-2 / PR-2、Scope revision 2026-10-27-r3
- Preview: [20回のPushを合流した表示](assets/1950-overlay-push.png)
- 対象・利用者・目的: 公開スレッドの参加者が、表示可能な画像・GIFを添付ごとに選んでLiveへ再表示する。
- 採用内容: 投稿カードと画像viewerにPushを設けた。ユーザーの追加指定に従い、画像は画面下側へ寄せ、リアクション表示の上へ積む。表示中の同じ添付は送信者を問わず加算し、右下に `x20` のように表示する。5秒で消え、消えた後はx1から始まる。GIFは表示中の連打で再スタートせず、新しい表示では先頭から再生する。
- 条件: WindowsのChromium、1440×900 / 390×900、dark/light、ja/en/zh-CN。最新5投稿の外にある他人の投稿、2添付の2枚目、pointer・Enter・Space、投稿レイヤーoff/on、reduced motion。
- 描画と操作: 18回のクリックとEnter・Spaceでx20となり、回数が画像の右下に収まる。連打中もボタンが有効でfocusを維持する。Live状態が不明・開始待ち・終了済みの場合は操作不可の理由を表示する。PNG/GIFの透明部分に背景を塗らず、回数だけに可読性用の背景を置く。
- GIFの確認: 元画像の緑フレーム再生中にPushすると、別のoverlayは赤い先頭フレームから始まり、元画像は緑のまま継続する。元のobject URLを共用し、overlayごとに安定したfragmentを付けて再生を分ける。reduced motionはAC-1と同じ静止canvasを使う。
- 性能・回収: 24人相当が各10回/秒、7添付を60秒Pushするブラウザ試験で14,400回を処理し、描画は最大6枠、受付失敗なし、フレーム応答p95は100ms以内。準備済み画像の再取得は増えず、期限切れ後は0枠となる。hookの仮想時間testでも累積12,000回を1枠・1解決処理・1timerで扱い、unmountで回収する。
- Validation: 対象Vitest、Playwrightの12条件と60秒の負荷試験、TypeScript、対象ESLint。署名・重複・scope・寿命・保持上限はRustのcontractで検証する。実アプリの3端末試験と音声・Linux/WebKitはPRの検証結果へ記録する。
- 監査で見つかった未達の修正: reduced motionの縦長画像6枚が映像内に収まり、段どうしが重ならないことを実描画で確認した。送信者の信頼判定、後続Pushのindex/hash固定、許容未来時刻での再配送除去は、修正前に失敗するtestで再現してから修正した。
- Review result: ブラウザの表示と操作を確認済み。独立監査とCIは固定headで判定する。
- WebKitの実寸確認: decode成功後も通常画像が高さ0へ縮む状態をnative試験で再現した。通常投稿の縦配置をflexへ統合し、本文を縮めず画像だけを残りの高さへ収める。実アプリは画像の正の描画寸法・寸法上限と、Push回数の右下配置も検査する。
- Exceptions: screen readerの実機確認は未実行。ブラウザfixtureは通信・保存の成功を示す証拠にはしない。
