# #1248 投稿の反映で署名つき envelope と replica を確かめる（2026-09-21）

- 対象 Issue: #1248（区分 C、Scope revision 2026-09-21-v1、基準 commit `0521a38b`）。関連は #1239（同じ file を変更中）。
- 仕様: [ADR 0052](../adr/0052-scale-independent-timeline-sync.md) §2（投稿の検証の規則を追記）。inventory: [replica の読み出しの inventory](../architecture/replica-read-inventory.md)（V-1・V-2）。
- 計画: `.claude/plans/2026-09-21-issue-1248-post-hydration-integrity.md`。

## 調査の結論

- docs の replica から projection へ投稿を反映する経路（`hydrate_object_projection_from_record`・`hydrate_object_projection_from_replica`）は、`objects/<object id>/state` の
  header をそのまま行にしていた。同じ object の署名つき envelope を読まず、`KukuriEnvelope::verify()` も呼ばない。行の `channel_id`・`topic_id` は header の申告値で、
  読んだ replica と照らしていなかった。通知・repost の snapshot・bookmark の添付・返信先の preview・取り下げ済みの行の作り直しも、同じ header を検証せずに使っていた。
- public topic の replica の namespace secret は replica id から決まる（`crates/docs-sync/src/replicas.rs` の `public_replica_secret`）ので、topic id を知る誰もが書ける。
- `docs/adr/` に「header を検証しない」という設計判断の記録は無かった。ADR 0011 §13.1.1・§14.1 は署名の検証を前提にし、ADR 0012 §4.5 は private replica の capability を同期の gate と定める。
  CN indexer は同等の検証を `ReferenceGuard::check_current`（`crates/cn-indexer/src/ingest/reference_guard.rs`）で行っており、author replica の反映（`profile_docs_support.rs`）も
  doc と envelope を突き合わせている。投稿の反映だけが例外だった。
- iroh-docs の key 指定の query は、同じ key の entry を docs author ごとに全部返す（`exact_bounded_query_limits_the_entries_of_one_key` で確認）。呼び出し側は先頭の 1 件だけを使っていた。

## 修正前の再現

`crates/app-api/src/tests/sync/hydration_integrity.rs`。`MemoryDocsSync` の replica へ `DocOp::SetJson` で直接書く。`0521a38b` では次の 7 件が失敗し、対照の 1 件が成功した。

| 記号 | 置く record | 修正前の結果 | test |
| --- | --- | --- | --- |
| a-1 | 攻撃者の鍵で署名した envelope と、`author` だけを被害者の pubkey に書き換えた header | 被害者の投稿としてタイムラインに出る | `header_author_that_differs_from_the_signed_envelope_is_not_shown_as_that_author` |
| a-2 | envelope なしの header だけ | タイムラインに出る | `header_without_a_signed_envelope_is_not_projected` |
| a-3 | 正しい envelope と、`payload_ref` を書き換えた header | 署名していない本文が著者の投稿として出る | `header_payload_that_differs_from_the_signed_envelope_is_not_shown` |
| b | public replica に、private channel の id を申告する正しく署名された投稿 | その private channel のタイムラインに出る（署名の検証だけでは防げない） | `post_in_the_public_replica_that_claims_a_private_channel_is_not_listed_in_the_channel` |
| c | topic A の replica に、topic B を申告する投稿 | topic B のタイムラインに出る | `post_that_claims_another_topic_is_not_listed_in_that_topic` |
| d | 自分の投稿への reply を装い、`author` を被害者にした header | 被害者からの通知ができる | `forged_reply_header_does_not_create_a_notification_from_the_claimed_author` |
| e | `state` として parse できない record を 1 件 | 反映が空の利用者の `list_timeline` が `missing field object_id` で失敗する | `unreadable_state_record_does_not_break_the_timeline_of_the_topic` |
| 対照 | 正しい envelope と、それに一致する header | 反映される | `consistent_post_is_still_projected` |

修正後は 8 件とも成功する。

## 変更の要約

- `crates/app-api/src/service/post_integrity.rs`（新設）: 検証済みの投稿 `VerifiedPost`。作り方は `VerifiedPost::verify`（envelope と scope から。docs を読まない）、
  `VerifiedPost::verify_local`（自分で署名した投稿）、`load_verified_post`（`objects/<id>/envelope` を key 指定で最大 8 record 読み、検証に通る最初の 1 件を選ぶ）。
  検証は、`verify()`、`envelope.id` と object id の一致、投稿の envelope であること、replica の scope との一致（public: topic が一致・channel なし・public。
  private: channel が一致・topic が購読の topic と一致）。拒否の理由は `PostRejection`。検証の失敗は `Ok(None)`（warn）、I/O の失敗は `Err`。
- 行は envelope の `to_post_object()` だけから作る。`projection_row_from_header` を `projection_row_from_post(&VerifiedPost, content)` に置き換え、header を docs から
  直接 parse する箇所と `fetch_post_object_for_projection` を削除した。`CanonicalPostHeader` を docs の record から parse する箇所は、app-api の本体から無くなった。
- 反映の入口: `hydrate_object_by_id` を `hydrate_object_in_topic(services, topic_id, replica, object_id, policy)` に置き換えた（private channel の replica id は topic を含まないので、
  購読の topic を渡す）。docs の event は `state` と `envelope` のどちらの key でも個別反映を試す（`envelope` の event は、行が無いときだけ）。
  全件走査は、同じ prefix の読み出しに含まれる envelope の record から行を作り、検証に通らない object を飛ばして続ける。
