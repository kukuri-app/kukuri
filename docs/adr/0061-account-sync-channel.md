# ADR 0061: 本人の端末間の account 同期の専用チャンネル

## Status

Accepted（Issue #1218 W5 AC-1。分類の接続・merge・鍵の保持・差分の取得・統合は同 Issue の AC-2〜6、鍵更新の担当は #1219 W6）

## Context

同じアカウントを複数の端末（native・Web）で使うとき、profile・共有する設定・private channel の鍵を、再度の QR なしで反映したい（#1213 D-9〜D-12）。
ユーザーの決定は、アカウントの秘密鍵から導出する専用の非公開チャンネル `kukuri:account:${hash}` を設け、起動・切替・復帰のときに既定で購読すること。`kukuri:topic` としては扱わない。

基準（統合 branch `78d623732`）の事実:

- docs の公開の replica は、replica id から namespace の秘密を誰でも導出できる（`crates/docs-sync/src/replicas.rs` の `public_replica_secret`）。除外は `channel::` と private の bucket だけで、未分類の id は公開の導出へ進む。
- private の replica は、`DocsSync::register_private_replica_secret` で登録した秘密だけで開く（`iroh_sync.rs` の `replica_secret`）。
- ADR 0053 は、アカウントの秘密鍵から `blake3::derive_key` で docs author を導出している。
- ADR 0055 の通知・DM・epoch 制御の受信 route（`receive::v1::<hash>`）は公開鍵から導出するもので、本人の端末間の同期とは別の契約である。
- 端末の設定の大半は端末固有である（成人向けの表示は年齢の申告に、Community Node の設定は node ごとの同意と token に結び付く）。

## Decision

### 1. 導出と namespace

- アカウントの秘密鍵（32 byte）から、`blake3::derive_key` の context を用途ごとに分けて 4 つの値を導出する（`crates/core/src/account_sync.rs`）。context は変えない（変えると全アカウントの同期先が変わる）。

| 用途 | context | 使い方 |
| --- | --- | --- |
| 識別子 | `kukuri.app 2026-10-01 account sync id v1` | docs の replica `account::v1::<hex>` |
| namespace の秘密 | `kukuri.app 2026-10-01 account sync namespace v1` | `register_private_replica_secret` で登録する |
| payload の鍵 | `kukuri.app 2026-10-01 account sync payload v1` | item の暗号化・認証（§3） |
| rendezvous | `kukuri.app 2026-10-01 account sync rendezvous v1` | gossip の hint の topic `kukuri:account:<hex>` |

- 導出は一方向で、公開鍵だけからは計算できない。同じアカウント鍵を持つ端末は同じ値になり、初回の QR で組んだ 2 端末以外も同じ同期先へ届く。識別子・topic・namespace・payload の鍵は互いに別の値で、docs author（ADR 0053）とも別である。
- `account::v1::` の replica は、replica id から namespace を導出しない（`public_replica_secret` で除外する）。namespace の秘密を登録していなければ開けず、公開へ fallback しない。
- 同期のチャンネルは、通常の private channel の epoch から独立している。アカウント鍵だけで導出でき、epoch の更新（W6）に巻き込まない。
- 値の prefix（`kukuri:account:`・`account::v1::`）は wire の定数として凍結する（`crates/core/src/wire.rs`）。

### 2. 同期する item（allowlist）

| item | docs の key | 値 |
| --- | --- | --- |
| 公開 profile | `profile` | 既存の署名済みの profile の envelope（公開の正本と同じ ID。別の正本を作らない） |
| 著者を常に表示する指定 | `trust/always-visible/<著者の公開鍵>` | 著者ごと。解除は tombstone |
| private channel の受領済みの世代の鍵 | `channel/<channel id の hex>/epoch/<epoch id の hex>` | (channel, epoch) ごと。追加だけ |
| private channel の明示の退会・取消 | `channel/<channel id の hex>/leave` | tombstone として保持する |
| private channel の鍵更新の担当 | `channel/<channel id の hex>/controller` | 意味は W6（#1219）が所有する |

