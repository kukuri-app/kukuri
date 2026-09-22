# 時間bucket基盤の inventory（#1243、T3の前半）

これはID、ローカル読み、同期の停止/再開の差分だけのinventoryである。clientのwriter切替、CN、回収、
全callerのbucket対応を完了したという意味ではない。親のinventoryと未完了範囲は
[作業記録](../progress/2026-09-22-1243-replica-time-buckets.md)を参照する。

## 入口からsinkと逆引き

| ID | 入口・trigger / member | helper | sink | guard / invariant | transition / test |
| --- | --- | --- | --- | --- | --- |
| BF-1 | BucketReplica::new/parse、public_replica_secret、post_replica_kind | TimeBucket、BucketScope検証 | namespace導出、scope判別 | canonical版/ID/時刻範囲、private/未知形式の公開導出禁止 | bucket_ids_roundtrip_without_scope_delimiter_collisions、bucket_parser_rejects_aliases_unknown_versions_and_unbounded_input、private_or_unknown_bucket_cannot_derive_a_public_secret |
| BF-2 | query_replica_with_policy、query_replica_exact_bounded、query_replica_by_author、query_replica_keys/by_author | collect_records、key_page、ensure_replica_for_read/with_sync | import_namespace、docs query、blob fetch | LocalOnly/key列挙/limit0は同期を新規開始しない。private capabilityが無ければ読む前に失敗 | local_only_bucket_lookup_does_not_start_replica_sync、local_bucket_reads_stay_idle_after_seed_reapply_and_close_preserves_data |
| BF-3 | open_replica、apply_doc_op、subscribe_replica/notices | ensure_replica(true)、sender | doc_start_sync、docs write、event task | 明示的な同期要求のみ昇格。重複openでtaskを増やさない | docs_topic_index_roundtrip、relay.rsの実replication tests |
| BF-4 | learn_peer、import_peer_ticket、set_seed_peers、restore_peer_state | reapply_sync_peers | doc_start_sync | sync_requestedの対象だけ。ローカルのみ/closingを起こさない | local_bucket_reads_stay_idle_after_seed_reapply_and_close_preserves_data |
| BF-5 | close_replica、remove_private_replica_secret、shutdown | close_replica_owned、close_tasks | secret削除、leave、close、task停止 | revokeと停止の所有を分離しない。entryは削除しない。caller cancel後もownerが完了する | bucket_close_and_revoke_finish_after_the_caller_is_cancelled、bucket_close_lost_ack_does_not_leave_a_closed_handle_in_the_cache、bucket_close_retries_leave_failure_without_repolling_the_event_task |
| BF-6 | restart_replica_sync、背景のmaybe_restart_replica_sync | registry lock、sync_requested/closing | doc_start_sync | closeと直列化。停止済み/ローカルのみ/不在を再openしない | bucket_close_and_revoke_finish_after_the_caller_is_cancelled、local_bucket_reads_stay_idle_after_seed_reapply_and_close_preserves_data |
| BF-7 | local ingest、doc event/hint/window/pageの投稿検証 | ReplicaPostScope、VerifiedPost::verify/verify_local | projection、通知、snapshotの前段 | bucketと署名済みcreated_at/topic/channelを照合。旧形式の契約は維持 | bucket_post_must_match_its_signed_creation_time_and_scope、hydration_integrity_contract |

## caller group

BF-1のID helperの全production callerは親記録の検索式で列挙する。この段階では既存helperの生成する旧IDを変えない。
BF-2のkey一覧の直接callerは次のgroup。これらは値の取得policyを別に持ち、key一覧だけではremoteを要求しない。

- `docs-sync/src/time_index.rs`: 時系列索引の窓/遡り。
- `app-api/src/service/subscription_catch_up.rs`: 投稿/取り下げ/sessionの窓。
- `app-api/src/service/reaction_hydration.rs`: 対象reactionの有限取得。
- `app-api/src/service/profile_timeline_support.rs`: profileの索引/旧形式の窓。
- `app-api/src/service/profile_docs_support.rs`: custom reaction asset、profile/graphのkey一覧。
- `app-api/src/service/author_state_support.rs`: profile/latest、自分を指すedge、follow/block各512件。
- `desktop-runtime/src/stack.rs`: ReloadableDocsSyncの転送。新closeも内側へ転送する。

BF-2のpolicy付きqueryは同じtraitの全callerに適用する。queryから同期へ到達する逆方向は
`doc_start_sync <- ensure_replica_with_sync / reapply_sync_peers / restart_replica_sync`。
namespace importは `ensure_replica_with_sync` に集約し、ticket importによる暗黙syncを除く。
閉じるsinkは `close_replica_owned -> close_replica_under_guard -> Doc::leave / Doc::close`。
秘密の削除も同じ所有task内にある。task台帳・registry guard・結果通知の所有と、停止操作の実体を分ける。
この分離の範囲と検証は[先行整理の記録](../progress/2026-09-22-1243-lifecycle-preparation.md)を参照する。

再列挙:

```powershell
rg -n 'query_replica|open_replica|subscribe_replica|apply_doc_op|restart_replica_sync|close_replica|remove_private_replica_secret' crates --glob '*.rs'
rg -n 'doc_start_sync|import_namespace|\.leave\(|\.close\(' crates/docs-sync/src --glob '*.rs'
rg -n 'ReplicaPostScope|VerifiedPost::verify|select_verified_post' crates/app-api/src --glob '*.rs'
```

## 遷移と禁止副作用

| ID | 事前状態 / event | 期待状態 | 許可I/O | 禁止副作用 |
| --- | --- | --- | --- | --- |
| BT-1 | fresh → LocalOnly/key一覧 → peer再適用 | ローカルだけで開く | ローカルdocs I/O | sync開始、remote blob取得 |
| BT-2 | ローカルのみ → 明示open → close → LocalOnly | 同期を開始/停止し保存値を再表示 | 明示open時のsync、ローカルread | entry削除、勝手な再sync |
| BT-3 | private未登録/失効 → read/open | 拒否 | なし | namespaceの公開導出、同期、読取り |
| BT-4 | close登録済み → caller cancel → 背景restart | ownerが停止を完了 | leave/close | stale restartによる再open、新handleの削除 |
| BT-5 | leave失敗 → read/restart → close再試行 | 読みを拒否し再syncせず、closeを再試行 | 明示close再試行 | 消費済みJoinHandle再poll、無限再試行 |
| BT-6 | close RPC応答喪失 → LocalOnly再open | 閉じたDocを使わず保存値を読める | ローカルimport/read | 閉じたhandleの再利用 |
| BT-7 | 署名の正しい投稿を別時刻bucketへ配置 | 反映しない | 署名検証・ローカルread | projection/通知/snapshotへの反映 |

分類結果と実行証跡はPR headの独立監査時に確定する。ここにtest名があることだけでPASSとはしない。
