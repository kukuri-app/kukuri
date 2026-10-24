# 画像・動画のブックマーク

- Status: current
- Supersedes: None
- Superseded by: None
- Identifier: `codex/media-bookmarks`、ADR 0069
- Preview: [選択Dialog・dark](assets/media-bookmarks/picker-dark.png)、[画像viewer・light](assets/media-bookmarks/viewer-light.png)、[メタデータ・light](assets/media-bookmarks/metadata-light.png)
- 対象と目的: 投稿の画像・動画を保存し、自分の投稿へ再利用する利用者。投稿のブックマークとは別に保存し、元投稿の情報を確認できるようにする。
- 採用した変更: 画像・動画・サムネイルの左上にghostのアイコンボタンを置く。添付欄から一覧Dialogを開き、選択した既存blobを下書きへ追加する。右クリックとShift+F10から元投稿・投稿者・hashなどを表示してコピーする。
- 確認条件: Windows上のChromium。1280×844・dark・英語と390×844・light・日本語。保存済み・未保存、選択、取消、コピー、再利用、画像viewer、成人向け表示制限を確認した。Storybookには一覧のready・empty・loading・error・gatedを置いた。
- 入力とAccessibility: Playwrightでpointer、右クリック、Shift+F10、Escape、focus復元、clipboard、下書きの維持を確認した。選択DialogとメタデータDialogへのaxe-core検査は両条件で違反0件。狭幅のDialog内の横方向のoverflowは1px以内。
- 表示制限: 一覧の20件も既存のadvisory lookupへ加え、自己申告・post_id/blob_cid advisory・共有hashの制限を共用する。pending中は取得せず、OFFへ戻した時点で取得済みのpreviewも隠して解放する。
- 資源: 一覧は20件の置換窓。画像または動画posterだけを1件ずつ読み、動画本体は選択一覧で取得しない。閉じる・ページ移動でpreviewを解放し、選択したURLの所有を下書きへ渡す。投稿時の解放もbrowser testで確認した。SQLのcursor条件は`bookmarked_media_order`索引を使う。
- 検証: 関連するRust・Vitest・typecheck・ESLint・Storybook build・Playwright、migrationの全63世代の往復とschema golden、Web向けcompile。詳細は[作業記録](../progress/2026-10-25-media-bookmarks.md)。
- 未確認: Tauri実機、screen reader、touchの実入力。新しいnative入力処理は追加せず、既存のDialogとclipboardを使ったbrowser上の操作を確認した。画素のbaseline比較はWindowsでは無効のため、実操作・画像の目視・寸法の検査を用いた。
- Review result: 実装と局所検証を完了し、独立監査待ち。
- 例外: なし。
