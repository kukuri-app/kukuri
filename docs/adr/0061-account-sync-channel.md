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
| private channel の参加・明示の退会・取消 | `channel/<channel id の hex>/membership` | 参加の端末に依らない欄（§9）。退会・取消は tombstone として保持する |
| private channel の鍵更新の担当 | `channel/<channel id の hex>/controller` | 意味・値・採否は W6（#1219）が所有する（ADR 0018 §8） |

同期しないもの: アカウントの root の秘密鍵（初回の移行と既存の backup で扱う）、iroh の endpoint 秘密鍵・端末 ID、Community Node の token・設定・同意、アプリの同意・年齢の申告・成人向けの表示、OS の permission、window・通知・開発者の設定、discovery の seed、SDP・ICE・WebRTC の session（ADR 0057）。
allowlist の外の種類は封を開けても受け付けない（`AccountSyncItemKey` の `kind` の照合）。開いた item の key と docs の key の一致も確かめる。`value` の中身の検査は、書き手と読み手を実装する AC-3・AC-4 が種類ごとに行う。

### 3. payload

- 1 item を XChaCha20-Poly1305 で暗号化・認証する。平文は `key`・`op_id`（更新ごとの 32 桁の hex。再送しても変えない）・`updated_at`（編集した端末での編集時刻、ミリ秒）・`value`（無ければ tombstone）。
- AAD は `kukuri account sync item v1\0`・アカウントの公開鍵・`\0`・docs の key。別のアカウント・別の key に置き換えた封は開けない。開いた item の key と docs の key の一致も確かめる。
- 平文は 16 KiB まで、封は 32 KiB＋256 byte まで。超えるものは書かず、読まない。

### 4. 競合の規則（AC-3・AC-4 で実装）

- profile: 署名済みの envelope の `created_at` が新しいものを採る。同じなら envelope の ID の辞書順で大きいものを採る。
- 著者を常に表示する指定などの設定の item: `updated_at` が新しいものを採る。同じなら `op_id` の辞書順で大きいものを採る。再受信・再起動・restore の時刻を `updated_at` にしない。
- private channel の鍵: (channel, epoch) ごとに追加し、相手に項目が無いことを削除と解釈しない。値の形と現在の世代への切替は、検証済みの遷移（W6、ADR 0018 §8）に従う。
- 鍵更新の担当: 担当の世代（`generation`）で採り、`updated_at` では決めない（ADR 0018 §8）。
- 参加・退会・取消（`membership`）は `updated_at` が新しいものを採る。同じなら `op_id` の辞書順で大きいものを採る。退会・取消の tombstone は、それより古い `updated_at` の鍵の item では参加を戻さない。明示の再参加は、新しい `updated_at` の値のある版として扱う（§9）。
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
| profile・設定の merge（§8） | W5 AC-3 |
| (channel, epoch) の鍵の保存と channel の item の merge の設計（§9） | W5 AC-4a |
| native の行ごとの保存と旧 registry・旧 backup の移行（§9。Web の IndexedDB の実装は W4 AC-2） | W5 AC-4b |
| channel の item の書き込みと merge（§9） | W5 AC-4c |
| 参加中の channel の一覧の続きの表示（画面・CLI） | W5 AC-4d |
| 起動・復帰・通知の欠落の差分の有限 page と durable な cursor、DB を失ったときの作り直し（§9） | W5 AC-5 |
| Web と native の 2 端末の統合 | W5 AC-6 |

### 7. 新しい種別の分類（W5 AC-2 の実装）

公開ではない topic の判定は `kukuri_core::wire::is_non_public_topic`（private channel・DM・account 同期の hint。`hint/` の有無によらない）の 1 つで行う。入口ごとの扱い:

