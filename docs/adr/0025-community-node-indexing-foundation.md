# ADR 0025: Community Node Indexing Foundation

## Status
Accepted

**2026-08-16 改訂**: #617 で `CommunityIndex` は `Availability::Available` へ移行済みである。
実際の読み取り面は設定と、現在の構成・配布版・判定項目に結び付いた有効な準備完了記録の
両方が揃った場合だけ公開する。

**2026-09-11 改訂**: #975 で索引状況の read 面 `GET /v1/indexing/status`（自分の申請状態と
対象の supported 判定）を追加した（§2.8）。多段ゲートと申請受付条件（#713）は変えない。

**2026-09-15 改訂**（#1051）: nsfw / objectionable の suspected は index から除外せず、`allow` verdict に
content advisory を伴って index し、`IndexEntryView.content_advisories` として client へ配信する（§7）。
`allow` のみ index する契約、DB CHECK 制約、fail-closed の順序は変えない。

## Date
2026-06-30

## Base Branch
`main`

## Related
- `docs/adr/0012-topic-first_progressive_community_filtering_draft.md`（topic-first スコープ）
- `docs/adr/0013-social-graph-foundation-draft.md`（author-owned canonical source）
- `docs/adr/0005-live-session-data-classification.md`（live session = streaming room）
- `docs/adr/0006-game-room-data-classification.md`（game / metaverse room）
- `docs/adr/0027-deterministic-moderation-critical-safety.md`（§2 component boundary / data flow / fail-closed invariants / §2.9 readiness。旧 `community-node-critical-safety.md` を集約）
- `docs/architecture/p2p-first-community-node-responsibility-boundary.md`（authority scope）
- `docs/adr/0027-deterministic-moderation-critical-safety.md`（§2.1 advisory ≠ command。旧 `moderation-event-trust-semantics.md` を集約）
- `crates/cn-operator/src/capability.rs`（`CommunityIndex` = `Availability::Available`、#617）
- `crates/cn-indexer/`（#413 Model C ingestion participant + relay validation gate + ArcadeDB 投影）
- `crates/cn-core/src/index_scope.rs` + `crates/cn-core/migrations/202607010001_index_scope.sql`（#413 scope 管理 state）
- 実装側 Issue: #404（fail-closed community indexing 本体）, #413（Model C ingestion / supported-topic scope / indexing request）
- 後続 ADR: trust / relation foundation（#409）, 決定論的 moderation（#410）, 非決定論的 moderation（#411）

## 位置づけ

community node の `indexing`（`index` / `search` / `discovery` / `recommendation`）の責務境界を固定する基礎の設計判断記録である。索引の実体と安全側に倒す読み取り境界は #404、共有文書からの取り込みは #413、実行時の準備判定と有効化関門は #616、提供中への移行は #617 で実装した。本記録は、その土台として **何を索引し、何を索引しないか**を固定し、`trust` / `relation`（#409）・`moderation`（#410/#411）との境界を説明する。

本 ADR は indexing の **scope（範囲）と content kind（対象種別）と fail-closed（安全）** の境界を定義する。trust / relation への risk signal 反映、provider 接続、ranking/recommendation アルゴリズムの詳細は本 ADR のスコープ外（後続 ADR / Issue）。

## Feature Data Classification
- Feature 名: community node content index（topic-scoped post 本文テキスト + media 派生タグ + room メタデータ）
- Durable / Transient: Durable な node-local server index state（再構築可能な derived projection）
- Canonical Source: index は canonical ではない。canonical source は author-owned のまま（post 本文は topic replica、room は `live/<id>/state` / `game/<id>/state` の docs pointer + manifest blob）。CN は ingestion = Model C（§6）で sync 元の topic / channel docs replica（public は導出 namespace、private は登録 capability）から取り込む。index は派生した検索用テキスト + media 派生タグ + メタデータ + safety verdict state のみを持つ
- Replicated?: No（index 自体は node-local。client へ canonical として replicate しない）。CN は supported topic / 許可 channel の **docs sync peer として参加**する（§6 Model C）。検索/発見結果は node の authority scope 内で API として提供する
- Rebuildable From: sync した topic / channel replica の post 本文・room state pointer + safety scan（VLM タグ含む）の verdict。再 ingest + 再 scan で再構築でき、docs+blob で backfill / restart 復元する
- Public Replica / Private Replica / Local Only: node-local な private server state（`cn-core` / Postgres）。public manifest には capability の有無のみを載せ、index 中身は載せない
- Gossip Hint 必要有無: peer discovery 補助として使用（Model C, §6）。基本データ経路は docs replica sync
- Blob 必要有無: No（**no permanent blob storage** を維持。raw media blob は index しない。index は VLM 由来の派生タグのみ。moderation server の一時 fetch のみ）
- SQLite projection 必要有無: No（server は SQLite を使わない）
- index 投影ストア: **ArcadeDB**（Lucene 全文検索付き multi-model DB。canonical ではない derived な写像）。#413 で決定・改訂（当初は「Postgres projection」としていたが、ADR 0026 §6.1 で relation graph 用に採用する ArcadeDB に index 投影も相乗りさせ、全文検索 + co-participation graph を単一エンジンに統合する）。**全文検索のみ**を今回のスコープとし、ベクトル検索は延期する（§4 の画像類似検索除外と整合）。scope 管理 state（supported set / user request / private channel capability）は node-local な制御情報として **Postgres（`cn_index` schema）** に置く（index 投影本体とは別）
- 必須 contract:
  - `index_scope_limited_to_operator_supported_topics`
  - `index_rejects_topic_outside_supported_set`
  - `index_admits_approved_user_indexing_request`
  - `index_text_post_body_only`
  - `index_media_searchable_via_derived_tags`
  - `index_excludes_raw_media_blob`
  - `index_streaming_room_metadata_only_excludes_in_room_comments_actions`
  - `index_metaverse_room_metadata_only_excludes_in_room_activity`
  - `index_only_allow_verdict_content`
  - `index_excludes_unscanned_and_scan_failed`
  - `search_discovery_recommendation_excludes_non_allow`
  - 2026-09-15 追加（#1051、§7）: `general_nsfw_is_indexed_with_advisory_label` / `index_entry_advisories_derive_from_latest_verdict` / `content_advisories_are_separate_from_signed_content_labels`
  - `index_only_indexes_shared_replica_entries`
  - `index_private_channel_requires_submitted_channel_secret`
  - `index_private_channel_request_authz_reuses_channel_permission`
  - `indexing_startup_requires_validated_relay`
  - `indexing_startup_fails_without_own_or_external_relay`
  - `indexing_status_returns_own_requests_only`（#975）
  - `indexing_status_private_target_requires_membership_proof`（#975）
  - `indexing_status_reads_do_not_mutate_scope_state`（#975）
