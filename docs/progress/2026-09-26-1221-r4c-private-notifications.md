# #1221 R4-C: private の通知を account の受信経路へ接続する

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R4-C。区分 C、Scope revision
`2026-09-24-outcome-v5-r2a-retention-1`、基準は #1367 merge `fddcc37a`。依存の R4-B・R5-B は完了済み。

private channel の投稿・返信で、明示された相手（本文の mention、返信先の著者）へ、R4-B と同じ account の
受信 route・1 account の送信 worker（待機最大 64 件）で通知の参照を送る。参照の中身（署名済み投稿、本文、返信先）は
投稿した epoch の鍵で暗号化し（`PrivateReceivePayloadV1`、平文 16,384 byte 以内）、offer の scope は
`PrivateSource { epoch_key_id }` とする。公開の route の外側を復号できても、epoch 鍵が無ければ中身を読めない。

受信側は、参加中の channel の現在の epoch の識別子で照合し、一致しなければ provider へ要求しない。取得と反映は
その channel の参加の世代が続く間だけ行い、復号した投稿の署名・送信者・channel/epoch の replica・本文 hash・
返信先（自分の投稿で、同じ channel のもの）を確かめてから、既存の通知候補の生成と保存（`put_notification_if_absent`）へ
合流する。通知 kind は既存の reply と mention のまま（private では repost の通知は作らない、既存どおり）。

対象外: 旧購読（private channel の docs event）からの通知生成の撤去（R2-C）、DM（R4-D）、rotation の制御配送、
新しい通知 kind。平文が上限を超える投稿の offer は送らない（旧購読の経路は R2-C まで残る）。

## 受入条件

| ID | 対象・期待結果 | 判定 |
| --- | --- | --- |
| AC-1 | private の投稿・返信の mention と返信先の著者へ、epoch 鍵で暗号化した参照を `PrivateSource` の offer で送る。公開の投稿は R4-B のまま | 送信した offer の scope と、payload が epoch 鍵なしで読めないこと |
| AC-2 | 受信側は参加中の channel の現在 epoch と照合し、署名・送信者・replica・本文・返信先を確かめてから、既存の通知 ID で 1 件だけ保存する | mention と reply が 1 件ずつ、同じ offer の再受信で増えない |
| AC-3 | 参加していない・epoch が合わない・退出後は、provider への取得と保存が 0。channel の違う投稿、送信者と署名者の不一致、改ざんした本文は保存しない | 取得回数 0、通知 0 |

維持する条件: INVAR-1 invite_only/friend_only/friend_plus の参加と epoch の capability を要求前と反映前に確かめる。
INVAR-2 公開の route の offer へ private の本文・hash・channel を平文で載せない。INVAR-3 既存の通知 ID・OS 設定・
成人向け preview の判定を変えない。

## 入口と副作用

| ID | 対応 | 入口 → helper | 期待結果 / 禁止する副作用 |
| --- | --- | --- | --- |
| T1 | AC-1 | `create_post_with_attachments_in_channel`（private）→ `queue_post_offer` → 送信 worker → `publish_receive_offer` | 宛先は mention と返信先の著者。自分は除く。epoch の鍵で暗号化 |
| T2 | AC-2/3 | account の受信 route → `ingest_account_receive_offer` → `ingest_private_notification_offer` | 照合できない offer は I/O 0。取得と保存は参加の世代が続く間だけ |

## 実装の照合（監査前）

- `public_notification_offer_support.rs`: `queue_post_offer` が公開と private の投稿・返信の offer を 1 か所で作る。private は
  manifest を投稿した epoch の鍵で暗号化し、`PrivateSource { epoch_key_id }` を scope にする。送信 worker と待機 64 件、
  1 宛先 2 秒は R4-B のまま（queue の要素に scope を加えた）。受信は `ingest_private_notification_offer` が参加中 channel の
  現在 epoch の識別子で照合し、`until_content_invalid` の中で取得、`content_save_access` の下で世代を確かめて保存する。
- 投稿の検証は公開と private で共通の `post_notification_candidate`（送信者と署名者、scope と channel、本文 hash、返信先）に
  まとめ、private 専用の候補生成を作らない。旧購読の通知生成の撤去は R2-C。
- 照合は参加中 channel の現在 epoch だけ（過去 epoch の識別子は計算しない）。

## 検証（局所）

- 変更前に失敗する条件: `PrivateSource` の offer は旧実装では受信側で捨てられ（`_ => Ok(false)`）、private の投稿は offer を
  送らなかった。
- 単体 `private_receive_offer`: mention と reply が 1 件ずつ・再受信で増えない、未参加と epoch 不一致で provider の取得 0、
  別 channel の投稿・署名者と送信者の不一致・改ざん本文は保存 0。R4-B の `public_receive_offer` 4 件も成功。
- 実 Iroh `real_private_offer_reaches_a_member_without_channel_sync`: channel を購読しない参加者へ account の route で
  mention が届き、channel 外の account は通知 0。`receive_offer`・`private_channels`・`notification` の関連 51 件、
  app-api lib 全 513 件が成功。
- app-api の `cargo clippy --all-targets -D warnings`（`iroh-integration-tests` つき）、`cargo fmt --check`、
  desktop-runtime・cli・harness の `cargo check --all-targets`。`oversized-files` は `timeline.rs` の offer 呼出しの整形で
  1015→1019（R5-C で下げた値からの増加）へ baseline を更新。全体 suite は PR の CI。