| 入口 | 扱い |
| --- | --- |
| 起動・import・切替・復帰 | runtime の起動で `AppService::start_account_sync` が scope の lease（`ScopeKey::AccountSync`）を取る。lease の task が replica の namespace の秘密を登録し、hint を購読する。private channel の復元より前に取る（scope の上限 64 の 1 つ）。import は account の追加・切替と同じ runtime の起動を通る。停止・切替は runtime の停止で lease ごと外れ、新しい runtime は新しい account の値だけを持つ。endpoint の作り直し（docs も新しくなる）は、lease の task の作り直しで秘密の登録と購読へ戻る |
| hint | `ScopeKey::AccountSync` は公開の topic の lease（`leased_topics`）に入らない。gossip は rendezvous が返した本人の端末とだけ合流し、bootstrap・ticket の peer（他人の端末・node）へ topic の join を送らない（送ると、合流できないまま warmup の接続を繰り返して他の通信を乱し、他人に topic を知らせる）。hint を受けた差分の取得は AC-5 |
| rendezvous | 購読中の account の hint は、秘密から導出した topic の rendezvous の鍵（`public_topic_rendezvous_key(hint/kukuri:account:<hex>)`）で Community Node へ送り、本人の端末どうしを Relay Supported P2P で会わせる。鍵はアカウント鍵を持つ端末だけが計算でき、node が受け取るのは不透明な鍵だけ。node が同じ account の端末を結び付けられることは、既存の受信 route の rendezvous（公開鍵から導出）と同じで、新しい情報を加えない |
| 診断 | sync status の topic の一覧と topic の診断から外す（`normalize_topic_name`） |
| 有界な読み出し（page_read） | `account::v1::` は private の replica として、登録した capability の証明がある要求にだけ応える |
| Community Node の索引・対応 topic | client の索引の依頼・`cn-user-api` の索引の依頼の受付・運用の対応 topic の追加と削除で、公開 topic として拒否する。account の replica は公開の導出で開けないので、indexer は読めない（§1） |
| 検索・発見・推薦 | 索引した投稿だけを返す。上の入口で索引に入らないので出ない |

### 8. profile・設定の merge（W5 AC-3 の実装）

- 採用した状態は account の store の `AccountSyncStore`（native は SQLite の `account_sync_items`、item ごとに 1 行）に置く。
  - 行は `(updated_at, op_id)` が今の行より大きいときだけ置き換える。同じ操作の再受信・古い版（restore した古い端末の版を含む）は何もしない。
  - Web の実装は W4 AC-2 が IndexedDB で作る。
- profile の item の op_id は envelope の ID の先頭 32 桁、`updated_at` は envelope の `created_at`。§4 の profile の規則（`created_at`、同じなら ID）を、他の item と同じ `(updated_at, op_id)` の比較で表す。
  - merge は envelope の署名・本人であること・item との一致を確かめてから、既存の profile の確定（`commit_my_profile`）で反映する。
  - この版より前の profile（行の無いもの）は、手元の profile の時刻と比べる。
- 書き込み: profile は確定のたび、表示例外は設定・解除のたびに、この端末の採用と同時に封をして account の replica へ書く。
  - replica へ書けなくても、この端末の採用は戻さない（送り直しは AC-5）。
  - replica は本人の全端末が同じ docs author（ADR 0053）で書くので、key ごとに書いた時刻の新しい 1 件だけが残る。`(updated_at, op_id)` の古い版を後から書くと、新しい版を隠す。
    そのため、どの編集よりも古い版（旧版の取り込み）は replica へ書かない。
  - 時計のずれで同じことが起きた場合に備え、読んだ版が手元の行より古い端末は、手元の版を書き直す。これは AC-5 の差分の取得の規則とする。
  - AC-3 より前に確定した profile は、次の編集で書く。
- 点読（`read_account_sync_item`）: account の docs author の組の 1 件を読み、無い・開けないときだけ、key ごとの上限 8 件の旧候補から最大の版を採る。replica の全件・全 snapshot を比べない。
  - 本人の別の端末から読む差分の取得と、点読・merge の呼び出しは AC-5。
- 作者ごとの表示例外（`trust/always-visible/<pk>`）は、本人の端末で共有する。
  - 旧版の端末内の file（`<db>.trust-display.json`）は、起動時に item（編集時刻 0。どの編集よりも古い）として、この端末だけで採用して消す（replica へは書かない。他の端末へは次の編集で届く）。
    取り込めない file（壊れているなど）は、表示設定にすぎないので起動を止めず、次の起動で取り込み直す。
  - 画面の「この端末だけの設定」の説明は、同じアカウントの端末で共有する旨に改めた（2026-10-02 ユーザー判断）。
  - 表示の判断は対象の作者ごとに 1 行を読む（全件を読まない）。

