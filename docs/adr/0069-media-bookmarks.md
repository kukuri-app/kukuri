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
