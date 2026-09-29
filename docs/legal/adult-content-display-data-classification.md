# Feature Data Classification: 成人向け表現の表示設定

ADR 0002 (`docs/adr/0002-feature-data-classification-template.md`) に基づく分類。仕様は ADR 0046。

### Feature Data Classification
- Feature 名: 成人向け表現の表示設定(adult content display preference)と取得ゲート
- Durable / Transient: Durable
- Canonical Source: ローカル設定ファイル(`<db_path>.content-display.json`、ユーザー端末のみ)。frontend は Rust 側の値の mirror。
- Replicated?: No(複製しない。ネットワークへ送らない)
- Rebuildable From: 再構築不可(ユーザーの設定行為そのもの)。喪失時・新規端末では既定 OFF に戻る。
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要(設定 OFF 中は成人向けラベル付き添付の blob 取得自体を行わない。ON 中に表示した添付は remote cache に成人向けの印を付けて置き、OFF へ戻したら背景で消す。#1419)
- SQLite projection 必要有無: 必要(成人向けラベルの hash 逆引き `adult_media_hashes`、object projection と object-backed notification projection の `content_labels`。取得・表示ゲートの判定に使う)
- 必須 contract: Tauri command `get_content_display_settings` / `set_adult_content_display_enabled` の payload 形状。`blob_media_payload` が「成人向けラベル付き hash かつ設定 OFF」で blob 取得を行わないこと。object-backed 通知が署名済み envelope 由来の `content_labels` を保持し、未解決の既存通知を設定 OFF で fail-closed に扱うこと。
- 必須 scenario: 取得ゲート(既定 OFF → 成人向けラベル付き添付の blob 取得・プリフェッチが発生しない → ON で ephemeral 取得 → OFF へ戻すと以後の取得停止 + 表示破棄)。取得の起点にはタイムライン系に加えて「見つける」の解決済み投稿(#1052)を含み、表示中の結果に限る一時状態として扱う(永続 projection にしない)。設定済み Community Node の content advisory が付いた添付も同じゲートで扱う(#1055。判定は self-label とは別欄で、`content_labels` へ書き戻さない)。表示ゲート(タイムライン・引用/埋め込み・返信プレビュー・Community Index の canonical 解決待ち/失敗/成功と解決済み投稿の添付メディア・in-app/OS 通知で raw text を露出しない)。frontend は `DesktopShellPage` / `CommunityIndexWorkspace` の vitest、backend は `crates/app-api` / Tauri のユニットテストで担保。

## 補足
- 表示設定は 18 歳以上の自己申告とは別の状態であり、自己申告だけでは ON にならない。既定 OFF。
- 成人向けラベルは投稿者自己申告(署名済み envelope の `content_labels`)であり、真正性は検証できない。ラベルなしコンテンツの安全は保証しない(ADR 0046)。

## 2026-09-15 改訂（#1051、ADR 0046 §6）: Community Node content advisory の合成
- ラベル源に、設定済み / 購読 Community Node が発行した `content_advisories`（ADR 0028 §8.6。`label = adult` / `sensitive`、issuer_node_id / category / confidence / signal_id / basis 付き）を第 2 の源として加える。node-local な advisory であり canonical でも署名対象でもない。`content_labels` へ書き戻さない。
- Blob: 設定 OFF 中は advisory 付き添付の blob 取得も行わない。ON 中に表示した添付は、self-label と同じく印を付けて remote cache に置き、OFF で消す（#1419）。
- SQLite projection: advisory 付き blob hash の集合を取得ゲート判定に使う。永続 projection にするか in-memory にするかは実装（#1051 child C3 / C4）で決定し、本節へ追記する。
- 実装の決定（#1055 = C3、2026-09-16）: **in-memory とする。永続 projection を作らない**。`AppService` がプロセス内の集合（`advisory_media_hashes`）として保持し、`adult_media_hashes` テーブルへは書かない。advisory は node-local かつ失効しうる判定であり、client 側は transient 分類（ADR 0028 §8.10）に従う。再起動で集合は空になるが、「見つける」は表示前に必ず index 照会を通るため、表示より先に再登録される。登録は insert-only で、表示設定 OFF / ON の切り替えでは集合を変えず、ゲートの可否は表示設定側で決める。
- 実装の決定（#1055 = C3、2026-09-16）: client は index 応答を受けた時点で issuer を照合する。desktop-runtime が `query_community_node_index` の応答後処理で、index を返した設定済み node の manifest `node_id` と `issuer_node_id` が一致する advisory だけを残し、manifest を取得できない場合は採用しない（fail-closed）。frontend へ渡る `content_advisories` は照合済みのみ。
- 実装の決定（#1055 = C3、2026-09-16）: プリフェッチの除外は hash 単位で行う。advisory は `PostView` に現れないため、投稿単位ではなく「ゲート中の blob hash」を shell state（`advisoryGatedMediaHashes`、一時状態）として持ち、プリフェッチの単一入口（`usePreviewableMediaAttachments`）で除外する。これは Rust 側 `blob_media_payload` のゲートを client 側で先取りするものであり、置き換えではない。
- 既知の限界（C4 まで）: advisory は index 応答でしか判明しないため、同じ添付が「見つける」以外の経路（タイムライン等）に先に現れた場合、client はその時点で advisory を知らず取得を要求しうる。その要求に対しても Rust 側ゲートが `None` を返すため bytes 取得は 0 だが、代替表示と説明は出ない。タイムライン経路の合成は C4（#1056）が一括照会 API で担う。
- 追加 contract: `advisory_labeled_media_respects_adult_display_gate`（`blob_media_payload` が「advisory 付き hash かつ設定 OFF」で blob 取得を行わない）、`content_advisories_are_separate_from_signed_content_labels`、`advisory_lookup_returns_only_configured_node_signals`（一括照会は設定済み node 自身の advisory のみ返す）。
- 追加 scenario: 表示ゲート（見つけるの `content_advisories`、タイムライン向け一括照会の応答）で self-label と同じプレースホルダーになり、発行 node / category / confidence と異議申し立て導線を説明できる。設定 OFF 中に advisory 付き media の bytes 取得が 0 であることを frontend vitest と `crates/app-api` の test で担保する。
- 利用規約 第3条 4 項の文言改訂と `LEGAL_BUNDLE_VERSION` 更新（再同意）は C4 で行う。それまで advisory の合成は有効化しない。
- 実装の決定（#1056 = C4、2026-09-16）: タイムライン・スレッド・ブックマーク・プロフィール・object-backed 通知の可視 subject（投稿 id、添付 hash、引用元・返信先の id と添付 hash）を、採用 node へ一括照会する。照会結果は subject 単位の一時状態（`timelineContentAdvisories`）と Rust 側の in-memory 集合（`advisory_media_hashes`）にだけ置き、永続化しない。
- 実装の決定（#1056）: 採用は node 単位の設定（community node 設定ファイルの `content_advisory_enabled`、既定 true）とする。これは利用者の設定値であり durable だが、推定そのものは保存しない。採用しない node へは照会しない。
- 実装の決定（#1056）: 照会は表示設定の ON / OFF にかかわらず行う。ON 中も advisory 付き hash は ephemeral 取得にする必要があるため。照会が確定するまでは、その投稿のメディアを取得せず、代替表示とは別のスケルトンを出す。照会先の有無が未確定の間も同様に扱う。
- C3 の既知の限界（見つける以外の経路では推定を知らず代替表示が出ない）は、上記のタイムライン系照会で解消した。
- 追加 contract（#1056）: `advisory_lookup_reads_do_not_mutate_state`、`lookup_sends_only_visible_ids_to_enabled_nodes`、`lookup_skips_nodes_with_content_advisory_disabled`、`lookup_stops_before_http_when_consent_is_pending`。
- 実装の決定（#1152、2026-09-18）: 表示用 projection と view に記録する blob の状態（投稿・DM の添付、投稿本文）は、ローカルの有無だけで決め、remote から取得しない（`BlobService::local_blob_status`）。advisory は受信・hydration の時点では未判明のため、状態確認の中で成人向け判定をしても防げない。それまでは状態確認が `blob_status` の remote 取得を兼ねており、表示設定 OFF の間も self-label / advisory 付き media の bytes を取得・永続化していた。remote 取得は、ゲートを持つ表示要求（`blob_media_payload`）と本文取得に限る。Dome の preset asset 確認と game / live の manifest は成人向けゲートの対象外で、remote で利用可能なことを確かめる `blob_status` を使い続ける。あわせて、desktop の本番で使う `ReloadableBlobService` が `fetch_blob_ephemeral` を実体へ転送しておらず、表示 ON の成人向け取得が trait 既定実装（永続化する `fetch_blob`）へ落ちて端末に保存されていたため、転送を追加した。
- 追加 contract（#1152）: `projecting_remote_adult_labeled_post_does_not_fetch_attachment_while_display_disabled`、`projecting_remote_posts_fetches_attachments_only_on_ungated_display_request`、`local_blob_status_does_not_fetch_or_persist_remote_blob`、`reloadable_blob_service_keeps_ephemeral_fetch_and_local_status_non_persistent`。