### 9. private channel の鍵の保存と channel の item（W5 AC-4。AC-4a で固定し、AC-4b・AC-4c で実装）

基準（統合 branch `74cc6a352`）の事実:

- 参加中の全 channel の capability（過去の全 epoch の秘密を含む）を 1 つの JSON にして、optional secret（purpose `private-channel-capabilities`、key `registry`。keyring、使えなければ file）に置いている。
  参加・世代の追加・退会のたびに全件を書き直し、起動時に全件を読んで全 epoch の秘密を docs の登録簿へ入れる。
- docs の登録簿（`register_private_replica_secret`）はメモリの表で、手元で replica を開くときと、相手からの private の読取り（`page_read`）への応答の両方がこれを引く。
  登録は、参加の登録（起動時の復元では lease の無い channel も含む）と endpoint の作り直しで、全 channel・全 epoch に対して行う。外すのは退会のときだけ。
- メモリの参加状態（`joined_private_channels`）は全 channel・全 epoch を持つ。通知・epoch 制御の offer の照合、owner の channel の巡回、topic ごとの一覧、rendezvous、Dome の context、endpoint の作り直しが、これを全件走査する。
  live・game の session の一覧・表示（`scope_replicas`）は、channel の全 epoch の replica を列挙する。
- device backup は、account の DB と秘密の bundle（`SecretBundleV1`。`private_channel_capabilities` は registry の文字列そのもの）を持つ。
  restore は bundle の registry を optional secret へ戻し、restore の検証（`validate_persisted_runtime_state`）は registry の秘密の形を確かめる。
- harness は registry を `Vec` で持ち、再起動のときに戻す。
- `JoinedPrivateChannelView.archived_epoch_ids` は、画面では使っていない（mock と story だけ）。CLI の出力の schema（必須の欄）と harness の試験が使う。

#### 保存の形（AC-4b）

正本は account の store の 2 種類の行とする。store の trait は `ProjectionStore` の合成に入れ、native は SQLite、Web は W4 AC-2 の IndexedDB の保護行で実装する。

| 行 | key | 欄 |
| --- | --- | --- |
| 参加 | channel id | topic id、label、作成者・owner・参加の経路の公開鍵、audience、現在の世代の ID、鍵更新の担当の記録（ADR 0018 §8）、参加か退会（tombstone）か、`membership` の item の `updated_at`・`op_id` |
| 世代の鍵 | (channel id, epoch id) | 世代の開始時刻、受信 route の識別子（`receive_epoch_key_id`）、鍵を受け取った時刻（`updated_at`）、封をした秘密 |

- 索引: 参加は (topic id, channel id) と (owner, channel id)。世代の鍵は受信 route の識別子と (channel id, 開始時刻)。
- 秘密の置き場: 世代の秘密は、アカウント鍵から `blake3::derive_key`（context `kukuri.app 2026-10-02 private channel key rows v1`）で導出した鍵で、XChaCha20-Poly1305 の封をして行に置く。
  - AAD は用途の文字列・アカウントの公開鍵・channel id・epoch id。別の行・別の account へ移した封は開けない。
  - 保護の強さはアカウント鍵の置き場（keyring、使えなければ file。Web は vault）と同じで、今の registry と変わらない。DB の file だけを持ち出しても開けない。
  - keyring・vault の entry を世代ごとに作らない。作ると OS の keyring の entry が件数に比例して増え、行の索引と秘密が別の保存先に分かれて、書き込みの途中で食い違う。
  - Web の vault は capability を持たない（ADR 0059 §1 を改める）。Web の行は cache の database の保護行で、strict の transaction で書き、容量の回収で消さない（ADR 0058 §4）。
