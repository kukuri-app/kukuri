# #1252 reaction・live session・game room の反映で署名と replica を確かめる（2026-09-21）

- 対象 Issue: #1252（区分 C、Scope revision 2026-09-21、基準 commit `bed95a98`）。先行は #1248（投稿の検証）。関連は #1239（同じ file を変更中）。
- 仕様: [ADR 0052](../adr/0052-scale-independent-timeline-sync.md) §2（reaction・live session・game room の検証の規則を追記）、ADR 0005・0006・0017（参照を追記）。
  inventory: [replica の読み出しの inventory](../architecture/replica-read-inventory.md)（「反映の検証が足す読み出し」）。
- 計画: `.claude/plans/2026-09-21-issue-1252-session-reaction-integrity.md`。

## 調査の結論

- reaction・live session・game room の反映は、署名の無い state doc（`reactions/<target>/<reaction id>/state`・`sessions/live/<id>/state`・`sessions/game/<id>/state`）の
  申告値をそのまま行にしていた。public topic の replica は topic id を知る誰もが書ける（`public_replica_secret`）。
- reaction は同じ replica に署名つきの `.../envelope` があるのに、読んでいなかった。
- live session と game room は、docs に署名つきの record が 1 つも無かった。`build_live_session_envelope` / `build_game_session_envelope` で作った envelope は
  `envelope.id` を `state.last_envelope_id` に入れるだけで、envelope 自体は docs にも store にも置いていなかった。owner の確認は書く側にしか無かった。
- どの prefix でも、読めない record が 1 件あると `?` で `hydrate_subscription_state` 全体が失敗した。
- public replica から読んだ reaction が private channel の投稿に数えられる経路は、修正前から無かった。reaction の行は `source_replica_id` を key に持ち、
  表示は投稿の行の source replica で引く（`list_reaction_cache_for_targets`・`reaction_state_for_target`）。
- `docs/adr/` に、検証しないとする設計判断の記録は無かった（ADR 0017 は projection を signed `reaction` object から再構築できるものと定め、ADR 0006 は
  「room 更新は owner のみ」と定める）。
- metaverse room は、訪問者（owner 以外）も chat で room の manifest を書く（`publish_metaverse_room_event`）。owner の署名を一律に要求すると、この既存設計が壊れる。
  一覧は以前から、署名つきの Dome Instance（`hosting_instance`）で room を確かめている。

## 修正前の再現

`crates/app-api/src/tests/sync/hydration_integrity_sessions.rs`。`bed95a98` の上で 11 件中 10 件が失敗し、R-b だけが成功した（Issue 本文の表のとおり）。
修正に合わせて、(b) の攻撃者の record は「攻撃者自身の鍵で正しく署名し、id も攻撃者の pubkey に結び付けたもの」に、(c) の隣の正しい session は
「owner が署名したもの」に改めた（署名の検証だけでは (b) を防げないことと、読めない record の隣の正しい record が反映されることを確かめるため）。修正後は 11 件とも成功する。

## 変更の要約

- `service/reaction_integrity.rs`（新設）: `VerifiedReaction`。`verify`（envelope と scope から。docs を読まない）、`verify_local`（自分の reaction）、
  `load_verified_reaction`（envelope の key を最大 8 record 読み、検証に通るもののうち署名時刻が最も新しい 1 件）。行は `reaction_projection_row(&VerifiedReaction)` からしか作れない。
  反映済みの行より古い envelope では行を戻さない。`state` と `envelope` のどちらの event でも個別反映を試す。全件走査と対象単位の hint は、同じ prefix の読み出しに含まれる
  envelope の record から行を作る（読む量は増えない）。
- `service/reaction_hydration.rs`（新設）: reaction の反映（全件走査・key 指定・対象単位）を `hydration_support.rs` から分けた（1,000 行の上限のため）。
- `service/session_integrity.rs`（新設）: `VerifiedLiveSession`・`VerifiedGameRoom`。書く側（`persist_live_session_manifest`・`persist_game_room_manifest`）が manifest 全体に署名した
  envelope を `envelopes/<envelope id>` へ state より先に置き、`state.last_envelope_id` がそれを指す。呼び出し側が id のためだけに作っていた envelope（12 か所）は削除した。
  読む側の確認は ADR 0052 §2 のとおり（署名者 == owner、id と owner の結び付け、state・manifest blob と署名された manifest の一致、scope の一致。metaverse room は id と
  Spatial Context・owner の結び付け）。行は `live_projection_row(&VerifiedLiveSession)`・`game_projection_row(&VerifiedGameRoom)` からしか作れない。
- 操作の読み出し: `fetch_live_session_state_and_manifest`・`fetch_game_room_state_and_manifest`（caller は live 3・game 5・dome_move 6・dome_hosting 2）を同じ検証へ通した。
  `fetch_live_session_state_from_replica`・`fetch_game_room_state_from_replica`（先頭 1 件を無検証で読む）は削除した。
- ScoreGame の projection の直列化（ADR 0006）: 候補が最新かどうかの確認を「同じ key の先頭の 1 件との一致」から「同じ key の record（上限つき）のどれかとの一致」に変えた。
  lock の単位と、canonical 比較の後に commit する順序は変えていない。