- 必須 scenario:
  - operator が topic を supported set に追加 → その topic の post 本文が検索に出る / supported 外 topic の post は検索に出ない
  - 画像のみの投稿は、本文が無くても VLM 派生タグで検索でき、raw blob 自体は index されない
  - exclude された media のタグは index されない（`allow` media のタグのみ検索に出る）
  - streaming / metaverse room は title / description / タグで検索/発見に出るが、その room 内のコメント・action は出ない
  - トピック毎の検索窓で topic 内検索ができ、CN 横断検索は別画面で supported topic 全体を横断する
  - unscanned / scan_failed / `allow` 以外の verdict の content は search / discovery / recommendation に出ない
  - private channel の secret を提示した indexing リクエストで `channel::` replica が sync され検索に出る / secret 無しでは index されない
  - 自前 relay も外部 relay も未設定の CN は indexing 起動に失敗する（fail-closed）
  - 共有 replica に実在しない content（CN へ直接渡されただけ）は index されない

## 1. 背景

- `community_index` / `moderation` / `community_local_trust` は #617 で `Availability::Available` へ移行した。`community_index` の検索・発見・おすすめは実装済みだが、設定だけでは公開せず、有効な準備完了記録も要求する。
- index は content-surfacing path（content を公開的に浮上させる経路）であり、安全設計が無いまま解禁してはならない（critical-safety.md §1）。
- index を「無制限に何でも拾う」設計にすると、(a) operator が責任を負えない範囲まで authority scope が膨張し、(b) media を含む有害 content の surfacing 経路が広がる。本 ADR はこれを scope と content kind の両面で**絞る**ことを既定とする。

## 2. Decision

### 2.1 index は node-local な derived projection（canonical ではない）

- index は **issuer node の authority scope 内**に閉じた node-local projection である。canonical source（author-owned の post / room）を所有・改変しない。
- index を消すことは canonical content を消すことを意味しない。index への登録/除外は「この node が自分の検索・発見・推薦にこの content を出すか」という node-local 判断に限定される（p2p-first responsibility boundary に従う）。
- user identity / profile / social graph は node-independent であり、index 化の対象としても canonical を所有しない。

### 2.2 無制限な indexing の防止 — scope は operator-supported topics に限定し、user indexing request も受ける

- index 対象は **operator が指定した supported topic の集合**に限定する。supported set 外の topic の content は index しない（`index_scope_limited_to_operator_supported_topics` / `index_rejects_topic_outside_supported_set`）。
- supported topic set は node-local な運用 state として持ち、運営 UI / CLI から変更できる（admin 画面の「サポートする topic を選択する機能」= #382 と接続）。
- **user からの indexing request を受け付ける**。ユーザーは「この topic / この content を index してほしい」と要求できる。ただし request は **index を保証しない**:
  - request された topic を supported set に入れるかは operator policy の判断。
  - supported になっても、個々の content は §2.5 の安全ゲートを通過した `allow` のみが index される。
  - つまり `request → operator 承認（supported 化）→ 安全 verdict 通過 → index` の多段ゲートとする（`index_admits_approved_user_indexing_request`）。
- この設計により、index の authority scope は「operator が明示的に引き受けた topic」に常に限定され、無制限に膨張しない。
- **index 対象は public topic に限らない。** 招待制（身内向け）CN（ADR 0024 admission）では **private channel に対する indexing request も想定**する。private channel の index は §6.3 の条件（indexing リクエスト＝secret 送信 / リクエスト権限は channel 権限モデルの応用 / scope を channel メンバー + その CN の authority に閉じる / visibility は `local` 寄り）を満たすことを必須とする。public topic（namespace 秘密が導出可能）と private channel（`channel::`、capability 必要）は取得経路が異なる（§6）。

### 2.3 index 対象の content kind — テキスト本文 + media の派生メタデータ（タグ）。raw blob は index しない

- index に格納・検索対象とするのは次に限る:
  - post 本文テキスト（`index_text_post_body_only`）
  - media の **派生メタデータ（タグ）**。media は moderation 過程で VLM（非決定論的 moderation, #411）を必ず通すため、そこで生成される descriptive tag / metadata を index し、media を **タグ経由で検索可能**にする（`index_media_searchable_via_derived_tags`）。
  - 最小限の共通メタデータ（post id / author pubkey / topic / timestamp / safety verdict state）
- **raw blob（画像 / 動画 / その他ファイルのバイト列）・知覚ハッシュ・サムネイルは index しない**（`index_excludes_raw_media_blob`）。index するのは VLM 由来の派生タグのみ。これは **no permanent blob storage**（critical-safety.md §3/§8）と整合する。
- 派生タグは `allow` verdict の media に対してのみ生成・index する。critical safety で exclude された media は index しない。CSAM 等の Match Data / 生検知結果はタグや index に流さない（#391 / #411 の非ゴールと整合）。
- **タグのサムネイル代替表示**: client は読み込み中、または安全用の代替表示（特にアダルト / 暴力的コンテンツ）として、サムネイルの代わりにこのタグを表示してよい。具体的な表示挙動は client UI 側の設計（本 ADR スコープ外）だが、index がタグを保持することで成立する。