- 書き込みは対象の行だけにする。
  - 参加: 参加の行と、現在の世代の鍵の行。
  - 世代の追加: 新しい世代の秘密は、行にするまで招待の preview と同じく docs の登録簿へ一時に入れる。検証を通らない世代の鍵の行は作らない。
    - owner の鍵更新: 今と同じ順（担当の判定 → 旧世代の凍結 → 新しい世代の準備 → grant の配布 → 確定）で、鍵の行と参加の行の現在の世代は、最後の確定の段で 1 transaction で書く（redeem と同じく、transaction の中で参加中であることを確かめる）。準備と配布の間は一時の登録で応える。
    - 世代の開始時刻は epoch id の時刻（`legacy` は最小）。新しい世代の epoch id の時刻は、今の時刻と、現在の世代の開始時刻 + 1 ミリ秒の大きい方にする。これで、担当の端末が変わっても時計が戻っても、開始時刻の順が世代の鎖（`previous_epoch_id`）の順と一致する。ただし新しい担当が鎖の先頭の世代を持っていることが前提で、それは担当の移譲の遷移の条件（移譲先が最新の世代を持ってから有効になる。#1219 AC-4）とする。
    - handoff の redeem: 止めるのは、grant の旧世代が手元の現在の世代でないときだけ。新しい世代の鍵の行の有無では止めない。policy の検証を通ったら、鍵の行が無ければ書き、参加の行の現在の世代を進める（1 transaction）。旧世代の照合は、commit する transaction の中で参加の行に対して行い、行が参加中（tombstone でない）で、その現在の世代が grant の旧世代と一致するときだけ書く。検証を通らなければ何も書かず、次の呼び出しでやり直す（今と同じ）。新しい世代の replica への参加の doc は、行の commit が成立した後に書く。
    - 一時の秘密は、検証を通ったときは行の commit の後に外し、redeem の policy の検証を通らなかったときもすぐに外す。owner の鍵更新が配布の途中で失敗したときは、今と同じく巻き戻さず、再起動まで一時の登録に残す（配布の間に redeem を済ませた参加者へ応え続ける）。
    - メモリの参加状態と lease は、行の commit が成立したときだけ更新する（退会と並行した世代の追加で、参加に戻さない）。commit とメモリの更新は、既存の全体の排他（`content_save_access`）の中で行う。退会と tombstone の採用も同じ排他を取る（lease の解除と順序が入れ替わらない）。排他の中で行うのは、参加の行の commit とメモリと lease の更新だけ。redeem の検証の待ち、hint の送信、docs・outbox への記録の書込み、鍵の行の page ごとの削除は排他の外で行う（退会では、記録を書いてから鍵の行を消す）。channel ごとの lock の表は作らない。
  - 退会: 参加の行を tombstone にする（排他の中）。その後、排他の外で、退出の記録を owner へ書き（下の「owner への記録」）、その channel の世代の鍵の行を page で消し、replica を閉じる。鍵の行を消してから replica を閉じる（閉じた後に開き直されても、秘密を引けない）。
  - 全件の書き直しをしない。
- 退会の tombstone と鍵の行は、容量の回収で消さない（§5）。

#### docs の秘密の引き方（AC-4b）

- 参加中の channel の世代の秘密は、docs の登録簿へ入れない。docs が replica の秘密を要するとき（手元で開く、相手からの private の読取りに応える）は、登録簿に無ければ、replica id から (channel id, epoch id) を求め、世代の鍵の行を key で 1 件読む。
  - 引き方は、app-api が docs へ渡す非同期の参照にする（`page_read` の秘密の参照も非同期にする）。鍵の行があれば返す（tombstone の channel でも、鍵の行を消すまでは退出の記録を書くために返す）。
  - lease の無い channel・過去の世代にも、今と同じく応える。起動時・endpoint の作り直しで秘密を登録し直さない。
- 登録簿に残るのは、account 同期の replica と、一時の秘密（参加の前の招待の preview、行にする前の新しい世代）だけ。一時の秘密は、成否によらず外す（成功のときは行の commit の後）。
- endpoint の作り直しで docs が新しくなったら、参照も入れ直す。`MemoryDocsSync` の private の確認も同じ参照を使う。

#### 読む範囲（AC-4b）