- 新しく作る live session・ScoreGame の id の末尾を、pubkey の先頭 8 桁から 16 桁にした（`owner_bound_id_suffix`）。
- store: `VERIFIED_REACTION_PROJECTION_VERSION`・`VERIFIED_SESSION_PROJECTION_VERSION`（2）。migration `20260921010000_drop_unverified_reaction_session_projections` が
  旧い reaction・live session・game room の行を消す。正しい record は、購読の起動時の反映で手元の docs から戻る。
- `dome_hosting.rs` の `hosting_authority_view`: `if let` の条件式の中で `dome_host_heartbeats` の lock を取り、その then 節の中で同じ lock を取り直す既存の deadlock を直した
  （lock の取得を文として分けた）。検証に通らない heartbeat が先に届く順序でだけ踏む競合で、`main` では `owner_explicitly_transfers_hosting_to_one_community_node` が
  0.06 秒で終わっていたが、本差分で処理の順序が変わり、同じ test が終わらなくなったことで見つかった。

## 既存 test の期待値の変更

- `game_projection_freshness.rs`: test の「owner の別端末からの書き込み」helper（`publish_state`）を、署名つきの envelope を置く形にした。
  `missing_and_corrupt_canonical_state_do_not_apply_the_candidate` は、読めない record の反映が `Err` ではなく `Ok(false)` になる（AC-4）。行が変わらないことの assert は変えていない。
- `hint_rehydration.rs`: 削除した `fetch_live_session_state_from_replica` の代わりに、test の中で state を直接読む。
- `crates/store/src/tests/migrations*.rs`: migration の世代数（25 → 26）。

## 互換と利用者への影響

- 修正前の client が書いた live session と ScoreGame は、署名つきの envelope を持たないので、修正後の client には表示されない。owner が修正後の client で更新（終了・score の更新）すると表示される。
  Dome（metaverse room）は、修正前の client が書いたものも読める。
- state doc の形は変えていない。修正前の client は、修正後の client が書いた record をこれまでどおり読める。

## inventory の逆引き（修正後）

- `reaction_projection_row` の caller: `hydration_support.rs`（`hydrate_reaction_cache_from_reaction`）、`reactions.rs`（`toggle_reaction`。`verify_local` を通す）。
- `live_projection_row` の caller: `hydration_support.rs`（`hydrate_verified_live_session`）、`live.rs`（作成・終了。`persist_live_session_manifest` の戻り値）。
- `game_projection_row` の caller: `game_projection_support.rs`、`game.rs`（6）・`dome_move.rs`（3）・`dome_delete.rs`（1）（すべて `persist_game_room_manifest` の戻り値）。
- どの builder も引数が検証済みの型なので、未検証の値からは行を作れない。`ReactionDocV1`・`LiveSessionStateDocV1`・`GameRoomStateDocV1` を docs の record から parse する箇所は、
  app-api の本体では `reaction_integrity.rs`（envelope から `parse_reaction` 経由）と `session_integrity.rs` だけ
  （`live_game_support.rs`・`object_persistence_support.rs` は書く側で、値を組み立てて書くだけ）。
- 列挙方法: `rg "reaction_projection_row|live_projection_row|game_projection_row|upsert_(reaction|live_session|game_room)_cache|LiveSessionStateDocV1|GameRoomStateDocV1" crates --glob '!**/tests/**'`。未分類 0。

## 検証（ローカル、Windows）

- `cargo xtask rust-test`: nextest 1,154 件成功・6 件 skip、doc test 成功。
- `cargo nextest run -p kukuri-app-api --lib`: 278 件成功（修正前の再現 11 件 + 契約 9 件を含む）。`-p kukuri-store --lib`: migration の round trip と、旧い行を消す test を含めて成功。
- `cargo xtask check`: `cargo fmt --check` と workspace の `cargo clippy --all-targets -- -D warnings` は成功。続く `apps/desktop/src-tauri` の `cargo check` は、
  git worktree で実行したため workspace の解決で失敗した（差分と無関係。CI で確認する）。
- `cargo xtask oversized-files`: 成功（`game.rs` が上限を下回ったという note だけ）。
- 未実行: `cargo xtask app-api-slow-test`（実際の iroh-docs を通す統合 test）と `cargo xtask e2e-smoke`。CI の結果で確認する。

## 残る事項（本 Issue の対象外）

- live session・ScoreGame の古い署名つき state の再掲: owner が過去に署名した manifest と state を置き直すと、表示が過去の状態（終了前など）へ戻りうる。
  reaction は署名時刻で防いだが、session は `state.updated_at` が署名の対象外で、envelope の `created_at` は秒単位。署名時刻を ms にして行の新旧の基準にする変更は、
  固定 AC の外（owner の署名で裏づけられた値ではある）なので入れていない。
- 修正前の id（末尾 8 桁）の session は、id と owner の結び付けが 32 bit に留まる。
- metaverse room の title などの表示内容は、その topic に書ける者が変えられる（訪問者が manifest を書く設計のため。Dome の authority は ADR 0035 / 0036）。
  `hosting_instance` は `metaverse/dome-instances/` を prefix で読み、読めない record 1 件で `?` により失敗する（inventory の P-13。Dome の読み出しの見直しで扱う）。
- `hydrate_reaction_cache_for_target` は、1 投稿の reaction を prefix で全件読む（inventory の P-4。#1239 が所有）。