> **2026-09-15 改訂**（#1051、§7.3）: label 付き `allow`（nsfw / objectionable の suspected）の media もタグ化する。
> 成人向け・暴力的表現の代替表示の一次判定は `content_advisories`（ADR 0046 §6）に移り、タグは補助情報とする。

### 2.4 streaming / metaverse — room メタデータのみ index する。room 内の activity は index しない

- streaming（live session, ADR 0005）/ metaverse・game room（ADR 0006）は、**room メタデータ**のみを index・検索対象にする。具体的には room id / topic / title / description テキスト / タグ。
  - room タグは現状未実装の可能性がある。実装され次第 index 対象に含める（未実装の間は title / description テキストを対象にする）。
- room メタデータのテキスト・タグは検索可能にする（room を見つけるための検索）。
- **room 内のコメント・chat・action・score・presence などの in-room activity は index しない**（`index_streaming_room_metadata_only_excludes_in_room_comments_actions` / `index_metaverse_room_metadata_only_excludes_in_room_activity`）。in-room の逐次 activity は discovery / search / recommendation の対象にしない。

### 2.5 安全設計 — fail-closed。`allow` verdict の content のみ index する

index への登録は、**scope ゲート（§2.2/§2.3/§2.4）と safety ゲート**の両方を通過した content のみとする。safety ゲートは critical-safety.md §8 の fail-closed 不変条件を index 本体の制約として固定する:

- scan 前（unscanned）の content は index しない（`index_excludes_unscanned_and_scan_failed`）。
- scan failure / provider unavailable は `allow` に倒さない。fail-closed（index しない）。
- `hold` / `quarantine` / `exclude` verdict の content は search で返さない。
- critical verdict は discovery / recommendation に入れない。
- `allow` verdict の content のみが search / discovery / recommendation に出る（`index_only_allow_verdict_content` / `search_discovery_recommendation_excludes_non_allow`）。

safety verdict は `cn-safety-runtime` の `SafetyVerdict` を index 反映の前段に組み込むことで供給する。index entry は対応する safety verdict state を必ず伴う（verdict 無しの index entry を作らない）。

> **2026-09-15 改訂**（#1051、§7.1）: nsfw / objectionable の suspected は `allow` verdict に `advisory_labels` を伴って
> 本節のゲートを通過する。`allow` のみ index する契約は文言どおり維持され、advisory は verdict state の一部として
> index entry から参照される。

### 2.6 二重ゲートの順序

content が index に入る条件は次の AND とする:

1. **scope ゲート**: topic が supported set 内（operator 指定 or 承認済み user request）であり、content kind が index 可能（post 本文テキスト / media 派生タグ / room メタデータ）であること。
2. **safety ゲート**: safety verdict が `allow` であること（unscanned / scan_failed / provider_unavailable / 非 `allow` は不可）。

どちらか一方でも満たさない content は index されず、search / discovery / recommendation に出ない。

### 2.7 検索 UX — トピック毎の検索窓を基本とし、CN 横断検索は別画面

- 基本 UX は **トピック毎の検索窓**（topic-scoped search）。あるトピックの中で検索するのが既定の体験。
- **CN ベースのトピック横断検索（cross-topic search）は別画面**として用意する。supported topic set 全体を対象に、node の authority scope 内で横断検索する。
- どちらの検索面も §2.2 の supported-topic scope と §2.5 の safety ゲートに従う（横断検索でも supported 外 topic / 非 `allow` content は出ない）。
- **横断検索の対象は supported set のうち公開 scope（public topic）全体**（#711 で明確化）。非公開チャンネルの索引は §6.3 の閲覧境界（チャンネル参加者に閉じる）に従い、横断検索・発見・推薦には項目も識別子も出さない。非公開チャンネルの読み口は所属証明つきの範囲指定読みだけである。「supported set 全体」と「参加者に閉じる」は矛盾しない: 横断面は公開 scope の集合、非公開 scope は範囲指定面、と読み口を分ける。

### 2.8 索引状況の read 面 — 自分の申請状態と対象の supported 判定（#975）

利用者は申請（§2.2）の結果を申請時応答でしか知れず、対象が supported set に入っているかも
読めなかった。#975 で認証・同意済み利用者向けの読取り面 `GET /v1/indexing/status` を追加し、
「見つける」の空状態と申請 dialog が確定した状態と未確認を区別できるようにする。

- **返すもの**: `requests` = 呼出し主（bearer identity）の申請だけ（`pending / approved / rejected`、
  申請・判定時刻）。却下済みも含む。`target` = `scope_kind` + `scope_id` を指定した場合だけ、その
  対象が supported set に含まれるか（`supported`）。`supported` は §2.6 の 1 段目（scope ゲート）
  だけを表し、個々の投稿が検索に出るかは safety ゲートと sync の反映に依存するため、client は
  これだけで「索引済み」とは断定しない。
- **返さないもの**: 他利用者の申請、supported set 全体の一覧、非公開チャンネルの索引有無
  （所属証明なし）。manifest には引き続き supported topic を載せない（§4）。
- **門と順序**: 申請と同じ `require_indexing_gate`（索引参照が構成済み → 準備完了記録が有効 →
  bearer → consent）。未構成・失効は認証より先に 404（`INDEXING_REQUEST_NOT_CONFIGURED` /
  `INDEXING_REQUEST_NOT_ACTIVATED`。#713 と同じコードを再利用）。申請できないノードの申請状態は
  語らない。
- **非公開チャンネルの閲覧境界（#711 を read に適用）**: `target` が `private_channel` のときは
  範囲指定読みと同じ所属証明ヘッダ（`x-kukuri-channel-secret`）を要求し、未提示・不一致・未登録・
  鍵未設定を同一の 403 `CHANNEL_MEMBERSHIP_REQUIRED` で拒否する。自分の申請一覧（`target` 無指定）
  は所属証明を要求しない。申請者自身が提出した対象識別子を本人へ返すだけで、非参加者に索引の
  存在有無を漏らさない。
- **読取り専用**: `supported_topics` / `indexing_requests` / `channel_secrets` を書き換えない。
  承認は operator の CLI 判断のまま（§6.6）。