- メモリの参加状態は、`Channel` の key に lease のある channel（ADR 0055 の上限 64 の内）の、参加の行と現在の世代の鍵だけを持つ。
  - lease を取るときに行を読み、最後の lease が外れたら捨てる。
  - 過去の世代の鍵はメモリに持たない。
- 起動:
  - 旧 registry を移す（下記）。
  - 参加中の行を key の順に読み、参加の holder の lease を取る。上限に達したら読むのをやめる。残りは今と同じく購読せず、行は残る。
- 各操作は対象の行だけを読む。

| 操作 | 読むもの |
| --- | --- |
| 点の参照（書込み・共有・索引の依頼・Dome の locator・remote read の現在の世代） | メモリ。無ければ参加の行と世代の鍵の行を key で 1 件ずつ |
| 保存の gate（`active_content_scope_generation`） | メモリだけ。lease の無い channel（上限 64 を超えて参加しているもの）の内容は保存しない |
| 通知・epoch 制御の offer の照合 | 受信 route の識別子の索引で 1 件。通知は、その行が参加中の channel の現在の世代のときだけ |
| 過去の bucket の読み出し、Dome の書込みの anchor | (channel id, 開始時刻) の索引で、bucket に掛かる世代を新しい順に 8 件まで |
| 遅れた参加者への grant の直前の世代 | (channel id, 開始時刻) の索引で 1 件 |
| live・game の session の一覧・表示（`scope_replicas`）、thread の窓（`local_page_replicas`） | 全世代を列挙しない。候補の絞り込みは replica id の channel で行い（候補は購読の event と固定の窓で集めた作業集合）、replica を指定した表示の許可は、replica id の channel と、その世代の行の点読で確かめる。手元の docs の catch-up と、replica を指定しない表示の要求は、(channel id, 開始時刻) の索引で新しい順に 8 世代まで。anchor のある thread の窓は、過去の bucket の読み出しと同じく、anchor の時刻に掛かる世代を開始時刻の索引で読む。鍵の更新の後の session の状態の更新は現在の世代の bucket へ書かれるので、外れるのは、新しい 8 世代より古い epoch の replica にあり、その後に更新されていない旧形式の session だけ（取りこぼしを 0 にすることを目標にしない） |
| topic の参加中の一覧 | (topic id, channel id) の索引で 128 件の page（ADR 0055 の参加者の page と同じ）。続きは cursor から読む（2026-10-02 ユーザー判断。画面と CLI の続きの表示は AC-4d）。view の `archived_epoch_ids` は欄を残し、現在の世代より前に始まった世代を新しい順に 8 件まで（本人の別の端末から届いて、まだ現在の世代になっていない新しい世代は含めない） |
| owner の channel の巡回（epoch 制御）、現在の世代の記録の移行（#1221 R5-G） | 索引の cursor から 1 件ずつ |
| rendezvous、Dome の context | メモリ（lease のある channel）だけ。lease の無い channel は含めない |
| 退会 | その channel の世代の鍵の行を page で読みながら、replica の参照を外す |

#### 旧 registry・旧 backup の移行（AC-4b）

- 起動時に旧 registry があれば、1 回だけ行へ移し、成功したら registry を消す。
  - 旧 registry は 1 つの値なので、分けて読めない（一度だけの変換）。
  - 行の書き込みは 1 transaction で、冪等にする。途中で止まっても、次の起動でやり直す。
  - 担当の欄の無い自分の channel の移行（ADR 0018 §8）は、この変換で行う。
  - 移した行の `updated_at` は 0（どの編集よりも古い）。
- 旧 backup: restore は今のまま bundle の registry を optional secret へ戻し、次の起動で上の移行により行になる。
  - 新しい backup は、行を DB に持つ。bundle の `private_channel_capabilities` は `None` になる（形式は変えない）。
- restore の検証は、旧 registry があるときだけ秘密の形を確かめる（今のまま）。行は、store の migration と、読んだときの封の検査で確かめる。
- 旧形式の単一 account からの移行（`accounts.rs`）と account の file の自己完結化は、旧 registry の locator をそのまま扱う。次の起動で行になる。
- 行へ移した DB を、古い版で開くことは扱わない（store の migration は既に前方だけで、古い版は開くのを拒否する）。
- 不要になるものは消す。
  - persist の callback（`set_private_channel_capability_persist`）と、registry の書き出し。
  - 参加中の channel の秘密の docs への登録（`register_private_channel_replica_secrets`）。
  - harness の `Vec`（再起動では同じ store を使う）。
