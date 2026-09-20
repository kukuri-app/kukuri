# #1258 アカウントの署名鍵から導出した docs author で、著者の投稿と取り下げを読む（2026-09-21）

- 対象 Issue: #1258（区分 C、Scope revision 2026-09-21-v2、基準 commit `515d8c18`）。#1250・#1248 の後続。`main`（`14c8c596`。#1239 の PR #1247 を含む）を取り込み済み。
- 仕様: [ADR 0053](../adr/0053-account-derived-docs-author.md)（新設）。[ADR 0052](../adr/0052-scale-independent-timeline-sync.md) §2 の「未決」を ADR 0053 への参照へ置き換えた。
- 計画: `.claude/plans/2026-09-21-issue-1258-account-derived-docs-author.md`。

## 調査の結論

- iroh-docs の entry は（namespace、docs author、key）の組で、docs author の署名を持つ。同期で受け取る entry は `validate_entry` が namespace・署名・timestamp だけを検査し、
  アプリ側の判定を挟む口は無い。他の docs author の entry は消せない。検証に通らない record の伝播を client が止める手段は無い。
- 書き込みは端末ごとの既定の docs author（乱数の鍵）で行い、アカウントの署名鍵との対応はどこにも無かった。`DocRecord`・`DocKeyEntry`・`DocEvent` は docs author を持たなかった。
  読む側は key の列を先頭から上限つき（8 件）で調べるしかなく、9 件以上の不正な record を小さい docs author id で積まれると、著者の取り下げと投稿は反映されなかった。
- iroh-docs 0.101 には `Author::from_bytes`・`author_import`・`author_set_default` と、（docs author、key）の主 key の 1 件引き `Doc::get_exact` がある。
- アカウントの切り替えと復元は runtime を作り直す（`host::switch_account` → `build_detached_runtime`。docs の保存場所はアカウントごと）。docs author の設定は runtime の起動時の 1 箇所に置ける。
  `IrohDocsSync::new` の本番の呼び出しは `desktop-runtime/src/stack.rs` だけで、CLI と harness も同じ runtime を通る。CN indexer と `cn-e2e` は読む側で、アカウントを持たない。

## 実装

- 導出（ADR 0053 §1）: `KukuriKeys::derive_docs_author_seed`（`crates/core/src/crypto.rs`）。`blake3::derive_key("kukuri.app 2026-09-21 docs author v1", <秘密鍵>)`。
  戻り値の `DocsAuthorSeed` は `Debug` で中身を出さない。固定値の test（`docs_author_seed_matches_golden`、`account_docs_author_is_the_same_on_every_device_of_the_account`）で保護する。
  docs author の id の固定値は、Node の `crypto` で種から独立に計算した ed25519 の公開鍵と一致することを確かめた。
- 設定: `IrohDocsSync::use_account_docs_author`（`crates/docs-sync/src/iroh_sync.rs`）が author を import して既定にする。`SharedIrohStack::use_account_docs_author` が種を保持し、
  `rebuild` は作り直した stack へ、差し替える前に設定し直す。runtime は鍵を読んだ直後、docs へ何かを書く前に呼ぶ（`crates/desktop-runtime/src/runtime/mod.rs`）。
- 旧い名義の entry（実装中の発見）: 既定の docs author を切り替えた後に同じ key を書き直すと、その key に新旧 2 つの名義の entry が並ぶ。投稿と取り下げ以外の key（profile、session の state など）には
  「key を指定して先頭の 1 件を読む」読み手が残っており、旧い値を読みうる。`apply_doc_op` は書き込みのたびに、旧い名義が同じ key に持つ entry を消す（`supersede_legacy_entry`。旧い名義ごとに
  1 件引き）。`DeletePrefix` は旧い名義でも行う。端末ごとの鍵を残す決定は、この tombstone を書くために必要だった。
- `DocsSync`: `local_docs_author`（既定は `None`）と `query_replica_by_author`（既定はエラー）を足した。`DocRecord`・`DocKeyEntry`・`DocEvent`・`TimeIndexEntry` は、書いた docs author の id を運ぶ。
  `MemoryDocsSync::with_docs_author` は 1 つの名義で書く docs。`ReloadableDocsSync` は 2 つの method を転送する。
- 書く側: `build_post_envelope_with_docs_author`・`build_repost_envelope_with_docs_author`（`crates/core/src/posts.rs`）が署名の対象の tag `docs_author` を入れる。`KukuriEnvelope::docs_author` が読む
  （id の形でない値は無視）。`HintObjectRef::docs_author` は任意の field（無ければ出力しない）。`app-api` の投稿・repost・取り下げの hint が `local_docs_author` を入れる。
- 読む側（ADR 0053 §3）:
  - 投稿: `load_post_with_hint`（`post_integrity.rs`）。手がかりがあれば「docs author と key の組」で 1 件読み、検証に通ればそれを使う。それ以外は key だけの上限つきの読み出し。
    手がかりの出所は、docs の event（`hydrate_subscription_doc_event`）、hint（`hydrate_subscription_hint`）、索引の entry（`ensure_index_entries_projected`）。
  - 取り下げ: `hydrate_post_withdrawal_for_object_with_hints`（`post_withdrawal_hydration.rs`）。対象の投稿の docs author（検証済みの envelope の tag、または行の列 `source_docs_author`）と、
    取り下げを書いた docs author の手がかり（event・hint）の組で 1 件ずつ読む。反映できなければ key だけの上限つきの読み出し。検証（`verify_post_withdrawal`）と sink（`apply_verified_post_withdrawal`）の位置は変えていない。
  - `hydrate_object_in_topic_with` は、envelope を先に読んで検証し、その tag の docs author で取り下げを読んでから行を書く（取り下げを行より先に反映する順序は保つ）。検証に通る envelope が無いときは取り下げを読まない。
