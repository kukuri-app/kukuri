# 画像・動画のブックマークと再利用

基準: `cf5749ea552866a90b0f2d1203e4e1d8d1aa9493`。Scope revision 1。ユーザーから実装の依頼を受け、再利用後もブックマーク元の投稿者・元投稿を引き継ぐことを確認した。

## 完了条件

| ID | 対象・期待結果 | 判定方法 |
| --- | --- | --- |
| AC-1 | 投稿の画像・動画を投稿のブックマークとは独立して保存・解除できる。各画像、動画とそのサムネイル、拡大画像の左上にghostのアイコンボタンを出す | storeの保存・解除・再起動のcontract、componentとbrowserの操作 |
| AC-2 | 投稿・返信・公開スレッド作成の「ファイルを選択」の隣からモーダルを開き、保存済みの画像・動画を選んで投稿できる。動画はposterも再利用し、blobを再アップロードしない | 投稿のhash一致とblob書込みのcontract、composerのbrowser操作 |
| AC-3 | 投稿の画像・動画・サムネイルの右クリックとkeyboardから、対象のhash、MIME、bytes、role、元投稿・元投稿者・投稿時刻を表示し、個別またはまとめてコピーできる。再利用を繰り返しても参照元を保持する | coreのroundtrip、componentとbrowserの操作 |
| INVAR-1 | 投稿のブックマーク、画像拡大、動画再生、成人向け表示制限、添付のdraftを維持する | 既存の関連testと追加contract |
| INVAR-2 | 一覧は索引とcursorによる20件の置換窓とし、表示時の全件走査・全blob取得を行わない | storeのquery planとページングtest、表示窓の確認 |

## UIの設計

利用者は閲覧中に添付を保存し、後で自分の投稿に添付する。画像は1枚単位、動画は本体とposterを一組として保存する。保存単位の主blobのhashを識別子とし、poster上の操作も同じ動画を保存する。

モーダルはloading、empty、ready、errorを持つ。前後のページは置き換え、選択すると既存の下書きへ加えて閉じる。取消は下書きを変えず、開始ボタンへfocusを戻す。狭幅では1列、通常幅では2列で表示する。メタデータは右クリックまたはShift+F10からDialogを開き、長いhashと公開鍵を折り返す。コピーの成否を表示する。

成人向け表示が制限されている添付は取得・previewせず、既存の制限を保つ。保存は対象の検証済みprojectionだけを読み、再利用は保存した参照から投稿を作る。メッセージへの添付と画像編集は対象外。

## 実装と確認

- coreの添付参照へ省略可能な参照元を加える。既存の省略形のserializeを維持する。
- storeのアカウント別保存、索引、blob保護参照を同じtransactionで更新する。nativeとWebのtrait実装を揃える。
- runtimeと投稿作成は保存済み参照を解決し、既存blobのhashをそのまま使う。
- 共通の添付操作、モーダル、メタデータ表示を既存の投稿・viewer・composerへ接続する。
- 関連するRust、Vitest、typecheck、lint、StorybookとPlaywrightを選んで実行する。永続化・wire変更を含むため区分Cとし、固定commitを別コンテキストで独立監査する。

## 確認結果