- **client 側**: desktop-runtime は申請と同じ順序（session → 同意 → 秘密値 → token → HTTP）で
  送り、非公開チャンネルの `supported` 判定だけが所属証明を伴う。所属証明の送信は申請 dialog の
  明示確認後に限り、空状態からの自動取得は所属証明を伴わない（一覧と公開 topic のみ）。応答は
  永続化せず、表示 state でだけ保持する（再取得で再構築できる transient な写し）。
- contract: `indexing_status_returns_own_requests_only` /
  `indexing_status_private_target_requires_membership_proof` /
  `indexing_status_reads_do_not_mutate_scope_state`。実装 test は
  `crates/cn-user-api/tests/indexing_requests.rs` の
  `indexing_status_returns_own_requests_only_and_public_target_support`（1 件目と 3 件目を同じ
  test 内の `scope_state_counts` 比較で固定）と
  `indexing_status_private_target_requires_membership_proof`、client 側は
  `crates/desktop-runtime/src/tests/community_node/indexing_status.rs`。

#### Feature Data Classification（read 面）
- Feature 名: community node indexing status read（自分の索引申請状態 + 対象の supported 判定）
- Durable / Transient: Transient（client は保持しない。server 側は既存の `cn_index` scope state を読むだけ）
- Canonical Source: `cn_index.indexing_requests` / `cn_index.supported_topics`（node-local な運用 state。既存）
- Replicated?: No（node-local。他 node へ複製しない）
- Rebuildable From: 同 endpoint の再取得
- Public Replica / Private Replica / Local Only: node-local server state の本人向け読取り。非公開チャンネルの `supported` は所属証明つき
- Gossip Hint 必要有無: No
- Blob 必要有無: No
- SQLite projection 必要有無: No（client は表示 state のみ）
- 必須 contract: 上記 3 件
- 必須 scenario: なし（contract test で固定。harness scenario への追加は Optional-hardening として対象外）

## 3. Consequences

- index は「拾えるものを全部拾う」のではなく、operator が引き受けた supported topic × index 可能な content kind × `allow` verdict の交差に限定される。authority scope が常に説明可能になる。
- media 検索は VLM 由来の派生タグ経由で提供する（raw blob の内容検索・画像類似検索は提供しない）。media タグ生成は非決定論的 moderation（#411）に依存する。
- streaming / metaverse の検索体験は「room を見つける」までで（title / description / タグ）、room 内発言の全文検索は提供しない。
- ingestion = Model C を基本とするため、CN は supported topic / 許可 channel の **docs sync participant node を新たに常駐**させる（純 relay + KV rendezvous の現行構成を一段拡張）。indexing 起動時に **relay validation** を gate として通し、自前 relay が無ければ外部 relay 設定を必須化する。Model B / A は Appendix（optional）。
- private channel の indexing は「indexing リクエスト＝secret 送信」で C に capability を注入して解決し、別 ingestion 経路を新設しない。リクエスト権限は channel の権限モデルを応用する。
- index 本体（schema / storage / query boundary）と fail-closed 制約の実装は #404 が担う。本 ADR は #404 が満たすべき contract / scenario の必要集合を先に固定する。
- `CommunityIndex` capability は、§2 の範囲・内容・安全性の関門と ADR 0027 §2.9 の準備完了条件を #616 の実行時関門で固定した後、#617 で `Availability::Available` へ移行した。提供可能であることと個々の配備で読み取り面を公開することは分離し、後者は設定と有効な準備完了記録の両方を要求する。

## 4. Out of scope（後続へ申し送り）

- trust / relation reads への risk signal 反映経路（#406 / trust-relation ADR #409）。
- 決定論的 / 非決定論的 moderation の verdict 生成詳細（#410 / #411）。
- ranking / recommendation アルゴリズム、関連度スコアリングの具体。
- 画像類似検索 / raw blob 内容検索（タグ経由の検索は本 ADR で扱う）。
- tag 語彙（tag vocabulary）の標準化と VLM タグ生成の詳細（#411）。
- public manifest での index capability の表現詳細（capability メタデータ更新）。

## 5. 維持する境界

- index は node-local projection であり network-wide な canonical store ではない。
- index への登録/除外は node-local 判断であり、他 node や user の canonical state を変更しない。
- **no permanent blob storage** を維持する（raw media blob を index に取り込まない。index するのは VLM 由来の派生タグのみ）。
- `allow` 以外、unscanned、scan_failed、provider_unavailable を surfacing 経路に出さない（fail-closed）。

## 6. 投稿・room 情報の取得経路（ingestion path）= Model C を基本とする（Decision）

index する content（post 本文・media タグ・room メタデータ）を community node がどこから / 誰から / どの経路で受け取るかを、**Model C（topic / channel の docs replica を sync する canonical pull）を基本（required）**として確定する。Model B / Model A は **Appendix A（実装してもよいが必須ではない optional）**とする。

前提（AGENTS.md 通信経路）: 基本優先度は `Direct P2P -> Relay Supported P2P -> Relay Fallback`。`cn-user-api` が topic rendezvous state の owner。community node は P2P network の **service provider** であって home server ではない。

### 6.1 なぜ C を基本にするか（cost / attack surface）

- **コスト**: 総コストは safety scan（特に media VLM）が支配し、これは ingestion 経路に依らず共通。ingestion の差は相対的に小さい。replica sync は iroh-docs の **delta 同期（range 突合）**で per-post を償却し、late-join backfill / restart 復元が docs+blob だけで成立する（ADR 0005/0006）。「常駐プロセスが無い」ことを A の優位とみなすのは誤り（`cn-user-api` は既に常駐し、A は backfill 不能）。
- **attack surface（ghost 注入）**: post envelope の署名検証は self-contained（`KukuriEnvelope::verify`、pubkey 埋め込み schnorr）。しかし有効署名は「実際に共有 replica / gossip に存在した投稿」であることを証明しない。Model A は **CN に直接 POST された、ネットワークに流れていないゴースト投稿**を index し得る（CN 限定の私的注入面を新設し、index がネットワーク実体から乖離する）。Model C は **全参加者と同じ共有 replica の entry のみ**を index するため、この私的注入面を作らない。
- 結論: C は index を共有実体に一致させ、A 固有の注入面を持たず、per-post コストも償却できる。

