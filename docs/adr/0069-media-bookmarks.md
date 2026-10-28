# ADR 0069: 画像・動画のブックマークと再利用

## Status

Accepted

## Feature Data Classification

- Feature 名: 画像・動画のブックマーク、blob再利用、参照元メタデータ
- Durable / Transient: ブックマークと投稿の添付参照はDurable。Dialogの開閉・ページ・コピー結果はTransient。
- Canonical Source: ブックマークはアカウントのlocal store。投稿の添付参照と参照元は署名済みpost header、実体は既存blob。
- Replicated?: ブックマーク自体は同期しない。再利用した投稿の添付参照は既存の投稿同期に従う。
- Rebuildable From: ブックマークは自動再構築しない。投稿の添付参照はdocsから再構築する。
- Public Replica / Private Replica / Local Only: ブックマークはLocal Only。再利用した参照は投稿先の公開範囲に従う。
- Gossip Hint 必要有無: ブックマークには不要。投稿は既存hintを使う。
- Blob 必要有無: 既存の画像・動画・posterを参照し、ブックマーク中はcache回収から保護する。
- SQLite projection 必要有無: nativeは新しいbookmark table、Webは同じaccount databaseのobject storeを使用する。
- 必須 contract: 独立した保存・解除、cursorページング、参照の保護、既存blobの再利用、元投稿情報の保持、旧添付のroundtrip。
- 必須 scenario: 保存→選択Dialog→下書き→投稿、右クリック→メタデータ表示→コピー。

## Decision

画像は主blobごと、動画は本体とposterの組で保存する。投稿のブックマークとは別の保存先を使い、解除は他の保護参照を外さない。一覧は保存日時と主blobのhashの索引をcursorで20件ずつ読み、表示窓を置き換える。

添付参照に省略可能な`origin`（topic、元投稿のID、投稿者の公開鍵、投稿時刻）を加える。未設定なら表示中の投稿を参照元とし、再利用時は既にあるoriginを優先する。これは参照元の申告であり、作品の最初の作者であることの証明には使わない。旧データではoriginを省略し、省略時のserializeは変更しない。

利用者がブックマークから選び投稿したときだけ、保存した参照とoriginを投稿先へ公開する。通常のファイル添付と同じ投稿先確認を使う。メタデータの表示・コピーだけではblobの取得や投稿を行わない。

UIと検証条件は[作業記録](../progress/2026-10-25-media-bookmarks.md)および[DESIGN.md](../../DESIGN.md)に従う。

一覧を開いている間は、最大20件の添付と`bookmarked_from_object_id`（保存時点の投稿ID）を既存のcontent advisory照会へ加える。このIDは元投稿の表示に使うoriginとは分ける。未決の間は取得せず、自己申告・post_id/blob_cid advisory・共有hashの判定を通常の投稿と共用する。advisoryを自己申告ラベルへ書き換えたり永続化したりしない。ページ移動・終了で一時の照会対象を外し、表示OFFへの切替では取得済みpreviewも隠して解放する。

## メディアカラムとキャッシュの一覧（#1949）

メディアカラムは、キャッシュ・ブックマーク・自分の投稿の画像・動画を一覧し、そこから消せる。2026-10-28のユーザー判断で、キャッシュは他人の投稿の画像・動画に限り、自分の投稿の添付は「投稿」として別に出す。同じ画像・動画はキャッシュとブックマークのどちらか一方にだけ出す。

- Feature 名: キャッシュの画像・動画の一覧
- Durable / Transient: 一覧の行はDurable。ただしcacheの内容と同じ寿命で、blobが回収されると消える。
- Canonical Source: 手元のcache（`remote_content_cache`のblob行、Webは`contents`）。一覧の行はその索引で、表示した投稿の添付から作る。
- Replicated?: しない。Local Only。
- Rebuildable From: 作り直さない。cacheにある画像・動画は、次に表示したときに載る。
- Gossip Hint / Blob: 新しく持たない。既存のblobとcacheを参照するだけで、保護参照は足さない。
- SQLite projection: nativeは`cached_media`（主hash・posterのhash・載せた時刻・行のsnapshot）。Webは同じaccount databaseのobject store `cached_media`（版6）。
- 必須 contract: 載せる条件、ブックマークとの移り変わり、回収と同時の削除、削除で保護の無い部分を消すこと、cursorのページ。

画像・posterを表示のために手元へ置いたとき（既に置いてあったときを含む）、表示した投稿の添付から行を作る。動画は本体とposterを一組にし、主hashを動画の本体にする。保護の無い部分がcacheに無いもの（自分の投稿の添付、ほかの保護参照だけで残るもの）と、ブックマーク済みのものは載せない。載っていれば変えないので、表示のたびの書込みは起きない。

ブックマークすると同じtransactionで一覧から外し、解除するとcacheに保護の無い部分が残るときだけ一覧へ戻す。主hashかposterのどちらかがcacheから消えたら、消したtransactionの中で行を消す（容量・7日の未使用・成人向けの回収をすべて含む）。行がcacheより長く残ることはなく、行の数はcacheの容量で決まる上限を超えない。カラムからの削除は、行を外し、保護の無い部分をcacheから消す。保護された部分は残す。

一覧はブックマークと同じ形の行・cursor・20件の置換ページで、`list_bookmarked_media`・`remove_bookmarked_media`の省略可能な`shelf`（既定はブックマーク）で選ぶ。省略した旧来の呼び方の結果は変わらない。「投稿」は自分のプロフィールの投稿を20件ずつ最大5回読み、画像・動画を20件まで集める（取り下げ済み・リポストは除く）。新しい保存先は持たない。

Webの版5は、既存のdatabaseに`media_bookmarks`のstoreを作らなかった（新しく作ったdatabaseだけが持った）。版6の更新で、無いときだけ`media_bookmarks`と`cached_media`を作る。更新の間は既存の仕組みで「データの移行中です」を出して待ち、既存の行は書き換えない。