- `PrivateChannelCapability` は、旧 registry の読み込みだけに使う。形の凍結の試験（`capability_registry_snapshot`）は残す。`restore_private_channel_capability` は旧 registry の移行の入口として残し、試験も別の端末の状態をこれで作る。

#### DB を失ったとき（2026-10-02 ユーザー判断）

- 行は account の DB に置く。DB の file を失ったら、手元の account 同期の replica（docs の保存で、DB とは別にある）の channel の item から、AC-5 の差分の取得で作り直す。
  - cursor も DB にあるので、最初の page から読む。page は有限で、途中で止まっても再開できる。参加が戻るのは、背景の取得がその item に届いた後。
  - 作り直せるのは replica へ書いた item だけ。書けなかった行・旧版から移した行は、AC-5 の送り直しで書いた後に作り直せる。
  - 担当の記録は `membership` に含まれない。作り直した行の担当の記録は、W6 が書く `controller` の item から戻る。それまでは担当が不明な channel として、鍵更新を伴う操作を保留する（ADR 0018 §8）。
- 旧 registry の時の「DB を消しても再起動で参加が戻る」性質は、AC-4b から AC-5 までの間は失われる。
  DB を消して再起動する既存の試験（harness の `private_channel_invite_connectivity` の再起動の step、desktop-runtime の `private_channel_invite_restores_after_restart_without_reimport`・`friend_only_channel_restore_keeps_archived_epoch_history`・`friend_plus_channel_restore_accepts_fresh_share_after_restart`）は、AC-4b で DB を消さない再起動に改め、AC-5 で作り直しを確かめる試験に戻す。

#### channel の item の merge（AC-4c）

- `membership` を書くのは、参加（作成・招待による参加。既に参加中の channel への招待・grant の取り込み直しを含む）、明示の退会、明示の再参加のときだけ。世代の追加（鍵更新・handoff の redeem）では書かない。
  - 値は、参加の行のうち端末に依らない欄（topic id、label、作成者・owner・参加の経路の公開鍵、audience）。値が無ければ退会・取消の tombstone。
  - `(updated_at, op_id)` で採る（§4）。明示の再参加は、新しい `updated_at` の値のある版。
- 参加にするのは、値のある `membership` を採ったときだけ。他の端末の鍵が届いても、`membership` が届くまで参加にしない。
- 本人の端末の item は account の payload の鍵で封をされ、本人の端末しか書けない。そのため channel の replica の署名を読み直さずに、参加の行を作る。
- 本人の端末が鍵の item を書くのは、その端末が検証を通してその世代を現在の世代にしたとき（参加、owner の鍵更新の確定、redeem）だけ。旧版から移した行の過去の世代も、その端末がかつて現在の世代にした世代である。そのため、参加中の channel の現在の世代は、本人の端末から届いた鍵を含めて、開始時刻が最も新しい鍵の行の世代とする（ADR 0018 §8 の例外）。
  - 新しい端末・DB を作り直した端末・offline だった端末は、世代を 1 段ずつ辿らずに最新の世代へ移る（追い付く処理が世代の数に比例しない）。
  - 鍵の item を保存する transaction の中で、参加の行が参加中で、その世代の開始時刻が現在の世代より新しいときだけ、現在の世代を進める。
  - tombstone の行に残る世代は使わない（退会で鍵の行を消す）。
- 値のある `membership` を採ったとき、鍵の行が既にあれば、(channel id, 開始時刻) の索引で開始時刻が最も新しい 1 件を現在の世代にする。鍵の行がまだ無ければ、鍵待ちとする。参加の行だけを持ち、lease・購読・書込みをしない。鍵の item を保存して現在の世代が決まったときに lease を取る。
  - 鍵待ちになったら、手元の account の replica から、その channel の鍵の item（`channel/<hex>/epoch/` の prefix）を 64 件の page で読み直して保存する。tombstone を採ったときに消した鍵は、差分の取得の cursor を既に過ぎているため。読む量はその channel の世代の数だけで、他の channel に比例しない。背景で進め、途中で止まっても再開できる。
