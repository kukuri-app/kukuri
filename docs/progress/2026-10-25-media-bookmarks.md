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

実装と局所検証を終え、独立監査を待っている。未決の製品判断はない。

- `cargo check -p kukuri-app-api -p kukuri-desktop-runtime`: 成功。
- core・store・app-apiの`bookmark`関連: 24件成功。複数の動画を含む投稿からの選択と、再利用時のblob書込みを検査する追加contractも成功。
- storeのmigration: 全63世代のdown/upとschema goldenを含む13件成功。新しいcursor queryが索引を使うことも確認した。
- `cargo check -p kukuri-cn-indexer -p kukuri-cn-e2e -p kukuri-harness -p kukuri-cli --tests`: 成功。添付の省略可能なfieldを加えた影響先のcompileを確認した。
- TypeScript、ESLint、Storybook build: 成功。関連するVitestは9ファイル・62件成功。
- Playwright: 新機能2条件に加え、公開スレッドと画像viewer・表示制限の関連9件が成功。最後の2条件ではaxe-coreの違反0件と、選択したpreview URLの解放も確認した。
- Web: Windowsにはclangが無いため、runbookのDocker imageを`kukuri-media-bookmarks-wasm`としてbuildし、`cargo check --locked --target wasm32-unknown-unknown -p kukuri-web-runtime`が成功した。registry・git・targetのvolume名は`kukuri-media-bookmarks-wasm-{cargo,git,target}`。
- 大型ファイルbaselineは、APIの合成、生成TS、schema golden、添付view、shellのproviderと既存advisory lookupへの接続に必要な6pathだけを更新した。featureのための型と接続であり、別責務の抽出は増やしていない。

[UI確認記録](../ui-reviews/2026-10-25-media-bookmarks.md)に画像・入力・未確認事項を記録した。Tauri実機とWebブラウザでの実ネットワーク操作は未実施。全体suiteとPR CIはこのローカル作業では実行していない。

## 独立監査で見つかったINVAR-1の未達

固定head `41b90f3` の監査は、AC-1〜3とINVAR-2がPASS、INVAR-1のadvisory判定だけがFAILだった。自己申告ラベルのない保存画像にpost_idのadvisoryがある場合、Pickerが既存の照会と判定を通らずpreviewを取得していた。

旧headのPickerで追加testを実行し、pending中にpayloadが1回呼ばれる失敗をpost_id/blob_cidの2条件で再現した（`target/media-bookmarks-advisory-before.log`）。修正では、Pickerの20件を既存lookupと共有hash gateへ接続し、保存時点の投稿IDをoriginと別に保持した。新しい判定や独立したlookup経路は増やしていない。修正後は同じ条件に加え、表示OFFでのpreview解放・共有hashの制限を確認する23件が成功した。独立監査は追加差分とINVAR-1だけを再確認する。