- 取り下げ: 対象の envelope を上限つきで複数 record から探す。取り下げ済みの行は、docs の `state` から作り直さず、反映済みの行を伏せる（行が無ければ、投稿の反映の時点で伏せた行ができる）。
- 通知: `notification_candidate_from_object_event` は `VerifiedPost` から作る。`state` と `envelope` のどちらの event でも試す（通知の id は envelope id から決まるので重複しない）。
  起点（`snapshot_object_notification_baseline`）に `envelope` の key を含めた。
- repost の snapshot と bookmark の添付は、検証済みの projection の行から作る（docs の読み直しを削除）。返信先の preview は `load_verified_post`（`LocalOnly`）で反映する。
  `attachment_views_for_projection_row` の旧い行向けの fallback（inventory の V-2）を削除した。
- 自分の投稿（`ingest_event`）も同じ検証（署名、書き込む replica と topic / channel の整合）を通す。docs は読まない。docs へ書く header の値は変わらない。
- docs-sync: `post_replica_kind`（replica id の組み立ての逆。区切りを含む channel id / epoch id は受け付けない）と、key を 1 つ指定して返す record 数に上限を置く
  `DocsSync::query_replica_exact_bounded` を追加した。iroh の実装は query の `limit` を使う。`ReloadableDocsSync` は転送を宣言した。
- store: `VERIFIED_OBJECT_PROJECTION_VERSION`（3）。migration `20260921000000_drop_unverified_object_projections` が `projection_version < 3` の投稿の行を消す。
  正しい投稿は、手元の docs から反映し直される（bookmark・通知・reaction の表は触らない）。

## 既存 test の期待値の変更

- `hint_rehydration.rs` の 2 件（`topic_doc_events_do_not_rehydrate_whole_replica`・`topic_object_hints_do_not_rehydrate_whole_replica`）: 個別反映が key 指定で読む key が
  `objects/<id>/state` から `objects/<id>/envelope` に変わった。「全件走査しない」の assert は変えていない。
- `crates/store/src/tests/migrations*.rs`: migration の世代数（24 → 25）。

## inventory の逆引き（修正後）

- `projection_row_from_post` の caller: `hydration_support.rs`（全件走査・1 件の反映）、`timeline_view_support.rs`（返信先の preview）、`timeline_subscription_support.rs`（`ingest_event`）、
  test の `verified_projection_row`。引数が `VerifiedPost` なので、未検証の値からは行を作れない。
- `put_object_projection(s)` の caller: 上の caller と、反映済みの行を書き戻す 2 か所（`recover_missing_bodies` の本文、`hydrate_post_withdrawal_from_record` の伏せ直し）。
- `CanonicalPostHeader` を docs の record から parse する箇所: app-api の本体に無し。残るのは `DesktopRuntime::has_topic_timeline_doc_index_entry`（test と harness 用。sink なし）。
- 列挙方法: `rg "projection_row_from_post|CanonicalPostHeader|put_object_projections?\(" crates --glob '!**/tests/**'`。未分類 0。

## 検証（ローカル、Windows）

- `cargo xtask rust-test`: nextest 1,133 件成功・6 件 skip、doc test 成功。
- `cargo test -p kukuri-app-api --lib hydration_integrity`: 20 件成功（修正前の再現 8 件 + 契約 12 件）。`cargo test -p kukuri-store --lib`: 116 件成功（migration の round trip と schema の golden を含む）。
  `cargo test -p kukuri-docs-sync --lib -- replicas exact_bounded`: 6 件成功。
- `cargo fmt --check`、`cargo clippy -p kukuri-app-api -p kukuri-docs-sync -p kukuri-store -p kukuri-desktop-runtime --all-targets -- -D warnings`、`cargo xtask oversized-files`: 成功。
- `cargo xtask e2e-smoke`（`desktop_smoke_post_persist`。再起動をまたぐ投稿の保持）: 成功。
- `cargo xtask app-api-slow-test`（実際の iroh-docs の同期を通す統合 test）: 282 件成功、4 件失敗。失敗は添付の blob の状態（`Missing` != `Available`）の assert で、
  `media::iroh_transport_syncs_image_post_between_apps`・`media::remote_video_manifest_payload_available_after_sync`・`media::late_joiner_backfills_video_media_payload`・
  `sync::transport_replication::seeded_dht_backfills_docs_and_blobs_with_id_only_seed`。同じ 4 件が、本 Issue の差分より前の `main` の nightly（2026-09-18 `d372c91b`、2026-09-19 `df2fd79e`、
  run 35466114162）でも同じ行で失敗している。差分外の既知の失敗として扱い、別に調査する。private channel と replication の統合 test は成功した。

## 残る事項（本 Issue の対象外）

- 取り下げの record 自体の読み出し（`withdrawals/<id>/state`）は先頭の 1 件だけを見る。同じ key の先頭に不正な record を置かれると、正しい取り下げが反映されない。
- reaction・live session・game room の state doc も、署名の無い doc をそのまま反映する同じ形をしている（未再現）。
- 修正前に保存された bookmark の行と通知の行、成人向けの添付の hash の集合（insert-only）は、再検証していない。
