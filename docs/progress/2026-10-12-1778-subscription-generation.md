# #1778: 公開購読の世代履歴の縮約

## 対象と基準

- 条件の正本: [#1778](https://github.com/kukuri-app/kukuri/issues/1778)、親 [#1777](https://github.com/kukuri-app/kukuri/issues/1777)。`AC-GEN / PR-GEN`、Scope revision `2026-10-12-refactor-r1`、区分C。
- 比較起点: `1f77dd4e3252b37fc944446544b901435c04063f`。
- 採番の保持と旧taskの活動拒否だけを変更する。診断値を保持する `public_topic_delivery`、認証、同意、private audience、wire、保存形式は対象外。

## 構造上の成果

topic名ごとの `subscription_generations` と `next_subscription_generation` のlookup/insertを撤去した。`SubscriptionRegistry` が単一の `Arc<AtomicU64>` を所有し、public購読開始時に採番する。`account_handle` は同じregistryをcloneするのでcounterも共有する。世代値は内部の一致判定だけに使い、公開・永続化しない。

採番の経路は、`ScopeLeases::set_holder → start_scope_task → spawn_subscription_task → AtomicU64::fetch_add` の1経路である。過去のtopic名をcounterへ渡さず、counter本体は8byteで、訪問履歴を保持するcollectionも走査もない。`Relaxed` は採番だけに使い、診断値の更新は従来のdelivery mutexと世代一致判定を維持する。

`record_public_topic_docs_activity_if_current` のcallerは受信task内の1箇所。CodeGraphの一般名/caller解決では当該callerが返らなかったため、固有symbolの参照確認で補った。更新先は従来のtopic診断と活動時刻であり、取得・保存・送信先のguardは変更しない。

## 変更前後の確認

既存のendpoint再構築testへ、account handleを使う1列の10,000種類のtopicへの順次切替を加えた。全prefix（100・1,000・10,000を含む）でaccount共通の採番が進むことを確認する。列を閉じて稼働3scopeだけを再構築した後、旧世代の遅延活動を注入しても現在の診断時刻が更新されないこと、再購読は既存の2hint topicだけであることを確認する。

- 元のscope lease 10test、診断10test、wire snapshot 13testは比較起点でPASS。
- 最終形と同じ拡張testを元の3製品fileへ適用すると、`generation > index` が失敗した（exit 1）。製品fileを戻して同じtestを実行し、変更後にPASSを確認した。
- 購読/holder上限・参加分離・shutdown、診断/差分、wireの既存assertionを維持した。
- 全プロセスのRSSやdelivery台帳全体が固定量になったとは主張しない。固定量の対象は採番状態だけである。

## 局所検証

Windows、Rust 1.98.1で次を実行した。`CARGO_TARGET_DIR=C:\Users\kgm11\kukuri\target` は既存のbuild cacheを再利用する指定で、検証するsourceは本worktreeのもの。

| command | 結果 |
| --- | --- |
| `cargo test --locked -p kukuri-app-api --lib tests::sync::scope_leases` | 10 PASS。拡張した履歴/旧世代条件を含む |
| 同 `tests::sync::diagnostics` | 10 PASS |
| 同 `tests::views_wire_snapshot` | 13 PASS |
| `cargo check --locked -p kukuri-app-api` | PASS |
| `git diff --check`、対象fileのrustfmt | PASS |

test linkerの既存 `linker_messages` warningは変更前後とも1件。新しいwarningは観測していない。全suiteとwasm32 clippyは既存PR CIで確認する。固定head独立監査・実必須CI・merge一致の最終結果はIssue/PRへ集約する。

## 同一対象のコード量

UTF-8の物理行（空行を含む、末尾newlineの空要素を除く）を、基準commitと完成sourceで同じ4pathについて数えた。新しい実装/test/補助fileはない。

| path | before | after |
| --- | ---: | ---: |
| `crates/app-api/src/service/mod.rs` | 932 | 917 |
| `crates/app-api/src/service/scope_receive.rs` | 713 | 715 |
| `crates/app-api/src/service/subscription_registry.rs` | 81 | 73 |
| `crates/app-api/src/tests/sync/scope_leases.rs` | 632 | 652 |
| 合計 | 2,358 | 2,357 |

実装は21行減、既存testの拡張は20行増、合計1行減。旧「8field/復旧deadline/Mutex map」の現状と合わないcommentも現行ownerの説明へ置換した。成果は行数だけでなく、採番の正本が履歴mapから固定量counterへ変わり、keyとlookup/insert経路が消えたことで判定する。本証跡文書の追加はコード量と別計測する。

差し戻しは本PR単独revertで可能（永続移行なし）。判断待ち候補のdelivery回収・安全cache等は追加していない。
