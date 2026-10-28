# メディアカラム

- Status: current
- Supersedes: None
- Superseded by: None
- Identifier: #1949、ADR 0069
- Preview: [キャッシュ・dark・英語](assets/media-column/cache-dark-en.png)、[キャッシュ・light・日本語](assets/media-column/cache-light-ja.png)
- 対象と目的: 手元の画像・動画を整理したい利用者。キャッシュ・ブックマーク・自分の投稿の画像・動画を1つのカラムで見て、そこから消せるようにする。
- 採用した変更: Control Centerの「カラムを追加」に「メディア」を足す。ヘッダーのアイコンのtab（キャッシュ・ブックマーク・投稿）で切り替え、選択はカラムごとに保存する。一覧はサムネイルの格子で、1行の数はカラム幅に合わせる（最小6rem）。20件ずつ前後のページへ置き換える。各サムネイルの左上のghostのアイコンボタンで、キャッシュは削除、ブックマークは解除、投稿は確認の後に取り下げる。一覧の読み込み・プレビュー・表示制限は「ブックマークから選択」のDialogと共通の部品にした。
- 確認条件: Windows上のChromium（mock）。1280×844・dark・英語と390×844・light・日本語。他人の画像つき投稿6件を表示してキャッシュへ置き、カラムの追加、6件の表示、1件の削除、「投稿」への切替と空表示、再読み込み後の選択の維持を確認した。
- 入力とAccessibility: tabは矢印・Home・Endで移動し、選んだtabへfocusが移る（Vitest）。カラムへのaxe-core検査は両条件で違反0件。カラムの横方向のoverflowは1px以内。投稿の取り下げは既存の確認Dialog（`window.confirm`）を通る。
- 表示制限: 表示中の20件を既存のadvisory lookupへ加え、自己申告・post_id/blob_cid advisory・共有hashの制限を「ブックマークから選択」と共用する。pending中はプレビューを取得しない。
- 資源: 一覧は20件の置換窓で、画像か動画のposterだけを1件ずつ読む。動画の本体は読まない。「投稿」は自分のプロフィールの投稿を20件ずつ最大5回まで読む。キャッシュの一覧の行はcacheのblobと同じtransactionで消え、cacheより多く残らない。
- 検証: Playwright `media-column.spec.ts`（2条件）、Vitest（メディアカラム・ブックマーク・layoutの保存）、typecheck・ESLint、Rustのstore・app-api・CLIの対象test、migrationの全64世代の往復とschema golden。
- 未確認: Tauri実機、screen reader、touchの実入力。WebのIndexedDB（版6）の動作はCIの`linux-web-transport`のbrowser試験で確認する。
- Review result: 独立監査の結果をPRに記録する。
- 例外: なし。