### 6.2 Model C の決定内容

- **public topic**: `topic_replica_id(topic)`（= `topic::<topic_id>`）から namespace を導出し、`public_replica_secret` で replica を open、peer を discovery して iroh-docs sync する（`crates/docs-sync/src/{access,iroh_sync}.rs`）。
- **ghost 注入を作らない**: index 対象は **sync された共有 replica に実在する entry のみ**。CN に直接渡されただけで replica に存在しない content は index しない（`index_only_indexes_shared_replica_entries`）。
- **blob**: scan のための一時 fetch のみ。恒久保存しない（no permanent blob storage）。

### 6.3 課題 1 の解決: private channel indexing = secret 送信

- 招待制（身内向け）CN（ADR 0024 admission）では private channel への indexing request も想定する。private channel replica（`channel::`）は namespace 秘密が導出できず capability（登録済み secret）が必要（`access.rs`）。
- **解決**: **indexing リクエスト＝secret 送信**とする。リクエスト時に channel secret（capability）を CN に渡し、CN はそれを登録して **Model C と同じ仕組みで `channel::` replica を sync** する。private channel 専用の別 ingestion 経路は新設せず、C に capability を注入するだけで解決する。
- **リクエスト権限**: indexing をリクエストできる権限は **channel の既存権限モデルをそのまま応用**する（channel の secret にアクセスできる権限者が、その secret を提示して indexing をリクエストできる）。CN は新しい権限体系を作らない。
- scope / consent: private channel index は channel メンバー + その CN の authority に閉じ、risk signal / visibility は `local` 寄り（trust-semantics）。
- **閲覧境界（read 側。#711）**: 「channel メンバーに閉じる」は読み口にも適用する。範囲指定読み（`scope_kind = private_channel` の search / discovery）は、申請と同じ「secret を提示できること自体が権限の証明」の原則で **channel secret の提示を所属証明として要求**し、保存済み capability の復号値と定数時間比較で照合する。秘密値はアクセスログへ露出する URL クエリでなく専用ヘッダ（`x-kukuri-channel-secret`。`cn-protocol` に定数化）で送る。未提示・不一致・チャンネル未登録・暗号鍵未設定は同一の安定コード `CHANNEL_MEMBERSHIP_REQUIRED`（403）で拒否し、非所属者に索引の存在有無を漏らさない。横断読み（scope 無指定）には非公開チャンネルの項目を出さない（§2.7）。
- contract: `index_private_channel_requires_submitted_channel_secret` / `index_private_channel_request_authz_reuses_channel_permission` / `cross_scope_reads_exclude_private_channel_entries` / `private_channel_reads_are_limited_to_members_with_secret_proof`。
- **申請の受付条件（#713）**: indexing request の受付は、そのノードで索引参照が**構成済みかつ有効化（準備完了記録）が有効**であることを必須とする（Decision。「申請だけ受け付けて索引時に判定する」案は不採用）。索引を提供しない・提供が停止中のノードが申請（private channel では channel secret を含む）を受理・保存するのを防ぐ — 「秘密値の提示が権限の証明」は、提示先が索引を実際に提供していることの確認とセットで初めて意味を持つ。未構成は `INDEXING_REQUEST_NOT_CONFIGURED`、有効化失効は `INDEXING_REQUEST_NOT_ACTIVATED`（いずれも 404。検査は read 面と同じ順で認証より先）。拒否時は申請行も channel secret も保存されない。#698 の client 側送信前判定と対になるサーバ側の門。contract: `indexing_request_is_rejected_when_index_query_not_configured` / `indexing_request_is_rejected_when_activation_is_stale`。

### 6.4 課題 2 の解決: relay validation（relay 抜き CN）

- CN は relay 抜き構成を許容するため、Model C の peer discovery を CN-local relay 前提にできない。
- **解決**: **indexing 起動時に relay を validate する**。自前 relay（`cn-iroh-relay`）が無い構成では、**外部 relay の設定を必須化**する。relay（自前 or 外部）が未設定なら indexing を起動しない（fail-closed の起動 gate）。
- これにより relay 有無に依らず C の discovery（seed peer / imported ticket / 外部 relay / DHT。`iroh_sync.rs` の `seed_peers` / `learned_peers` / `imported_peers`）が成立する。
- contract: `indexing_startup_requires_validated_relay` / `indexing_startup_fails_without_own_or_external_relay`。

### 6.5 Feature Data Classification への反映（C 確定）

- Canonical Source: index は派生。canonical は sync 元の topic / channel docs replica（public は導出 namespace、private は登録 capability）。
- Replicated?: index 自体は node-local。CN は supported topic / 許可 channel の **docs sync peer として参加**する。
- Rebuildable From: sync した replica（topic / channel）+ safety scan。docs+blob で backfill / restart 復元。
- Gossip Hint: peer discovery 補助として使用。基本データ経路は docs replica sync。
- index 投影ストア: ArcadeDB（全文のみ、ベクトル延期）。scope 管理 state は Postgres（`cn_index`）。

### 6.6 実装（#413）

Model C ingestion と scope / request / relay validation は #413 で実装した。

