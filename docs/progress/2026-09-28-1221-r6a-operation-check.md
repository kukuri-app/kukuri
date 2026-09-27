# #1221 R6-A: 完成版の動作確認

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R6-A と、2026-09-28 のユーザー決定「R6-A 動作確認への再定義」。
比較実測(公開版との比較・OS 指標・コード量)は行わない。

- 対象: 完成版を Windows の実アプリ・Linux の実アプリ(リモートデスクトップ経由)・CLI の 3 つで動かし、
  test 用の topic で一連の通常操作が成立するかを確かめる。
- 判定: 各操作の結果(成立／不成立と観測した事実)を記録する。不成立があれば、事実と原因の調査結果を記録し、
  修正を要するかをユーザーへ提示する。

## 判定

- 一連の通常操作(投稿・返信・添付・通知・DM と ACK・過去の投稿の参照・private channel の参加と回転・再起動)は、
  3 つのクライアントの間ですべて成立した。
- 見つかった所見 4 件は、ユーザー判断(2026-09-28)で #1221 の中で直した(R6-B: 所見 1〜3、R6-C: 所見 4)。
  補修版で該当操作を再確認し、すべて回復した。

## 環境

| 対象 | 版 | 実行場所 |
| --- | --- | --- |
| Windows アプリ | NSIS `kukuri_0.2.8_x64-setup.exe`。1 回目は main `541eae77` の push の CI、再確認は PR #1381 の head `ecc16c3e` の CI(tree は main `84fd0c2d` と一致) | Windows 11 Pro。データは試験用の `KUKURI_APP_DATA_DIR` |
| Linux アプリ | `kukuri_0.2.8_amd64.AppImage`。1 回目は PR #1380 の head `5331f5c6` の CI(tree は main `541eae77` と一致)、再確認は PR #1381 の CI | Ubuntu 24.04.5(Wayland)。GNOME のリモートデスクトップ経由。データは試験用の `KUKURI_APP_DATA_DIR` |
| CLI | `kukuri-cli_0.2.8_x86_64-unknown-linux-gnu`(同上) | 同じ Linux 機。profile `r6a`、`daemon run` で常駐 |
| community node | `docker-compose.community-node.yml` を main `541eae77` の worktree から build(別の compose project) | Windows の Docker。user-api・iroh-relay・indexer・relation-analyze・Postgres・ArcadeDB・Valkey |

- 本番の CN は古いので使わない(ユーザー指示)。3 つとも同じローカル CN に接続し、同意・認証を行った。
- topic は `kukuri:topic:test`。private channel は CLI が作成した `r6a-private`。
- indexer の安全性の判定(外部の provider)は設定していない。試験の投稿を外部へ送らないため。

## 操作と結果

| # | 操作 | 結果 |
| --- | --- | --- |
| 0 | 3 つのクライアントでローカル CN の同意と認証、profile の設定 | 成立。ただし CLI の `fetch_community_node_policies` は失敗(所見 1)。policy の revision は `/v1/policies` から読んで同意した |
| 1 | Windows から公開投稿 | 成立。Linux と CLI の timeline に表示 |
| 2 | Linux から 1 への返信 | 成立。Windows と CLI の thread に表示 |
| 3 | Windows から画像つきの投稿(PNG 11,037 byte) | 成立。Linux で画像を表示、CLI は `get_blob_media_payload` で同じ大きさの file を取得 |
| 4 | CLI から 2 への返信 | 成立。Windows と Linux に表示 |
| — | フォローと通知 | 成立。フォローされた側に未読の通知。フォローするまで相手の表示名が出ない(所見 2) |
| 5〜7 | DM(Windows→Linux、Linux→Windows の返事、CLI→Linux) | 成立。相手に届き、送信側は配達済み(ACK)になった |
| — | 過去の投稿の参照(profile の timeline、thread) | 成立。CLI の `list_profile_timeline`・`list_thread`、Linux の profile 表示 |
| 8 | CLI が private channel を作成し招待、Windows と Linux が参加、Linux から投稿(回転前) | 成立。3 者に表示 |
| 9 | CLI が channel を回転、Windows から投稿(回転後) | 成立。新 epoch の参加者 3、Linux と CLI に表示 |
| 10〜12 | 3 つとも終了して再起動。CLI から公開投稿、Windows から公開投稿、CLI から private channel へ投稿 | 成立。再起動前の timeline・DM・通知・channel(回転後の epoch)が残り、再起動後の投稿も 3 者に届いた。Linux の復元された profile 列に内部エラーの文言が残った(所見 3) |

## 所見

