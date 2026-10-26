# 2026-10-27 Liveの投稿添付画像

- Status: current
- Supersedes: None（#1841の本文・返信・リアクション表示を継承）
- Superseded by: None
- PR: #1950 AC-1 / PR-1、Scope revision 2026-10-27-r2
- Preview: [ブラウザの描画](assets/1950-overlay-images.png)
- 対象・利用者・目的: Liveを見ている利用者が、会話に添付された先頭の画像・GIFを映像と一緒に見る。
- 採用内容: 先頭画像1枚、透明背景、最大192×160 CSS pxかつ映像幅の1/3、GIF再生、reduced motionで静止画。本文・作者・返信・reactionの操作と表示条件を維持する。
- 条件: WindowsのChromium、1440×900 / 390×900、dark/light、ja/en/zh-CN。本文ありPNG・本文なしGIF・複数添付・投稿レイヤーoff・reduced motion。
- 描画と操作: PNGの透明部分の画面pixelが背後の表示と一致する。GIFの画面画像が時間経過で変わり、reduced motionでは変わらない。既存の投稿切替ボタンから画像を隠して再表示できる。overlayはaria-hidden・pointer-events noneを維持し、画像に操作やfocusを増やさない。
- 性能・回収: 画像の自動表示は最大5枚。先頭以外の画像需要と画像切替timerを作らない。通常media ownerへhashの需要を渡し、gate・非表示・unmountで返す。
- Validation: 対象Vitest 29件、Playwright 12条件、対象ESLint、TypeScript。変更前は添付表示testが失敗し、変更後は成功。
- 未確認: Tauri/WebView・別端末の保存blobからの表示はPR CIと複数端末scenarioで確認する。ブラウザfixtureは通信・保存の成功を示す証拠にはしない。screen readerの実機確認は未実行。PushはAC-2の対象。
- Review result: ブラウザの透明表示・GIF・静止表示を確認済み。独立監査とCIはPR headで判定する。
- Exceptions: なし。