- 世代の鍵の item:
  - 追加だけ。同じ (channel, epoch) の行があれば何もしない。
  - 値の `epoch_id` と key の一致、秘密の形を確かめる。
  - tombstone の `membership` より古いか同じ `updated_at` の鍵は保存しない。
  - `op_id` は (channel id, epoch id) から決める（再送で変えない）。`updated_at` は、鍵を受け取った端末の時刻。
- tombstone を採ったら、この端末の退会と同じ順で、lease を外し、退出の記録を書き（現在の世代があるときだけ。鍵待ちの端末は書かない）、その channel の世代の鍵の行を消す。
  - 退会と並行して別の端末が世代を追加した場合、tombstone より新しい鍵の行が残ることがある。参加にはならず、次の退会か再参加で扱う。
- owner への記録: owner の参加者の表（ADR 0055）は、(channel, epoch, 公開鍵) ごとに時刻の後勝ちで、退出はそれより前の参加の行をすべて閉じる。本人の端末はどれも同じ公開鍵で書く。
  - 時刻: 参加・退出の記録の時刻（参加の `joined_at`、退出の `left_at`）は、端末の時計ではなく、その時点でこの端末が採っている `membership` の版の `updated_at` とする（参加・再参加の時刻、または退会の時刻。旧版から移した行は 0）。redeem・owner の鍵更新の確定で新しい世代へ書く参加の記録も同じ。
  - 書く時: この端末の参加・明示の退会・明示の再参加・redeem・owner の鍵更新の確定（今と同じ）に加え、`membership` の採用でこの端末の参加・退会が変わったとき（tombstone を採った、値のある版を採って参加に戻った。鍵待ちのときは現在の世代が決まったとき）。
  - これで、owner の表の順序は account の `membership` の版の順序と一致する。古い tombstone を遅れて採った端末の退出は、それより新しい再参加の記録を閉じない。退会の後に、それを知らずに redeem した端末の参加の記録（退会より前の参加の時刻）は、その端末が tombstone を採ったときの退出が閉じる。（再参加の記録が届く前に owner が回転すると、owner は古い世代宛ての参加の記録を捨てる。これは今の owner の受け付け（#1221 R5-H）からの制約で、本 ADR では変えない。）
- 担当の記録は、ADR 0018 §8 の規則（`generation`、同じなら `transfer_to`）で採る。書くのは W6。
- 書き込み:
  - 世代の鍵は追加のとき、`membership` は上の 3 つの操作のときに、行の保存と同じ所で封をして replica へ書く。owner の鍵更新で作った世代も、同じ所を通る。
  - AC-3 と同じく、replica へ書けなくても採用は戻さない。
- AC-5 が持つもの:
  - 移した行（`updated_at` 0）と書けなかった行の送り直し。
  - 本人の別の端末の版の取得と、DB を失ったときの作り直し（上記）。
  - 旧版から移した行は、replica の同じ key に版が無いときだけ書く（新しい退会を古い版で隠さない）。

#### 実装（AC-4b）

- 行: store の `PrivateChannelKeyStore`（SQLite の `private_channels`・`private_channel_epochs`、migration `20261003000000`）。封は core の `PrivateChannelKeyRowSeal`。
- docs の秘密の参照: docs-sync の `PrivateEpochSecrets` を app-api が入れ、相手への応答は iroh-node の非同期の `PrivateSecretLookup` で同じ参照を引く。
- 読む範囲の数: 起動時の復元と topic の一覧（`list_joined_private_channels` の `next_cursor` で続く）と退会の鍵の削除は 128 行の page、過去の世代は新しい順に 8 世代まで、bucket に掛かる世代は 2 件まで。
- 参加の行の版（`updated_at`・`op_id`）は、AC-4c の `membership` の item の版になる。op_id は item と同じ形で作り、旧 registry から移した行の時刻は 0。