同期しないもの: アカウントの root の秘密鍵（初回の移行と既存の backup で扱う）、iroh の endpoint 秘密鍵・端末 ID、Community Node の token・設定・同意、アプリの同意・年齢の申告・成人向けの表示、OS の permission、window・通知・開発者の設定、discovery の seed、SDP・ICE・WebRTC の session（ADR 0057）。
allowlist の外の種類は封を開けても受け付けない（`AccountSyncItemKey` の `kind` の照合）。開いた item の key と docs の key の一致も確かめる。`value` の中身の検査は、書き手と読み手を実装する AC-3・AC-4 が種類ごとに行う。

### 3. payload

- 1 item を XChaCha20-Poly1305 で暗号化・認証する。平文は `key`・`op_id`（更新ごとの 32 桁の hex。再送しても変えない）・`updated_at`（編集した端末での編集時刻、ミリ秒）・`value`（無ければ tombstone）。
- AAD は `kukuri account sync item v1\0`・アカウントの公開鍵・`\0`・docs の key。別のアカウント・別の key に置き換えた封は開けない。開いた item の key と docs の key の一致も確かめる。
- 平文は 16 KiB まで、封は 32 KiB＋256 byte まで。超えるものは書かず、読まない。

### 4. 競合の規則（AC-3・AC-4 で実装）

- profile: 署名済みの envelope の `created_at` が新しいものを採る。同じなら envelope の ID の辞書順で大きいものを採る。
- 著者を常に表示する指定などの設定の item: `updated_at` が新しいものを採る。同じなら `op_id` の辞書順で大きいものを採る。再受信・再起動・restore の時刻を `updated_at` にしない。
- private channel の鍵: (channel, epoch) ごとに追加し、相手に項目が無いことを削除と解釈しない。現在の世代への切替は、検証済みの遷移（W6）に従う。
- 退会・取消の tombstone は、それより古い `updated_at` の鍵の item では参加を戻さない。明示の再参加は、新しい `updated_at` の別の認証済みの更新として扱う。
- 採用した状態は item ごとに 1 行で持つ（操作の log を持たない）。同じ `op_id` と `updated_at` の再受信は何もしない（重複排除の台帳を別に持たない）。

### 5. 上限

| 資源 | 上限 | 超えたとき |
| --- | --- | --- |
| 1 item | 平文 16 KiB | 書かない |
| 1 回の差分の取得 | 64 item（key の一覧の 1 page） | 次の page は cursor から取る |
| 送信待ちの item（outbox） | account ごとに 256 | 新しい編集を保留として示し、古い送信待ちを消さない |
| 送信の試行 | 既存の需要の owner の期限と回数（ADR 0055） | 保留に戻す |
| 保存 | item ごとに現在の状態の 1 行 | 操作の log・重複排除の台帳を持たない |

鍵・退会・採用済みの version の行は、容量の回収で消さない。

### 6. 所有する AC

| 内容 | 所有 |
| --- | --- |
| 導出、payload の封、allowlist、wire の定数、公開の導出からの除外（本 ADR） | W5 AC-1 |
| 起動・import・切替・復帰、hint・rendezvous・CN の索引・検索・推薦・診断での新しい種別の分類（page_read の private の判定を含む） | W5 AC-2 |
| profile・設定の merge | W5 AC-3 |
| (channel, epoch) の鍵の保持と退会（native と Web の行ごとの保存。ADR 0059 §3） | W5 AC-4 |
| 起動・復帰・通知の欠落の差分の有限 page と durable な cursor | W5 AC-5 |
| Web と native の 2 端末の統合 | W5 AC-6 |

### 7. 新しい種別の分類（W5 AC-2 の実装）

公開ではない topic の判定は `kukuri_core::wire::is_non_public_topic`（private channel・DM・account 同期の hint。`hint/` の有無によらない）の 1 つで行う。入口ごとの扱い:

| 入口 | 扱い |
| --- | --- |
| 起動・import・切替・復帰 | runtime の起動で `AppService::start_account_sync` が scope の lease（`ScopeKey::AccountSync`）を取る。lease の task が replica の namespace の秘密を登録し、hint を購読する。private channel の復元より前に取る（scope の上限 64 の 1 つ）。import は account の追加・切替と同じ runtime の起動を通る。停止・切替は runtime の停止で lease ごと外れ、新しい runtime は新しい account の値だけを持つ。endpoint の作り直し（docs も新しくなる）は、lease の task の作り直しで秘密の登録と購読へ戻る |
| hint | `ScopeKey::AccountSync` は公開の topic の lease（`leased_topics`）に入らない。gossip は rendezvous が返した本人の端末とだけ合流し、bootstrap の peer（他人の端末・node）へ topic の join を送らない（送ると、合流できないまま warmup の接続を繰り返して他の通信を乱し、他人に topic を知らせる）。hint を受けた差分の取得は AC-5 |
| rendezvous | 購読中の account の hint は、秘密から導出した topic の rendezvous の鍵（`public_topic_rendezvous_key(hint/kukuri:account:<hex>)`）で Community Node へ送り、本人の端末どうしを Relay Supported P2P で会わせる。鍵はアカウント鍵を持つ端末だけが計算でき、node が受け取るのは不透明な鍵だけ。node が同じ account の端末を結び付けられることは、既存の受信 route の rendezvous（公開鍵から導出）と同じで、新しい情報を加えない |
| 診断 | sync status の topic の一覧と topic の診断から外す（`normalize_topic_name`） |
| 有界な読み出し（page_read） | `account::v1::` は private の replica として、登録した capability の証明がある要求にだけ応える |
| Community Node の索引・対応 topic | client の索引の依頼・`cn-user-api` の索引の依頼の受付・運用の対応 topic の追加と削除で、公開 topic として拒否する。account の replica は公開の導出で開けないので、indexer は読めない（§1） |
| 検索・発見・推薦 | 索引した投稿だけを返す。上の入口で索引に入らないので出ない |

## 採らない方式

- 公開鍵から同期先を導出する: 公開鍵を知る誰もが同期先を知れる（ADR 0055 の受信 route と同じになる）。
- 1 つの鍵を識別子・namespace・暗号化に使い回す: 1 つの値の漏れがすべてに及ぶ。
- 操作の log を同期する: 件数に比例して増える。item ごとの現在の状態だけで収束する。
- account の同期を通常の private channel として作る: epoch の更新と担当（W6）に巻き込まれ、bootstrap にならない。

## Consequences

- アカウント鍵を持つ端末は、公開鍵と別の同期先を持つ。同じアカウント鍵を持つ端末は区別されない（端末の強制失効はしない。#1213 の Non-goals）。
- `account::v1::` と `kukuri:account:` は予約された prefix になる。

## Data classification

ADR 0002 の template に従う。

- Feature 名: 本人の端末間の account 同期
- Durable / Transient: 採用した item の状態は Durable（item ごとに 1 行）。送信待ちは Durable。hint は Transient。
- Canonical Source: profile は既存の署名済みの envelope。その他は、検証済みの item と各端末の保存状態。
- Replicated?: 同じアカウント鍵を持つ本人の端末の間だけ（Private Replica）。
- Rebuildable From: 同期の copy を持つ本人の端末から再取得できる。すべて offline なら待つ。
- Public Replica / Private Replica / Local Only: Private Replica（`account::v1::`）。公開の索引・検索・推薦の対象外。
- Gossip Hint 必要有無: あり（`kukuri:account:<hex>`。内容を含まない変更の手掛かり）。
- Blob 必要有無: なし。
- SQLite projection 必要有無: 採用した状態の行（native は SQLite、Web は IndexedDB。AC-3〜5）。
- 必須 contract: 導出の golden、同じ鍵での一致・別の鍵での不一致、用途の分離、封の検査、上限、公開の導出からの除外、受信 route との分離（本 PR の試験）。
- 必須 scenario: W8 の 2 端末の同期（W5 AC-6）。
- 新しい外部送信: なし（既存の gossip・docs の経路で、暗号化した item を本人の端末へ送る）。

## References

- Issue #1213（D-9〜D-12・D-14）、#1218（本 ADR）、#1219、#1211
- ADR 0053・0055・0056・0059
- RFC 5869（用途の分離の考え方）