実装・局所検証・独立監査を完了した。Issue [#1928](https://github.com/kukuri-app/kukuri/issues/1928) でPR CIとマージを進める。未決の製品判断とblockerはない。

- `cargo check -p kukuri-app-api -p kukuri-desktop-runtime`: 成功。
- core・store・app-apiの`bookmark`関連: 24件成功。複数の動画を含む投稿からの選択と、再利用時のblob書込みを検査する追加contractも成功。
- storeのmigration: 全63世代のdown/upとschema goldenを含む13件成功。新しいcursor queryが索引を使うことも確認した。
- `cargo check -p kukuri-cn-indexer -p kukuri-cn-e2e -p kukuri-harness -p kukuri-cli --tests`: 成功。添付の省略可能なfieldを加えた影響先のcompileを確認した。
- TypeScript、ESLint、Storybook build: 成功。関連するVitestは9ファイル・62件成功。
- Playwright: 新機能2条件に加え、公開スレッドと画像viewer・表示制限の関連9件が成功。最後の2条件ではaxe-coreの違反0件と、選択したpreview URLの解放も確認した。
- Web: Windowsにはclangが無いため、runbookのDocker imageを`kukuri-media-bookmarks-wasm`としてbuildし、`cargo check --locked --target wasm32-unknown-unknown -p kukuri-web-runtime`が成功した。registry・git・targetのvolume名は`kukuri-media-bookmarks-wasm-{cargo,git,target}`。
- 大型ファイルbaselineは、APIの合成、生成TS、schema golden、添付view、shellのproviderと既存advisory lookupへの接続に必要な6pathだけを更新した。featureのための型と接続であり、別責務の抽出は増やしていない。

[UI確認記録](../ui-reviews/2026-10-25-media-bookmarks.md)に画像・入力・未確認事項を記録した。Tauri実機とWebブラウザでの実ネットワーク操作は未実施。全体suiteはPR CIで確認する。ローカルで成功した検証は、変更影響のない範囲では再実行しない。

## 独立監査で見つかったINVAR-1の未達

固定head `41b90f3` の監査は、AC-1〜3とINVAR-2がPASS、INVAR-1のadvisory判定だけがFAILだった。自己申告ラベルのない保存画像にpost_idのadvisoryがある場合、Pickerが既存の照会と判定を通らずpreviewを取得していた。

旧headのPickerで追加testを実行し、pending中にpayloadが1回呼ばれる失敗をpost_id/blob_cidの2条件で再現した（`target/media-bookmarks-advisory-before.log`）。修正では、Pickerの20件を既存lookupと共有hash gateへ接続し、保存時点の投稿IDをoriginと別に保持した。新しい判定や独立したlookup経路は増やしていない。修正後は同じ条件に加え、表示OFFでのpreview解放・共有hashの制限を確認する23件が成功した。独立監査は追加差分とINVAR-1だけを再確認する。

再監査は固定head `1f72f3c6a384a9613ca02f46555e400df7d07066` に対してPASS、blockerは0件。監査側でも対象の3ファイル・23件が成功し、変更のないAC-1〜3・INVAR-2のPASSを維持した。全受入条件を満たしたため、実装と確認を終了した。

PR枠は#1928の末端成果AC-MEDIAの1枠とする。本書のAC-1〜3は、その成果に必要な判定観点として内容をすべて維持する。製品範囲と期待結果は変えず、CI成功とmerge後の内容照合を終えてからIssueをCloseする。

## PR CIでの補完

PR [#1929](https://github.com/kukuri-app/kukuri/pull/1929) の最初のhead `a39eddca3` では次の2件を確認した。

- Regression: Linux専用の`cn-indexer/tests/live_moderation.rs`の1行のAssetRef初期化に`origin`が無く、linux-cnのcompileがE0063で失敗した。CIの失敗を再現証拠とし、既存テストデータへ`origin: None`を補った。動作と旧形式のserializeは変えない。
- 意図した視覚差分: `composer-dialog-wide-dark.png`で「ブックマークから選択」の追加を含む差分を検出した（他45件は比較成功）。expected・actual・diffを目視確認し、runbookのKukuri Visual Baselineを同じbranchで実行した。[run 40126463425](https://github.com/kukuri-app/kukuri/actions/runs/40126463425) は成功し、生成artifactから該当する1枚だけを採用した。比較の閾値と検査は変更していない。

この追加差分はLinux専用のテスト初期値・基準画像・検証記録に限定する。製品範囲と実行時のコードは変更せず、追加差分の監査と更新headのCI成功後にマージする。

### 全体CIでの既存契約との同期

head `d16f76dd7` の全体CIでは、追加したruntimeの4入口にCLI対応表とfutureサイズ測定の登録が不足していた。既存のGUI/CLI対応契約に従い、保存・点取得・一覧・解除を共有runtimeへ委譲するCLI adapterとschemaを登録し、対応表を206入口・CLI登録簿を170件へ同期した。除外の追加や64KiBのfuture上限の緩和は行っていない。future検査は成功し、最大値は従来どおりmove_domeの47,408 bytesだった。

UI側では、独立したサムネイル操作に合わせたli単位のDOM参照、galleryの添付メタデータを含む期待値、追加ボタンを含むTab順を既存testへ反映した。画像の取得・表示制限・focus ringの検査は維持した。翻訳は既存の「ユーザー」用語へ揃え、日本語のhashラベルを「ハッシュ」にした。値とコピー内容は変更していない。

関連するCLI lib 34件・対応表5件・futureサイズ1件、UI対象4ファイル87件（修正後の再実行を含む）、keyboardとメディア再利用のPlaywright8件、typecheck・ESLint・format・差分検査は成功した。WindowsでTS featureを含めたCLI全integration testをまとめてbuildした際にはrustcの内部エラーが発生したため、関連targetはincrementalを無効にして確認し、全体確認は更新headのPR CIで行う。