#### 一覧の画面と CLI（AC-4d）

- 2026-10-02 ユーザー判断: 画面は「このトピックで参加中」の一覧の card の末尾に「さらに表示」を置く（読み込み中は「読み込み中…」で無効）。続きの読み込みに失敗したら、読み込んだ行とボタンを残し、ボタンの下に 1 行のエラーを出す。CLI の `list_joined_private_channels` は入力に `cursor` を足し、出力を配列から `{items, next_cursor}` に変える（旧い配列の出力を読む利用者は読めなくなる）。
- 定期の再読み込みは最初の page だけを読む。表示中の行のうち、その page の最後の channel id より後ろの行は読み込んだ続きとして残し、続きの cursor も保つ（一覧は channel id の順）。最初の page に `next_cursor` が無ければ、その page で置き換える。

#### 判定（AC-4b・AC-4c）

- AC-4b:
  - 旧 registry の固定 fixture（複数の channel、過去の世代、担当の欄の無いもの）と旧 backup の bundle から行へ移る。
    移した後の参加状態の view と担当の移行の結果が、移す前と同じになり、registry は消える。
  - 次の store の読み書きの行数が、他の channel を 10 倍にしても、対象の channel の過去の世代を 10 倍にしても変わらない（退会は、対象の channel の世代の数だけ消す）。
    - 書き込み: 参加・世代の追加・退会。
    - 起動の読み込み: lease を取る channel の参加の行と現在の世代の鍵だけ。上限を超える参加を加えても同じ。
    - 照合と読み出し: offer の照合、相手への private の応答、過去の bucket の読み出し、session の一覧、一覧、巡回。
  - 投稿者が鍵を更新して再起動した後も、過去の世代の bucket の相手からの読取りに応える。
  - 既存の private channel の試験と、harness の scenario が通る（DB を消す再起動は、上記のとおり DB を消さない再起動に改める）。
- AC-4c: #1218 の AC-4c の判定方法（join → 世代の追加 → 退会 → 旧 snapshot の再送、明示の再参加、容量の回収の後の固定の遷移。各操作は対象の項目だけを読む）。
  - 退会と、別の端末の世代の追加（redeem）が並行しても、参加に戻らない。
  - 退会の後に別の招待で再参加した端末の `membership` と鍵を採ると、届いた鍵の最新の世代で参加する（tombstone の行に残る世代へ戻らない）。

## 採らない方式

- 公開鍵から同期先を導出する: 公開鍵を知る誰もが同期先を知れる（ADR 0055 の受信 route と同じになる）。
- 1 つの鍵を識別子・namespace・暗号化に使い回す: 1 つの値の漏れがすべてに及ぶ。
- 操作の log を同期する: 件数に比例して増える。item ごとの現在の状態だけで収束する。
- account の同期を通常の private channel として作る: epoch の更新と担当（W6）に巻き込まれ、bootstrap にならない。
- private channel の鍵を世代ごとに keyring・vault の entry にする（§9）: entry が件数に比例して増え、行の索引と秘密が別の保存先に分かれる。
- 知らない channel の参加を、その replica の署名済みの metadata・policy を読んで作る（§9）: 本人の端末の item で足り、replica を持つ相手が online になるまで参加にできない。

## Consequences

- アカウント鍵を持つ端末は、公開鍵と別の同期先を持つ。同じアカウント鍵を持つ端末は区別されない（端末の強制失効はしない。#1213 の Non-goals）。
- `account::v1::` と `kukuri:account:` は予約された prefix になる。

## Data classification

ADR 0002 の template に従う。

- Feature 名: 本人の端末間の account 同期
- Durable / Transient: 採用した item の状態は Durable（item ごとに 1 行）。送信待ちは Durable。hint は Transient。
- Canonical Source: profile は既存の署名済みの envelope。その他は、検証済みの item と各端末の保存状態。
- Replicated?: 同じアカウント鍵を持つ本人の端末の間だけ（Private Replica）。
- Rebuildable From: 同期の copy を持つ本人の端末から再取得できる。すべて offline なら待つ。DB を失った端末は、手元の account の replica から作り直す（§9）。
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