- **crate 構成**: docs replica sync participant を新 crate/binary `cn-indexer` に分離する（`cn-user-api` = HTTP/DB/rendezvous、`cn-iroh-relay` = 純 relay の責務を汚さない）。
- **scope 管理 state**: `cn-core` の `cn_index` schema（`supported_topics` / `indexing_requests` / `channel_secrets`）。channel secret（capability）は **at-rest 暗号化（XChaCha20Poly1305）**して保存し、平文は列に残さない。復号鍵は runtime（Secret Manager / env 注入 `COMMUNITY_NODE_CHANNEL_SECRET_KEY`）が供給する。
- **運用面**: operator は `cn-cli`（`supported-topic` / `indexing-request` サブコマンド）で supported set と request 承認を運用する。user の indexing request は `cn-user-api` の `POST /v1/indexing/requests`（認証 + consent）で受ける。承認は operator が CLI で行い、auto-approve は作らない。#382 admin UI は別 Issue。
- **relay validation 起動 gate**: `cn-indexer` は起動時に config 検査で relay を validate する（自前 relay = operator の `iroh_relay` capability、または外部 relay URL のどちらか。両方未設定なら indexing を起動しない）。到達性 probe はしない。
- **ingest → 投影**: 共有 replica の実在 post entry のみを `cn-safety-runtime` の orchestrator で scan し、`allow` verdict のみ ArcadeDB 投影へ書く（unscanned / scan_failed / 非 allow は投影しない fail-closed）。supported 除外 / channel secret 失効時は sync 停止 + de-index する。media は scan/tag pipeline へ渡す接続点まで（VLM 本体は #411）。
- **seam（#404 との境界）**: #413 は ingest → 投影 + 投影レベル read（`allow` entry の存在確認）まで。ユーザー向け search / discovery / recommendation 本体と fail-closed query gate は #404。

### 6.7 実装（#404）

fail-closed indexing 本体（DB 制約 + query 境界）は #404 で実装した（詳細は
`docs/progress/2026-07-02-404-fail-closed-community-indexing.md`）。

- **verdict state / index 真実源**: `cn_safety.scan_verdicts`（対象ごとの最新 verdict。`allow` 含む）と
  `cn_index.index_entries`（index の真実源）。§2.5 の「index entry は対応する safety verdict state を
  必ず伴う」を NOT NULL FK で、「`allow` verdict の content のみ」を `CHECK (verdict_action = 'allow')`
  と `CHECK (NOT critical)` で **DB 制約として**固定した。ArcadeDB 投影は真実源の derived な写像。
- **query 境界**: `cn-indexer` の `IndexQuery`（topic 内検索 / 横断検索 / 新着列挙）+
  `FailClosedIndexQuery`（唯一のユーザー向け読み口。投影 hit を真実源 + 最新 verdict と突合して
  非 allow / critical / 残留 hit を落とす）。`cn-user-api` の `GET /v1/index/{search,discovery,recommendations}`
  （認証 + consent + rate limit）が公開面。
- **安全側の公開関門**: `CommunityIndex` 自体は #617 で `Availability::Available` へ移行済み。
  ただし `COMMUNITY_NODE_INDEX_QUERY_ENABLED` は既定 false で、真にしても現在の profile、
  operator config、配布版、判定項目、有効期限に一致する準備完了記録が無ければ検索・発見・
  おすすめの読み取り面を公開しない。条件不成立時は 404 へ倒す。
- **スコープ外のまま**: ranking / 関連度スコアリングの具体（§4）。discovery / recommendation は
  created_at 降順の新着列挙を最小 surface とする。

## Appendix A: 代替・補助 ingestion モデル（B / A, optional）

以下は Model C を補完する optional モデル。**実装してもよいが必須ではない。** 採用する場合も §6.1–§6.4 の不変条件（ghost 注入を作らない / relay validation / private は capability）を満たすこと。

### Model B: CN が gossip 参加者として受信（liveness 補助, optional）

- CN が supported topic の gossip を subscribe し、新着の liveness hint を得て次 peer へ forward する（sink ではなく Relay Supported P2P の良き参加者）。C の canonical sync を低遅延化する補助。
- index は依然 §6.2 の「共有 replica の entry のみ」に従う（gossip だけで来た未 replica content を直接 index しない）。

### Model A: client が cn-user-api へ明示 submit（opt-in 例外, optional）

- client が index 許可付きで post を CN の API に submit する opt-in 経路。
- 必須ではない。採用する場合、ghost 注入を防ぐため **submit された content も共有 replica 上の実在を確認してから index する**（単独 POST だけで index しない）。確認できないなら index しない。

## 7. 改訂追補（#1051、2026-09-15）: content advisory 付き index entry

本節は 2026-09-15 の製品決定（Issue #1051、ADR 0028 §8）のうち index に関する部分を記録する。

### 7.1 verdict state に advisory を持つ
- nsfw / objectionable の suspected は `allow` verdict に `advisory_labels` を伴って index される
  （`general_nsfw_is_indexed_with_advisory_label`）。index 真実源 `cn_index.index_entries` の
  `CHECK (verdict_action = 'allow')` / `CHECK (NOT critical)`（§6.7）と、`FailClosedIndexQuery` の
  「真実源 + 最新 verdict と突合して非 allow を落とす」境界は不変。
- advisory は `cn_safety.scan_verdicts` 側（`advisory_labels`）に保持し、index entry は既存の verdict FK を
  通じて最新の advisory state を参照する（`index_entry_advisories_derive_from_latest_verdict`）。
  index entry 自体に verdict 以外の判定列は増やさない。

### 7.2 wire: `IndexEntryView.content_advisories`
- `GET /v1/index/{search,discovery,recommendations}` の `IndexEntryView` に `content_advisories`
  （ADR 0028 §8.6 の要素: issuer_node_id / subject_kind / subject_id / category / label / confidence /
  signal_id / basis）を追加する。`text` と同じく canonical content の信頼元にせず、client は署名済み投稿を
  ローカル解決した後も第 2 のラベル源として保持する（ADR 0046 §6）。
- 署名済み `content_labels` とは別欄であり、index は `content_labels` を生成・改変しない。
- blob 単位の advisory（`subject_kind = blob_cid`）は post entry に同梱し、client の hash 単位取得ゲートに使う。

### 7.3 派生タグと代替表示
- §2.3 の「派生タグは `allow` verdict の media に対してのみ生成・index する」は文言どおり維持し、label 付き
  `allow` の media もタグ化する。critical / Match Data / 生スコアの除外は不変。
- §2.3 の「タグのサムネイル代替表示」について、成人向け・暴力的表現の代替表示の一次判定は
  `content_advisories`（と self-label）に移し、タグは補助情報とする。

