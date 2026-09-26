# #1221 R2-C: client の購読・参加・再構築を 64 active scope へ統合

## 完了境界

正本は [#1221](https://github.com/kukuri-app/kukuri/issues/1221) の R2-C と、2026-09-26 のユーザー決定「R2-C 購読の対象と上限」。
基準は #1372 merge `ce5fd338`。依存の R4-D・R5-B・R5-C は完了済み。

- 購読する scope を 4 種類だけにする: (1) workspace に開いている列の topic/channel、(2) 参加中の private channel(現 epoch)と
  参加中の live/Dome、(3) profile 画面か DM 会話を表示している相手の author、(4) CLI daemon の desired(保存は 64 件まで)。
- 同時に購読する key(`Topic`・`Channel`・`Author`)は account あたり 64 件。超える列の追加・参加は拒否し、画面で説明する。
- 撤去: `restart_active_subscriptions` と 2 つの呼出し、読み書きの操作からの暗黙の購読の開始、起動時の follow/block 全員の
  author 購読、private channel の過去 epoch ごとの購読 task、所有者の無い Dome heartbeat の `tokio::spawn`、上限の無い task map。
- 維持: account 受信(R4)、DM outbox、session 表示の observer、通知の既存経路、CN rendezvous、`Direct → Relay Supported → Relay Fallback`。

対象外(R5-H): docs の旧定常 sync そのもの(`start_sync`・`reapply_sync_peers`・`restart_replica_sync` と recovery tick の中身)、
legacy reader / worker selector、`worker.rs` の ReplicaEvent queue。R2-C は lease の無い対象の replica を閉じることだけを保証する。

## 受入条件

| ID | 対象・期待結果 | 判定(試験) |
| --- | --- | --- |
| AC-1 | key は最大 64。超える取得は `ScopeLimitReached` で何も取らない。複数 key の要求も部分的に取らない。既存 key の共有は枠を使わない | app-api `the_ledger_holds_at_most_64_keys_and_never_takes_part_of_a_request`・`the_65th_column_is_rejected_without_subscribing` |
| AC-2 | task・hint・replica の購読は lease のある key だけ。最後の holder が外れたら abort・`unsubscribe_hints`・`close_replica`。読み書きは購読を開始しない | `reads_and_writes_do_not_start_subscriptions`・`the_last_holder_stops_the_task_leaves_hints_and_closes_the_replica` |
| AC-3 | `unsubscribe_topic` は desired の holder だけを外し、開いている列と参加は止めない。列を閉じると止まる。列を閉じても参加(live・Dome)の購読は続く。live 退出は参加の holder だけを外す | `leaving_a_screen_and_ending_participation_are_separate` |
| AC-4 | 起動時は参加中 channel の現 epoch と desired だけを購読する。休止履歴 10 倍でも task・hint topic・open replica は lease の数 | `startup_with_ten_times_the_dormant_history_subscribes_only_the_leases`、desktop-runtime `desired_subscriptions_are_bounded_and_share_the_scope_limit` |
| AC-5 | endpoint の世代が変わった時だけ lease の task を作り直す。seed・ticket では作り直さない | `endpoint_rebuild_recreates_only_leased_tasks_and_peer_changes_do_not`、desktop-runtime `endpoint_rebuild_resubscribes_only_leased_topics` |
| AC-6 | live 参加・Dome hosting は参加の holder。shutdown 後に presence・heartbeat・購読 task が動かない | `shutdown_leaves_no_presence_heartbeat_or_subscription_running`・`participation_over_the_limit_is_refused_without_saving_state` |
| AC-7 | 購読していない topic への publish は短期送信先 16 件まで。lease の hint 購読は抜けない | transport `publishing_to_unsubscribed_topics_keeps_at_most_16_short_term_topics` |
| AC-8 | `set_scope_display` と Tauri・`DesktopApi`・mock。frontend は列の差分だけを登録・解除し、上限の列を閉じて説明する | frontend `columnScopeLeases.test.tsx` の 2 件 |
| AC-9 | private channel の参加は参加の holder で channel key。上限なら状態を保存しない。退出で外す。回転が続いても開く replica は現在と直前の 2 つまで | `participation_over_the_limit_is_refused_without_saving_state`・`a_rotating_channel_keeps_only_the_current_and_previous_replicas_open` |
| AC-10 | desired は 64 件まで。65 件目は上限エラー。64 件を超える file は先頭 64 件。上限は lease と共通 | desktop-runtime `desired_subscriptions_are_bounded_and_share_the_scope_limit`、CLI `the_scope_limit_is_a_validation_failure` |

## 入口と副作用

| 入口 | helper | 結果 |
| --- | --- | --- |
| GUI の列(`useColumnScopeLeases`)→ `set_scope_display` | `set_scope_holder("display:<列 id>", keys)` | 列の追加で key を取り、閉じた列だけを外す。上限なら列を閉じてダイアログ |
| live 参加 / 退出・終了 | `live_holder` の取得 / `stop_live_presence_task` での解放 | 上限なら presence を始めない。終了した session は presence task が自分で解放する |
| Dome hosting の開始 / 終了・移譲・削除・shutdown | `dome_holder`、`stop_owner_dome_hosting`、`dome_heartbeats` | heartbeat の handle を registry が持つ |
| private channel の作成・参加(import) / 退出 | `hold_joined_private_channel` / `remove_joined_private_channel` | 何かを保存する前に取る。失敗した import は取った holder を外す |
| 起動時の capability 復元・desired 復元 | `restore_private_channel_capability`・`ClientHost::restore_desired_subscriptions` | 上限を超えた分は購読せず warn |
| endpoint の作り直し | `rebuild_scope_subscriptions` | 秘密を新しい docs へ登録し直し、lease の key の task だけを作り直す |
| gossip の停止・再開 | `restart_topic_scope_subscriptions` | lease は残し、task だけを止める・起こす |

## 実装の照合

- app-api: `service/scope_leases.rs`(`ScopeLeases`・`ScopeKey`・`ScopeLimitReached`・holder 名・`stop_scope_task`)。
  `SubscriptionRegistry` の topic/channel/author の task map 3 つを `scope_leases` へ置き換え、`dome_heartbeats` を足した。
  購読 task の起動(`spawn_subscription_task`・`spawn_author_subscription`)は `ScopeTask` を返すだけにし、登録は台帳が行う。
  `ensure_topic_subscription`・`ensure_author_subscription(s_for_rows)`・`ensure_scope_subscriptions`・
  `ensure_(joined_)private_channel_subscription(s)`・`restart_*_subscription`・`unsubscribe_private_channel`・`warm_social_graph` を削除
  (block の Dome 接続の解除は `reconcile_blocked_dome_connections_at_start` に残した)。`sync.rs` に `set_scope_display`・`set_desired_scope`。
  Dome の heartbeat と hosting の終了は `service/dome_host_ownership.rs`(heartbeat の handle を `dome_heartbeats` が持つ)。
  private channel の epoch が変わったら task を現 epoch へ作り直す。直前の epoch の replica は 1 つだけ開いたまま覚え(参加者が
  handoff の grant を同期で受け取るため)、2 世代前の replica を閉じる。最後の holder が外れたら現在と直前の両方を閉じる。Dome の移動は、次の段が projection から読む staging を自分で projection へ置く
  (購読 task の反映を待たない)。公開 topic の Dome 操作・入場の権限は、購読の有無ではなく gossip を止めていないことで判定する。
- transport: `short_term_topics`(`MAX_SHORT_TERM_HINT_TOPICS = 16`)。
- desktop-runtime: desired の holder、64 件の上限(`SubscriptionStateErrorKind::LimitReached`)、起動時の warn、世代が変わった時の
  `rebuild_scope_subscriptions`、`set_scope_display`。
- Tauri: `set_scope_display` command、`ScopeLimitReached` を code `SCOPE_LIMIT_REACHED` へ。CLI: 上限を `validation_failed` へ。
  `command-parity.json` は `set_scope_display` を `gui_visible_membership` の除外に分類(CLI は desired で同じ上限を扱う)。
- frontend: `shell/columnScopeLeases.ts`(列の購読の対象・差分の登録・参加の失敗の説明)と `shell/page/ColumnScopeLeases.tsx`(ダイアログ。
  列を管理する Control Center が描画する)。`setSessionDisplay` と `setScopeDisplay` を `commands/displayDemandApi.ts` にまとめた。
  文言は `shell.scopeLimit`(ja・en・zh-CN)。
- 文書: ADR 0055 §1.1。

## 検証(局所)

- 変更前に失敗する条件: 読み込み(timeline・thread・profile)と書き込みが topic・author の購読を始め、起動時に follow/block の全員と
  private channel の全 epoch を購読していた。seed の変更と ticket の取込みが全購読を作り直し、shutdown 後も Dome の heartbeat が
  動いていた。購読の数に上限が無く、購読していない topic への publish は topic に入ったまま抜けなかった。
- app-api `cargo test -p kukuri-app-api --lib` 510 件成功、`--features iroh-integration-tests` 557 件成功。
  `scope_leases.rs` の 10 件(AC-1〜AC-6・AC-9)。
- transport `cargo test -p kukuri-transport --lib` 135 件成功(AC-7 の 1 件を含む)。
- desktop-runtime `cargo test -p kukuri-desktop-runtime --lib` 317 件成功(`host::tests` の AC-4/5/10 の 2 件、lock 分類の試験を含む)。
- harness `cargo test -p kukuri-harness --lib -- --test-threads=1` 23 件成功。CLI `cargo test -p kukuri-cli --no-fail-fast` 全件成功
  (`process_e2e` は Linux 限定のため Windows では 0 件)。
- frontend `vitest run` 255 file・2,059 件成功(`columnScopeLeases.test.tsx` の 2 件を含む)、`tsc --noEmit`、変更 file の eslint。
  `cargo xtask ipc-types` の生成結果は手で足した `ScopeDisplayRequest`・`ScopeDisplayTarget` と一致。
- `cargo clippy --all-targets -D warnings`(app-api は iroh 結合試験の feature でも。transport・desktop-runtime・cli・harness)、
  `cargo fmt --check`、`cargo xtask oversized-files`(違反 0)。Tauri crate は一時的に `[workspace]` を足して `cargo check --locked` が成功
  (試験の実行は、この端末では test exe の起動が `STATUS_ENTRYPOINT_NOT_FOUND` になり未実行)。
- mutation check(外すと失敗することを確認し、元に戻した): 上限の判定(AC-1 の 3 件)、timeline の読込での購読の開始(AC-2)、
  最後の holder での hint・replica の後始末(AC-2)、`unsubscribe_topic` が参加も外す・列の holder も外す(AC-3)、
  回転で 2 世代前の replica を閉じない・最後の holder で直前の replica を閉じない(AC-9)、起動時の block 相手の購読(AC-4)、
  作り直しで task を作らない(AC-5 の app-api と desktop-runtime)、shutdown で heartbeat を止めない(AC-6)、短期送信先を数えない(AC-7)、
  channel の作成で参加の holder を取らない(AC-9)、desired の file を 64 件へ切らない(AC-10)、frontend の列の取消と閉じた列の解除(AC-8)。
- 試験の書き換え: 読み込みで購読していた試験は、列を開く(`display_topic`・`display_author`、desktop-runtime・harness の
  `open_topic_column`・`open_profile_column`)か CLI の desired に置き換えた。seed・ticket で購読を作り直すことを固定していた試験は、
  作り直さないことの試験へ改めた。
- 未実行: 全体の CI、Linux の `process_e2e`、Tauri crate の試験の実行。

## 既知の制約

- 公開 topic の Dome 操作と入場の権限は、購読の有無ではなく、その topic の gossip を止めていないことで判定する(R2-C 以前の実効的な判定と同じ)。
  列を閉じた状態や CLI からの Dome 操作も、gossip を止めていなければ扱える。
- block による Dome 接続の解除は、lease のある topic と参加中の channel の context だけを対象にする。閉じている topic の Dome 接続は、
  その topic を開くか起動時の解除まで残る。
- author は profile か DM 会話を表示している間だけ購読する。手元の DB を失った後の follow の関係は、その author の profile を開くか、
  follow の offer が届いた時に戻る(起動時に follow の全員を購読しない)。