- projection: `object_index_cache.source_docs_author`（migration `20260921020000`）。列が空の行は旧 record として扱い、反映し直さない。

## 修正前の再現

`crates/app-api/src/tests/sync/docs_author_reads.rs`。test double の `ShadowingDocsSync`（`shadowing_docs.rs` へ切り出し）を、docs author つきの docs として使う。
「docs author と key の組」の読み出しを無効にする（= 修正前の key だけの読み出し）と、不正な record を 9 件積む 5 件が失敗し、有効にすると成功する。不変条件の 4 件はどちらでも成功する。

## AC / INVAR と test

| 条件 | transition | test |
| --- | --- | --- |
| AC-1 | — | ADR 0053、ADR 0052 §2、`docs/legal/app-data-flow-inventory.md` |
| AC-2、INVAR-5 | TR-1・TR-2 | `crates/docs-sync/src/tests/docs_author.rs` の `account_docs_author_is_the_same_on_every_device_of_the_account`・`entries_of_the_previous_docs_author_are_superseded`、`crates/desktop-runtime/src/stack.rs` の `account_docs_author_survives_a_stack_rebuild`、`crates/core` の `docs_author_seed_matches_golden` |
| AC-3 | TR-3・TR-10 | `every_entry_applies_a_withdrawal_behind_a_flooded_withdrawal_key`（object・event・hint・background）、`writer_hint_applies_a_withdrawal_of_a_post_without_the_tag` |
| AC-4 | TR-5 | `doc_event_projects_a_post_behind_a_flooded_envelope_key`、`hint_projects_a_post_behind_a_flooded_envelope_key`、`index_entry_docs_author_projects_a_post_behind_a_flooded_envelope_key`、`created_post_declares_the_local_docs_author` |
| AC-5 | TR-8 | `reads_do_not_grow_with_the_records_piled_on_the_keys`（9 件と 90 件で同じ読み出し）、`read_by_docs_author_returns_one_record_among_many_authors`（実 iroh-docs） |
| AC-6 | TR-7・TR-9 | 既存の #1248・#1250 の上限つきの読み出しの test、`post_envelope_declares_the_docs_author_in_a_signed_tag`、`gossip_hint_object_ref_docs_author_is_optional_on_the_wire` |
| INVAR-1・2 | TR-4 | `withdrawal_written_by_another_docs_author_does_not_hide_the_post` |
| INVAR-3 | TR-6 | `false_docs_author_hint_never_projects_an_unverified_value` |
| INVAR-4 | TR-11 | `read_failure_by_docs_author_is_an_error` |
| INVAR-6 | TR-6（Issue） | `private_channel_hint_does_not_carry_the_docs_author`。docs author の id を置く場所は、投稿の envelope・docs の entry（その replica の中）と、public topic の hint だけ |

## inventory

Issue #1258 の INV-1〜INV-10。INV-8（`DocsSync` の implementor）は、`rg "impl DocsSync for"` で 18。新しい読み出しを実装または転送するのは `IrohDocsSync`・`MemoryDocsSync`・`ReloadableDocsSync`・`ShadowingDocsSync`。
それ以外の test double は docs author を持たない `MemoryDocsSync` を包んでおり、`local_docs_author` が `None` なので tag も手がかりも生まれず、新しい読み出しは呼ばれない。
INV-10（CN indexer）は実装を変えていない。`ReferenceGuard` は `verify()` と `to_post_object()` の値を使い、tag の集合は照合しない。

## 独立監査の指摘への対応

- N-1（INVAR-6）: private channel の hint の topic（`hint/private/<channel id>`）は epoch の秘密に依存せず、現在の epoch を同期できない者にも届きうる。投稿と取り下げの hint の `docs_author` は、
  public topic のときだけ入れる形にした（`crates/app-api/src/timeline.rs`）。channel の参加者は、docs の event と索引の entry から同じ手がかりを得る。
  test は `private_channel_hint_does_not_carry_the_docs_author`（filter を外すと失敗する）。ADR 0053 §2 も同じ内容にした。
- N-2: ADR 0053 の「バックアップの対象にしない」は誤りだった。端末バックアップは iroh-docs の保存場所ごと暗号化して運ぶので、導出した鍵も暗号化された backup に含まれる。ADR の記述を直した。
- N-3〜N-8 は non-blocker のまま（監査記録を参照）。

## 残る限界

- 旧 record（tag も手がかりも無い投稿と、その取り下げ）は、上限つきの読み出しのまま（決定済み）。
- 手がかりを持たない入口（利用者の操作からの key 指定の反映、返信先の preview、repost 元の解決、hint の `ThreadUpdated`）は、投稿の envelope を key だけの上限つきの読み出しで読む。
- 索引や replica へ偽の entry を積まれる問題と replica の肥大は対象外（#1243、CN の責務）。