### 7.4 contract
- 追加: `general_nsfw_is_indexed_with_advisory_label`、`index_entry_advisories_derive_from_latest_verdict`、
  `content_advisories_are_separate_from_signed_content_labels`（ADR 0028 / 0046 と共有）。
- 維持: `index_only_allow_verdict_content`、`index_excludes_unscanned_and_scan_failed`、
  `search_discovery_recommendation_excludes_non_allow`、`index_media_searchable_via_derived_tags`、
  `index_excludes_raw_media_blob`。

### 7.5 変更しないもの
- scope ゲート（§2.2 / §2.4）、fail-closed の順序（§2.6）、ingestion Model C、readiness と公開関門（§6.7）。
- 過去に `Exclude` された投稿の backfill は `policy_version` 更新に伴う再 ingest で反映し、専用 migration は
  行わない（ADR 0028 §8.9）。

### 7.6 保存済み verdict の再利用と risk signal の集約（#1050、2026-09-15）

- indexer は pass ごとに scope 全件を走査するが、subject の**内容 fingerprint**（旧形式post =
  `objects/<id>/state` レコードの content hash、v1 bucket post = 検証済み署名ID、blob = blob hash）と **scan 構成 fingerprint**
  （`SafetyPolicy` の serde 表現 + provider の `config_fingerprint()` の sha256）が保存済み verdict
  （`cn_safety.scan_verdicts.source_fingerprint` / `scan_config_fingerprint`）と一致する限り provider を
  呼ばず、保存済み verdict と `derived_tags` を再利用する。再利用時は moderation artifact
  （risk signal / signed event）を生成しない。
- **fail-closed の verdict（scan failure / provider unavailable / unscanned / hold）は再利用しない**。次の
  pass で必ず再試行し、provider 復旧時に `allow` へ更新する。TTL による定期再検証は置かない（再 scan の
  契機は内容変化と scan 構成変化のみ。既知 hash DB の更新を既存 index に反映するには policy /
  provider 構成の更新で再 scan を起こす）。
- 撤回・tombstone・送信防止・envelope 検証は再利用判定より前に評価し、再利用より優先して de-index する。
- risk signal は鍵 `(issuer_node_id, target, target_id, category, basis)` の活性行を 1 件に保つ
  （部分 UNIQUE index `uq_cn_safety_risk_signals_active_key`）。同鍵の再 scan は既存行の severity /
  confidence / visibility を更新し、id / persisted_at / appeal_status を据え置く。失効していない
  `cleared` 行があれば新規行を作らない。signed moderation event は signal が新規作成されたとき、または
  verdict の action / reason_code / critical が変わったときだけ発行する。
- 変更通知（DocEvent）駆動の取り込みは、変更 key に対応する object（`objects/<id>/…` と
  `withdrawals/<id>/state`）だけを処理し、対象を特定できない key は scope 全体の見直しへ倒す。
  300 秒の全件見直しは reconciliation として不変。
- 変更 key は共有 replica の key 種別表（`kukuri_docs_sync::SharedReplicaKeyFamily`、#1065）で分類する。

  | 種別 | prefix | 取り込み |
  | --- | --- | --- |
  | 投稿 object / 撤回 | `objects/`、`withdrawals/` | 当該 object だけ |
  | media manifest | `manifests/media/` | scope 全体（参照元 object を特定しない） |
  | 索引・reaction・envelope・session・channel・metaverse | `indexes/timeline/`、`indexes/thread/`、`reactions/`、`envelopes/`、`sessions/`、`channels/`、`metaverse/` | 無視 |
  | 未登録 | それ以外 | scope 全体 |

  無視できる種別は indexer が読む種別（`objects/`・`withdrawals/`・`manifests/media/`）と交わらない。
  種別の追加は cn-indexer の `key_disposition` で取り込み方の判断を強制する（ワイルドカードを置かない）。
  scope 全体へ倒した回数と直近の理由（種別 prefix。識別子は含めない）は `/v1/status` の
  `event_whole_scope_fallbacks` / `last_whole_scope_fallback_reason` に出す。
- contract: `second_pass_with_unchanged_content_performs_no_provider_calls`、
  `held_verdict_is_never_reused`、`rescan_with_same_key_updates_signal_instead_of_inserting`、
  `cleared_signal_with_same_key_is_not_resurrected`、
  `identical_rescan_emits_no_new_event_but_verdict_change_does`、
  `dedupe_migration_compresses_duplicates_keeping_referenced_disputed_and_oldest`、
  `client_post_change_keys_ingest_only_that_object`、`non_indexing_change_keys_do_not_ingest`、
  `ignored_key_families_never_include_what_the_indexer_reads`、
  `shared_replica_writes_use_only_registered_key_families`（app-api）。

### 7.7 取り込みの一時的な失敗と de-index の区別（#1090、2026-09-17）

- v1 bucketの候補選別は[ADR0054](0054-time-bucketed-docs-replicas.md)のデータ分類に従う。
  配置・署名を確認できない候補は、別sourceで確認済みの索引を消す根拠にしない。未署名markerの
  書換えは署名IDの判定再利用を失効させず、走査中の現物envelopeの取得不能・検証不能は一時的失敗とする。
  以下のstate破損・state再読不一致の扱いは旧形式に適用する。
- 1 件の取り込みの失敗は、確定した理由と一時的な失敗に分ける（cn-indexer `ingest::failure`）。
  印の無い失敗は確定した理由として扱う。

  | 分類 | 例 | 既存 entry | 新規・未索引の投稿 |
  | --- | --- | --- | --- |
  | 確定した理由 | 撤回、削除・tombstone、送信防止、scope 非対応、state / envelope の破損・署名不一致、参照再確認での state・envelope・media 参照の変化、本文の検証失敗（サイズ・hash・UTF-8・上限）、manifest の欠落・検証失敗、非 allow の verdict | 真実源 → 投影の順で de-index | 索引しない |
  | 一時的な失敗 | replica の照会失敗（参照再確認の `LocalOnly` 照会、manifest 照会）、本文 blob の一時取得失敗・未取得、真実源の読み取り失敗、判定記録・advisory・真実源・投影の書き込み失敗 | 保持（次の走査で再評価） | 索引しない |