| # | 事実 | 原因 | 由来 |
| --- | --- | --- | --- |
| 1 | CLI の `fetch_community_node_policies` が `internal_error`(「command output does not match its registered schema」)を返す | CN は各 policy 文書に `policy_kind` を返す(`cn-protocol` の `models.rs`)。CLI の出力 schema `policy_document()`(`kukuri-cli/src/commands/community_views.rs`)に無く、`additionalProperties: false` で拒否される。CLI の試験の mock CN は `policy_kind` を返さないので検出しない | #1238(2026-09-20)から。#1221 の回帰ではない |
| 2 | follow していない author の表示名が、他のクライアントの timeline で「不明なユーザー」になる。再起動後も同じ。Linux で profile を開いても 20 秒出ず、follow すると出た | timeline の名前は手元の profile だけから読む(`timeline_view_support.rs`)。手元へ profile を入れるのは author の lease の task(`scope_receive.rs` の `spawn_author_subscription`)だけで、lease を取るのは profile 列・DM・CLI の desired。#1221 の前は `list_timeline` がページの全 author を購読して取り込んでいた(`78a7b1c2~1` の `timeline.rs` の `ensure_author_subscriptions_for_rows`)。R2-C で読取りの暗黙の購読を外したときに、この役目を置き換えていない。profile 画面は選択時に 1 回読むだけで、背景の取り込みが終わっても読み直さない | #1221 R2-C の回帰 |
| 3 | 再起動後、Linux の復元された profile 列に「sending to iroh_docs actor failed: sending into a closed channel」が残る。受信・購読・timeline は回復し、再起動後の送受信は成立 | 起動直後、CN の relay が 0 件から 1 件になり stack を作り直す(直接だけと relay つきの切替。R2-B で作り直しの対象と決めたもの)。その間の読取りが失敗する。購読・author の取り込み・timeline の remote 読取りは再試行されるが、profile 列の読込みは status が `loading` のときだけ再取得する(`useDesktopShellSectionLoaders.ts` の `loadShellSections`)ので、`error` のまま残る | 既存(作り直しは R2-B 以前から relay の集合の変化ごと、profile 列の条件は #995 から) |

- 補足: Linux アプリのログには、再起動後に `replica not open`(相手からの読取りで、手元に無い namespace)44 件と
  `remote page read failed`(connection lost など)54 件が出て、約 5 分半後に止んだ。相手が bucket を持っていないという
  想定内の取りこぼしで、受信は同時 8 本、送信は範囲ごとの台帳と 5 秒〜5 分の backoff で有界。不成立には数えない。

- 所見 1〜3 は R6-B(PR #1381、merge `84fd0c2d`)で直し、下の再確認で回復を確かめた。

## 補修後の再確認

同じ試験用のデータのまま、3 つを R6-B の成果物へ入れ替えて起動し直した。

| 対象 | 結果 |
| --- | --- |
| 所見 1 | CLI の `fetch_community_node_policies` が成功し、7 件の policy と `policy_kind` を返した |
| 所見 2 | follow していない author の名前が出た。CLI の timeline で Windows の author が `R6A Windows`、Windows の timeline で CLI の author が `R6A CLI` |
| 所見 3 | 再起動後の Linux の profile 列(CLI の author)は、名前つきで表示され、エラーは残らなかった(起動直後の stack の作り直しの WARN は前回と同じく出たが、表示は回復した) |

- 新たな所見 4: 再起動で復元された、選ばれていない DM の列(Linux、相手は Windows)が「不明なユーザー」「メッセージはまだありません」のまま、
  列を選ぶまで読み込まれなかった。列を選ぶと DM 5・6 が表示された。表示中の非 active の列を読む処理
  (`useDesktopShellData.ts` の `visibleListLoads`、#1349 で追加)は timeline・thread・profile・bookmark だけが対象で、
  DM の列は対象外。#1349 より前も、DM の列は選んだとき(`activateWorkspaceColumn` → `openDirectMessagePane`)だけ
  読んでいたので、#1221 の回帰ではない。
- 所見 4 は R6-C(PR #1382、merge `e8c314d4`)で直した。Linux のアプリを PR #1382 の CI の AppImage(tree は main
  `e8c314d4` と一致)へ入れ替え、通知の列を active にして再起動したところ、復元された非 active の DM の列が、
  選ばずに相手の名前(`R6A Windows`)と DM 5・6 を表示した。

## 試験環境の手当て(製品の不具合ではない)

- `docker-compose.community-node.yml` をそのまま起動すると、cn-user-api に `COMMUNITY_NODE_OPERATOR_CONFIG` が渡らず、
  cn-indexer は `COMMUNITY_NODE_CONNECTIVITY_URLS` が無く、起動しない。override の file で補った。
- Windows の peer ticket の直接の到達先は仮想 adapter(172.20.80.1)だった。Relay Supported P2P で通信は成立した。
- Linux の終了時に AppImage の外側の process まで止めたため、アプリが SIGBUS で終わった(止め方による。再起動後の状態は正常)。