- 保持した entry の本文は、索引時に検証した署名済み内容のままである（object id は署名済み envelope に
  束縛される）。確定した理由は次の走査で従来どおり評価されるため、一時的な失敗と重なった撤回・送信防止の
  反映の遅れは次の走査までに留まる。query 境界は最新 verdict を join して再確認する（§6.7）。
- scan service 越しに返る参照再確認の失敗は、再確認が一度でも確定した理由で失敗していれば確定した理由、
  そうでなければ一時的な失敗として扱う。
- provider 利用不可などで新たに記録された fail-closed の verdict は「非 allow の verdict」であり、
  従来どおり de-index する（§7.6 のとおり再利用せず次の pass で再試行する）。
- contract: `transient_guard_query_failure_keeps_indexed_post_and_holds_new_post`、
  `state_change_detected_by_any_recheck_deindexes_indexed_post`、
  `transient_index_store_read_failure_keeps_indexed_post`、
  `transient_manifest_query_failure_keeps_indexed_media_post`、
  `missing_manifest_still_deindexes_indexed_media_post`、
  `blob_text_fetch_failure_keeps_an_existing_entry_until_validation_fails`、
  cn-e2e `replica_query_failure_keeps_new_posts_out_of_surfaces_until_recovery`。

### 7.8 復旧時のpeer状態管理と投稿取得scheduler（#1212、2026-09-20）

- remote fetchはendpointの接続世代・接続状態と、peerごとの取得成功/失敗・転送時間・短期backoffを
  更新する。接続成功だけをblob取得成功と扱わず、古い接続世代の通知で新しい状態を上書きしない。
- 次のblob取得は、現在接続中か、直近に検証済み取得へ成功したpeerを優先する。失敗peerも期限後の
  回復probeとして候補に残す。取得全体には30秒のattempt budgetを設け、全peer・candidateのtimeoutの
  合計まで1投稿を待たせない。budget超過は一時的な取得不能であり、allowへ変換しない。
- peerへの取得要求頻度はendpoint identity単位のledgerで制限する。HTTPのIP主体、P2P endpoint、
  relay clientは型で区別する。HTTPはtower-governor、relay ingress byteはupstream limiterを唯一の
  enforcement adapterとして維持し、同一要求を二重計上しない。
- cn-indexerは `scope kind + scope id + object id + source revision` のjobをschedulerで管理し、
  queued / fetching / processing / retry_wait / completed / suppressed / cancelledを観測する。
  一つのscope内では設定値（既定4）を上限として投稿を並列処理する。新revisionは旧leaseを失効させ、
  stale completionを反映しない。再起動後はraw bytesを復元せず、authoritative replicaの全件照合から
  jobを再構築する。
- schedulerの診断台帳は実行中を含め1,024件以内とし、完了等の古い記録を回収する。全枠が実行中なら
  新規jobを延期する。実行futureが所有するleaseのDropは同世代の実行中記録だけをCancelledへ移し、
  cancelによる永久占有を防ぐ。部分key/1bucketの欠落をlogical scope全体の削除とは解釈しない（#1293）。
- schedulerは実行順・並列上限・診断記録の保持を所有する。署名、scope、withdrawal、transmission prevention、
  source revision、moderation verdictのguardと、真実源→投影のmutation順は既存pipelineが所有する。
  一時的な失敗では既存entryを保持し、確定した理由と非allowだけが既存規則でde-indexする。
- `/v1/status`は投稿schedulerの状態別件数と最古pending時刻を出す。本文、hash、peer ID、addressは
  statusへ含めない。全件巡回の同期時刻は従来どおりreconciliation完了を表し、schedulerの進捗と
  混同しない。
- contract: `successful_peer_is_ranked_before_recently_timed_out_peer`、
  `stale_disconnect_does_not_replace_newer_connection_generation`、
  `request_frequency_keeps_http_peer_and_relay_subjects_separate`、
  `bounded_scheduler_allows_healthy_job_while_slow_job_is_waiting`、
  `stale_completion_cannot_replace_newer_revision`、
  `independent_posts_are_ingested_concurrently_within_the_configured_bound`。

## 8. 公開bucketの検索結果の保存先（#1243 / #1293）

- 公開bucketのentryは、検索・発見・おすすめの`IndexEntryView`へoptionalな`source_replica_id`を付ける。
  legacyとprivateのentryではfieldを省略し、従来の読取り経路を維持する。fieldのない旧応答も受け入れる。
- source・author・created_atはquery gateで照合した確定済み索引から返す。遅延した検索投影のmetadataを保存先の根拠にしない。
- 新locatorのLocalOnly読取りは既存namespaceのexact keyに限り、不存在namespaceの登録や常駐handleを増やさない。
  namespaceが削除されても検証済みの保存projectionは表示でき、既知の取り下げは引き続き本文・添付を隠す。
- locatorは権限やcanonical本文の証明ではない。clientは要求topic・版・scopeを読取り前に照合し、
  指定先の署名済みenvelopeとbucketの時刻を検証する。未知版・不一致を旧replicaへの自動fallbackにしない。
- clientの先行readerはLocalOnlyで解決する。保存済みprojectionがあればそのcanonical sourceを使い、
  投稿と、返信先/repost元の高々2参照の取り下げをLocalOnlyで再確認する。view生成も背景remoteを起動しない。
  本文blobの欠落はその状態を表示へ渡し、index textをcanonical本文の代わりにしない。
- 不正・取得不能な結果1件で、他の正常な結果を隠さない。全bucketの探索や未許可private取得を追加しない。
- これは[ADR0054](0054-time-bucketed-docs-replicas.md)の読取り準備であり、writer/private epoch/同期owner/GCの
  切替完了を意味しない。既存の認証・同意・安全性・readiness gateを通った結果にだけmetadataを付ける。
