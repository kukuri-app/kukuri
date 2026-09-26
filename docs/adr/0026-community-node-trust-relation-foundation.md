# ADR 0026: Community Node Trust / Relation Foundation

## Status
Accepted（foundation は #409 / PR #414 で確定。§6 未決事項は #416 で Decision 化）

**2026-09-15 改訂**（#1051）: general moderation のうち **nsfw と `objectionable`（ADR 0028 §8.2）の risk signal は
trust の相対成分へ寄与しない（0）**。signal は生成・永続化し、利用者向け read の basis に寄与 0 で残す。
§2.7 の振り分けは「critical = 絶対 / spam・malware・phishing・node-local 観測 = 相対 / nsfw・objectionable =
advisory-only（0）」の 3 分岐になる。詳細は「§7 改訂追補」を正本とし、§2.3 / §2.7 と必須 scenario の該当行は
失効注記で後継を示す。絶対成分、cross-node 開示（§6.3）、appeal 反映（§6.2）は変更しない。

**2026-09-18 改訂**（#1061）: 利用者向けの信頼値 `trust` を、閲覧者に依存しない trust 絶対値 T と、閲覧者別の
relation 値 R（関係の深いユーザー群のブロック / ミュート観測から求める）の CN 側合算値にする。T の算出と cross-node
開示は変更しない。詳細は「§8 改訂追補」を正本とする。

## Date
2026-06-30

## Base Branch
`main`

## Related
- `docs/adr/0027-deterministic-moderation-critical-safety.md`（advisory ≠ command / authority scope / visibility, §2.1 / §2.6。旧 `moderation-event-trust-semantics.md` を集約）
- `docs/adr/0013-social-graph-foundation-draft.md`（author-owned follow graph。canonical, node-independent）
- `docs/adr/0025-community-node-indexing-foundation.md`（index = co-participation の観測元 / recommendation 境界）
- `docs/adr/0024-community-node-admission-data-classification.md`（admission。node-local な補助提供可否）
- `docs/adr/0027-deterministic-moderation-critical-safety.md`（§2.5 / §2.6 signed event / risk signal model）
- `crates/cn-safety/src/signal.rs`（`SafetyRiskSignal` / `RiskSignalTarget`）
- `crates/cn-operator/src/capability.rs`（`CommunityLocalTrust` = `Availability::Planned`）
- Issue: #406（runtime 結線で risk signal を trust/relation reads に反映）, #409（本 ADR foundation）, #416（§6 未決事項の Decision 化）, #665（client 統合と distance opt-out semantics）

## 位置づけ

community node の主要未検討機能 **trust / relation** の **意図・境界・read 契約**を固定する foundation ADR。`CommunityLocalTrust` capability（`Availability::Planned`）の中身を定義し、#406 の「risk signal を trust/relation reads に反映する」反映先 read 契約を確定する。

trust と relation は **node-local かつ advisory** な derived signal であり、`docs/adr/0027-deterministic-moderation-critical-safety.md` §2.1 の不変条件（network-wide command ではない / issuer node の authority scope に閉じる / user identity・profile・social graph の canonical を所有・改変しない）に従う。

本 ADR は **意図・境界・read 契約**を定義する。具体的な clustering / scoring アルゴリズムの詳細は §6（未決）と後続 Issue に委ねる。

## Feature Data Classification

### trust
- Feature 名: community-local user trust signal（troll/bot でない確信度）
- Durable / Transient: Durable な node-local derived state（再構築可能）
- Canonical Source: derived。canonical な user trust は存在しない（node-independent）。**絶対成分**の入力は決定論（#410）+ 厳格非決定論（#411）の verdict / risk signal（`cn_safety.risk_signals`）、**相対成分**の入力は非決定論（#411）+ node-local 観測 + relation。policy で合成
- Replicated?: No（node-local advisory。配布する場合は trust-semantics の visibility に従う signal として）
- Rebuildable From: moderation verdict / risk signals + node-local 観測 + relation + policy。再計算可能
- Public Replica / Private Replica / Local Only: node-local（`cn-core` / Postgres）。配布は visibility（local / subscribed_nodes / public）規則
- Gossip Hint 必要有無: No
- Blob 必要有無: No
- SQLite projection 必要有無: No（server は Postgres）
- 必須 contract:
  - `trust_is_not_single_absolute_scalar`
  - `trust_separates_absolute_and_relative_indicators`
  - `trust_absolute_indicators_not_relation_weighted`
  - `trust_relative_indicators_are_relation_weighted`
  - `trust_resists_mass_report_bombing`
  - `trust_absolute_negative_is_weighted_double`
  - `trust_composition_weights_are_operator_tunable`
  - `trust_reflects_risk_signals_split_by_category`
  - `trust_does_not_own_or_mutate_user_identity`
  - `trust_read_is_explainable_with_basis`
  - `trust_is_clamped_to_unit_interval`（最終値は `[-1, 1]`, §6.2）
  - `trust_absolute_component_does_not_decay`（絶対成分は時間減衰しない, §6.2）
  - `trust_relative_component_decays_over_time`（相対成分・node-local 観測は半減期減衰, §6.2）
  - `trust_appeal_pending_holds_contribution`（`pending` は寄与据え置き, §6.2）
  - `trust_appeal_accepted_excludes_contribution`（`accepted` は該当寄与除外, §6.2）
  - `cross_node_pull_discloses_only_confirmed_absolute_component`（cross-node pull は confirmed 絶対成分のみ, §6.3）
  - `viewer_relative_read_requires_authenticated_viewer`（相対成分 read は viewer 署名検証必須, §6.3）
  - 2026-09-15 追加（#1051、§7）:
    - `general_advisory_contributes_zero_to_trust`（nsfw / objectionable の signal は評価に寄与 0。ADR 0028 §8.5 と共有）
    - `trust_read_lists_advisory_only_basis_with_zero_contribution`（利用者向け read の basis に寄与 0 で残る）
- 必須 scenario:
  - CSAM 系 risk がある pubkey は絶対成分が下がり、relation や通報数で揺れない
  - 特定 cluster からの大量通報は相対成分に raw count として効かず、relation で重み付けされる（report-bombing 耐性）
  - hate / アダルトなど相対指標は viewer の cluster によって評価が変わる / risk・観測が無ければ不当に下げない（2026-09-15 superseded: アダルト（nsfw）と hate 等（objectionable）は評価を動かさず basis にのみ残る。§7）
  - 2026-09-15 追加（#1051）: nsfw / objectionable の advisory だけを持つ pubkey は、相対成分・最終 `trust` とも 0 のままで、basis に寄与 0 の行として現れる
  - 絶対成分と相対成分がともに最悪でも最終 `trust` は `-1` を下回らない（clamp）
  - 相対成分の寄与は時間経過で半減し、絶対成分（known-hash 由来）は同期間で減衰しない
  - 他 node が pull すると confirmed 絶対成分のみ根拠つきで返り、相対成分は返らない
  - viewer 署名の無い / 別 identity を騙る viewer 相対 read は拒否される（なりすまし防止）

### relation
- Feature 名: community-local pairwise relation（A↔B のコミュニティクラスタ近接度）
- Durable / Transient: Durable な node-local derived projection（再構築可能）
- Canonical Source: derived。canonical な relation は存在しない。入力は supported topic / 許可 channel の co-participation（index, ADR 0025 / #404）+ social-graph projection（ADR 0013, follow edges）。relation graph backend は ArcadeDB（最小）/ neo4j（scale, Cypher 互換, §6.1）
- Replicated?: No（node-local）
- Rebuildable From: index の co-participation + follow projection。再計算可能
- Public Replica / Private Replica / Local Only: node-local（`cn-core` / Postgres）
- Gossip Hint 必要有無: No
- Blob 必要有無: No
- SQLite projection 必要有無: No（server は Postgres）
- 必須 contract:
  - `relation_is_pairwise_cluster_proximity`
  - `relation_does_not_mutate_social_graph_canonical`
  - `relation_read_is_explainable`
  - `relation_visibility_choice_is_user_controlled_and_reversible`
  - `relation_does_not_auto_suppress_cross_cluster_content`
  - `relation_read_requires_authenticated_viewer`（viewer 相対 read は viewer 署名検証必須, §6.3）
  - `relation_distance_opt_out_bidirectionally_suppresses_distant_users_and_content`（user が有効化した場合、node-local policy の距離境界より遠い相手との間で、双方の user / post surfacing を抑制する, §6.3）
  - `relation_distance_opt_out_is_not_privacy_or_graph_deletion`（relation graph からの離脱・canonical 削除・プライバシー保証として扱わない, §6.3）
  - `relation_defaults_local_and_not_cross_node_pullable`（relation は `Local` 既定, cross-node pull に返さない, §6.3）
  - `relation_private_channel_signal_stays_local_and_scoped`（private channel 由来は channel + CN authority に閉じ Local 固定, §6.3）
- 必須 scenario:
  - 同一 cluster の A,B は近接度が高く、別 cluster は低い（根拠つき）
  - user が distance opt-out を有効化すると、node-local policy の距離境界より遠い相手について、相手自身と投稿を「見ない」かつ自分自身と投稿を相手へ「見せない」。解除すると surfacing が戻り、relation graph と trust は変化しない
  - cross-cluster content は user が選択しない限り自動で抑制されない
  - relation read は cross-node pull に返らない（`Local` 既定）
  - private channel 由来の co-participation は channel メンバー可視の relation に閉じ、public relation に混ざらない

## 1. 背景と意図

### trust（信頼度）
- 定義: **その CN における、あるユーザーが troll / bot でない確信度**。SNS で一般的な bot 排除・荒らし排除の意図。
- node-local な derived signal。#406 の risk signal（`SafetyRiskSignal`、target = `UserPubkey` / `PeerNode`）を主要入力の一つとして反映する。
- **trust は単一の絶対スカラーにしない。** コミュニティ単位の通報爆撃（恣意的な大量通報）が予想される以上、CN にとっての**絶対指標**（CSAM など community / culture に依らない）と、**相対指標**（hate / 暴力 / アダルトなど community / 文化圏で法・判断が変わる）を分けて処理する。絶対指標は決定論 + 厳格非決定論で、相対指標は relation（clustering）を加味した値で扱う（§2.3）。

### relation（関係性 / クラスタ近接度）
- 定義: **CN から見た、ユーザー A とユーザー B のコミュニティクラスタ的な近さ**（A から見た B の近接度）。
- 目的:
  1. **public からコミュニティを構築していくきっかけづくり**（近接クラスタの surfacing）。
  2. **クラスタの内外を俯瞰的に定量化**し、対立 / エコーチェンバーに「ある種の納得感」を与える（descriptive transparency）。
  3. ユーザーに **「そもそも見ない / 見せない」という選択肢**を与え、距離のあるコミュニティ同士の大規模な対立を避ける（user agency）。

trust が「個人の信頼度（troll でないか）」を測るのに対し、relation は「2 者のクラスタ的距離」を測る。両者は別の量である。

## 2. Decision

### 2.1 trust と relation を別概念として定義する
- **trust**: per-user（pubkey）の node-local 信頼度。troll/bot でない確信度。**単一の絶対スカラーにせず、絶対指標 + 相対指標の合成**として扱う（§2.3）。断定ラベルではなく根拠つき（basis / 寄与 signal）advisory。
- **relation**: pairwise（viewer A, target B）の node-local クラスタ近接度。A↔B のコミュニティ的近さ。advisory。対象は 2 者間。
- 両者は別 read として提供し、混同しない。

### 2.2 social-graph v1 との境界 — overlay であって canonical を改変しない
- social-graph v1（ADR 0013）の follow edge / `mutual` / `friend_of_friend` は **author-owned canonical, node-independent**。
- trust / relation は CN が観測から導く **node-local な derived overlay** であり、social graph の canonical を所有・改変・上書きしない（trust-semantics の `does_not_apply_to: user_social_graph_canonical_source` と整合）。
- relation は follow projection を **入力の一つ**として参照してよいが、relation の出力は social graph とは別物（cluster proximity であって follow 関係ではない）。

### 2.3 trust の構成 — 絶対指標と相対指標を分ける（report-bombing 耐性）
- trust は **単一の絶対スカラーにしない**。コミュニティ単位の通報爆撃（恣意的な大量通報）が予想される以上、CN にとっての絶対指標と、コミュニティ / 文化圏で法・判断が変わる相対指標を分けて処理する。
- **絶対指標（absolute）**: CSAM など、community / culture に依らず CN にとって絶対の指標。
  - 入力: **決定論的 moderation（#410）+ 厳格な非決定論的 moderation（#411）**（known-hash / provider-verdict / 厳格 classifier）。
  - **relation で重み付けしない**。evidence / 検知ベースであり、通報数では動かない（report-bombing に対して不動）。
- **相対指標（relative）**: hate / 暴力 / アダルトなど、community / 文化圏で法・判断が変わる指標。
  - 入力: 非決定論的 moderation（#411）+ node-local 観測（spam / abuse 報告、rate など）+ **relation（clustering）を加味**した値。
  - viewer / cluster 相対。relation で重み付けすることで、特定 cluster からの大量通報が raw count として効かず cluster 文脈で相対化される（report-bombing 耐性）。
- bot / 自動化 abuse の検知は挙動ベースの絶対指標寄り、troll / harassment の判断は文化依存の相対指標寄りとして扱う。

> **2026-09-15 改訂**（#1051、§7）: 上記の相対指標の例示のうち **アダルト（nsfw）と hate 等（objectionable）は
> 相対成分の入力から外し、寄与 0 の advisory として basis にのみ残す**。相対成分の入力は spam / malware /
> phishing と node-local 観測（+ relation 重み付け）に限る。
- read: trust は **絶対成分（viewer 非依存）+ 相対成分（viewer / cluster 依存）**の合成として返す（合成式は §6.2、初期決め打ち・operator 可変）。いずれも断定ラベル（「このユーザーは troll」）ではなく根拠つき advisory（basis / 寄与 signal / confidence / visibility / expiry を説明可能、trust-semantics §4）。

### 2.4 relation の入力と read
- 入力（node-local）: supported topic / 許可 channel の **co-participation**（index, ADR 0025 / #404 が観測元）、social-graph projection（follow edges, ADR 0013）、その他 node-local な共起シグナル。
- read: pairwise（A, B）の cluster proximity（近接度 + 根拠）。viewer 視点で相対化する。
- 具体的 clustering / scoring は §6（未決）。本 ADR は入力・出力形・境界のみ固定する。

### 2.5 advisory / authority scope / visibility（trust-semantics 準拠）
- trust / relation は network-wide command ではない。issuer CN の authority scope に閉じた optional trust input。
- 別 CN は別の trust / relation を持ちうる（単一の正解は無い）。
- **read は pull 型**。ある pubkey について read API を叩くと、**その CN の視点**で計算された trust / relation が返る（push 複製ではない。§6.3 で Decision 化）。node A が node B に問い合わせれば返るのは **B の view** であり、A 自身の view は A の CN が返す。
- `visibility`（`local` / `subscribed_nodes` / `public`）は **push 複製ポリシーではなく、cross-node pull に対するアクセス範囲**（誰がこの CN の signal を read できるか）として解釈する。誤検知を拡散しないため既定は `local`（§6.3）。
- client は issuer / basis / confidence / visibility / subscription を説明できる前提で表示する（断定ラベルを根拠なく出さない）。

### 2.6 user agency と echo-chamber への配慮
- relation はユーザーに **「見ない / 見せない」選択肢**を与えるための **descriptive かつ user-controlled** な signal とする。目的は、relation が一定以上離れたコミュニティ間で対立が大規模に増幅することを避けることであり、プライバシー保護ではない。
- **CN は relation を使って cross-cluster content を勝手に抑制しない**（auto-segregation を既定にしない）。distance opt-out は user が明示的に有効化した場合だけ発火し、可逆・説明可能であること。
- distance opt-out を有効化した user A と、node-local policy の距離境界より遠い user B の間では、CN の surfacing 上で **A は B 自身と B の投稿を見ない**、かつ **B には A 自身と A の投稿を見せない**。片方向の block ではなく、A の選択を起点にした node-local な相互非表示として扱う。
- 「見せない」は当該 CN の client-facing relation read / discovery / recommendation / index-search 等の surfacing 境界に限る。user / post / social graph / relation graph の canonical や derived edge を削除せず、内部の pairwise 計算は距離判定のため保持する。P2P・別 CN・network 全体での不可視性や秘匿性を保証しない。`relation/optout` という API 名を、relation graph からの離脱やプライバシー操作と解釈してはならない。
- 距離境界は node-local policy とし、network-wide な固定閾値を ADR に持ち込まない。適用時は利用者に境界と根拠を説明できること。
- echo-chamber の緊張: 「見ない / 見せない」選択は filter bubble を強める恐れがある。緩和として (a) 既定で隠さない、(b) 近接度と根拠を透明に提示し「納得感」を descriptive に与える、(c) 選択は可逆、(d) relation を「対立の固定化」ではなく「俯瞰と選択の材料」として位置づける。

### 2.7 #406 の反映先 read 契約（絶対 / 相対で振り分ける）
- #406 の「risk signal を trust/relation reads に反映」は、risk signal の category に応じて trust の **絶対成分 / 相対成分**へ振り分けて反映する。
  - CSAM など critical safety = **絶対成分**（relation 非依存、report-bomb で不動）。
  - hate / nsfw / spam など = **相対成分**（relation で重み付け、viewer / cluster 相対）。
    （2026-09-15 superseded: spam / malware / phishing のみ相対成分。nsfw / objectionable（hate を含む）は寄与 0 の advisory-only。§7）
- relation は相対成分の重み付け入力として使う。relation 本体は cluster proximity であり risk ラベルではない。
- 反映は advisory（根拠つき）。断定 / canonical 改変はしない。

## 3. Consequences
- `CommunityLocalTrust` capability の中身が trust（per-user 信頼度）+ relation（pairwise cluster proximity）の 2 read として定義される。capability 昇格は実装・テストが揃った段階で別途判断（本 ADR では `Availability::Planned` 維持）。
- #406 は反映先 read（trust）が定義されたことでブロッカー解消。runtime 結線は risk signal を category に応じて trust の絶対 / 相対成分へ入力する形になる。
- trust が絶対 / 相対の 2 系統に分かれるため、絶対系は決定論（#410）+ 厳格非決定論（#411）moderation に、相対系は #411 + relation に依存する。本 ADR は #410 / #411 と密接に結びつく。
- relation は index の co-participation を入力にするため、indexing（#404 / ADR 0025）に依存する。graph backend は最小 = ArcadeDB、scale = neo4j（Cypher 互換、§6.1）。
- #416 で §6 未決事項を Decision 化: 最終クランプ `[-1,1]`（§6.2）、閾値なし / 相対成分 decay / appeal 反映（§6.2）、pull 型 read と cross-node 開示は confirmed 絶対成分のみ + viewer 相対 read の署名検証（§6.3）、opt-out 範囲（§6.3）、private channel は Local 固定（§6.3）、graph-store 抽象境界 API と移行閾値（§6.1）。foundation 実装（#415）はこれらの contract / scenario を満たす。

## 4. Out of scope（後続 / 別 ADR・Issue）
- 具体的 clustering / community detection アルゴリズムの評価・チューニング（入力特徴の重み実測、viewer 相対化の詳細、pairwise の計算 / 保存コスト最適化）。合成式・最終クランプ・decay・appeal・visibility の方針は §6 で決定済み。
- recommendation での relation 利用詳細（ADR 0025 の recommendation 境界に従う）。
- 決定論 / 非決定論 moderation の verdict 生成（#410 / #411）。
- client UI（trust / relation の表示・distance opt-out による「見ない / 見せない」導線）。
- foundation 実装（#415）。本 ADR は決定・contract を固定し、実装は #415 が担う。

## 5. 維持する境界
- trust / relation は node-local advisory。network-wide command でも canonical でもない。
- user identity / profile / social graph の canonical を所有・改変しない（overlay のみ）。
- 断定ラベルを根拠なく出さない（基準は trust-semantics §4）。
- relation で cross-cluster content を勝手に抑制しない。distance opt-out を user が明示的に有効化した場合だけ、node-local policy の距離境界に基づく「見ない / 見せない」を相互・可逆・透明に適用する。
- read は pull 型。cross-node pull への開示は visibility 規則に従い、confirmed 絶対成分のみに限る（相対成分・relation は Local）。誤検知を public へ拡散しない。
- viewer 相対 read は viewer identity の署名検証を必須とし、なりすましによる他者視点の取得を許さない。

## 6. Decision（§6 未決事項の Decision 化, #416）

foundation（#409 / PR #414）が残した §6 の未決事項を #416 で決定した。以下を Decision とする。なお具体的 clustering / community detection アルゴリズムの評価・チューニング（入力特徴の重み実測、pairwise 計算 / 保存コストの最適化）は後続 Issue に残る（§4 Out of scope）。

### 6.1 relation clustering の backend と graph-store 抽象境界（Decision）
- **最小構成 = ArcadeDB**（軽量・低コスト・埋め込み可能な multi-model / graph）。項目ごとの解析 worker が複数項目を点数化（例: 共有 topic 数、friend-of-friend、co-participation 頻度、follow projection）して relation graph に格納し、pairwise proximity を graph query で算出する。relation の観測元（co-participation）は index（Postgres, ADR 0025）。
- **scale path = neo4j**（大規模）。ArcadeDB と **Cypher 互換**で移行しやすい。
- **graph-store 抽象境界（Decision）**: backend を差し替え可能にする trait を置く。最小 API 形は次を必須とする（いずれも viewer / target は pubkey、proximity は根拠つき）:
  - `upsert_edge(from, to, features)` — co-participation / follow projection 等の特徴を格納。
  - `pairwise_proximity(viewer, target) -> Proximity{ score, basis }` — A 視点の B への近接度（viewer 相対、根拠つき）。
  - `neighbors(viewer, k) -> [pubkey]` — discovery / surfacing 用の近接近傍。
  - `cluster_of(pubkey) -> ClusterRef` — cluster 帰属（相対成分の重み付け入力）。
  - クエリは **Cypher 互換**表現に落とせることを前提とし、ArcadeDB / neo4j 双方で同一 API を満たす。
- **移行閾値（Decision）**: 判断軸は **規模 + profile**。
  - invite-only（ADR 0024 admission）/ 小規模 node は **ArcadeDB 据え置き**。
  - public profile かつ大規模 supported topic を持つ node は edge / node 数の増大で **neo4j を検討**。
  - 具体しきい値は運用データが無いため **初期は目安に留め、`cn-operator` の readiness で再評価**する（決め打ち固定値は置かない）。

### 6.2 trust scoring（絶対 / 相対の合成）— 最終クランプ・閾値・decay・appeal（Decision）
- 絶対成分・相対成分はそれぞれ **±1 を上下限**とする。
  - 絶対成分: 決定論（#410）+ 厳格非決定論（#411）の verdict / risk signal から算出。relation 非依存、report-bomb 不動、**viewer 非依存**（§6.3 の pull 開示で viewer context を要さない成分）。
  - 相対成分: 非決定論（#411）+ node-local 観測を relation で重み付けして算出。**viewer / cluster 相対**。
- **合成式（初期決め打ち, operator 可変）**:
  ```
  absolute ∈ [-1, 1], relative ∈ [-1, 1]
  w_abs = 2.0 if absolute < 0 else 1.0
  trust = clamp(-1, 1, (w_abs * absolute + relative) / 2)
  ```
  絶対成分がマイナス（CSAM など確定的に排除すべき）のとき重み 2 倍で distrust を支配的にし、相対指標（文化依存）が良好でも薄まらないようにする。プラスのときは 1 倍。
- **最終クランプ（Decision）**: `trust` は最終的に **`[-1, 1]` にクランプ**する。絶対成分マイナス時に合成値が -1 を下回り得る（例: absolute=-1, relative=-1 → -1.5）が、read 値域を対称に保ち client 表示・閾値設計を単純化するため clamp する。distrust の支配性は「絶対マイナス時は relative が良好でも救済されない」性質で clamp 前に既に担保される。
- **重みは operator が変更できる**（`w_abs` の係数等）。
- **閾値（Decision）**: read は**連続値の advisory** を維持し、「このユーザーは troll」等の**断定閾値は置かない**（trust-semantics §4）。client 表示のバケット化（例 low / mid / high）が要るなら **operator 可変パラメータ**とし、ADR では固定しない。
- **decay（Decision）**: **相対成分・node-local 観測**には **半減期方式の時間減衰**を導入する（半減期は operator 可変）。**絶対成分は evidence / 検知ベースのため減衰させない**（known-hash / provider-verdict は時間で薄めない）。
- **appeal（Decision）**: `AppealStatus` を trust 寄与へ次のとおり反映する。
  - `pending`: 該当 signal の寄与を**据え置き**（申し立て中に勝手に緩めない）。
  - `accepted`: 該当 signal の寄与を**除外**して再計算する。
  - `rejected`: 確定寄与のまま維持する。
- ノード内の利用者向け取得は、`accepted` に対応する `Cleared` の判定を実効寄与 0 の
  説明用 `basis` として残す。これにより利用者は審査結果を再取得できる。評価値と別ノード向け取得
  には寄与させず、失効済み判定は従来どおり返さない。
- **訂正版再発行の終結（#710・案A）**: 審査経路の再発行は旧判定を失効させず、同一取引で
  `Cleared` にして終結させる（失効させると本節の失効除外により根拠一覧から消え、利用者が
  審査の終結を確認できないため）。利用者の再取得には、旧判定（`Cleared`・寄与 0）と訂正版の
  新判定の両方が現れる。認容・棄却の契約は変わらない。

### 6.3 visibility / cross-node 開示（pull 型）・opt-out・private channel（Decision）

- **read は pull 型（Decision）**: ある pubkey について read すると、**その CN の視点**で計算された trust / relation が返る。push 複製はしない。`visibility` は **cross-node pull に対するアクセス範囲**（誰がこの CN の signal を read できるか）を表す。
  - `Local`: この CN 自身のクライアントの read にのみ応答（他 node の pull には返さない）。**既定**。
  - `SubscribedNodes`: この CN を subscribe している node からの pull に応答してよい。
  - `Public`: 誰でも pull 可。
- **cross-node pull の開示範囲（Decision）**: cross-node pull に対しては **confirmed 絶対成分のみ**を根拠つき（issuer / basis / confidence / expiry）で応答する。**相対成分・relation・suspected は `Local` 固定**とし、cross-node pull には返さない（誤検知・文化依存指標を拡散しない）。
- **viewer 相対 read の認証（Decision, なりすまし防止）**: 相対成分 / relation は viewer 相対のため read query に **viewer（誰視点か）** を含める必要がある。ユーザー C がユーザー A を騙って「A から見た B の trust / relation」を取得することを防ぐため、**viewer 相対 read はリクエスタが viewer identity への権限を証明（viewer 鍵による署名を検証）できることを必須**とする。**絶対成分 read は viewer 非依存のため viewer 証明を要さない**が、`visibility` scope には従う。
- **distance opt-out「見ない / 見せない」の範囲（Decision）**: user A が opt-out を有効化した場合、node-local policy の距離境界より遠い user B との間で、当該 CN の surfacing を相互に抑制する。A には B 自身と B の投稿を出さず、B には A 自身と A の投稿を出さない。対象 surface は client-facing relation read / discovery / recommendation / index-search 等の user / post surfacing とする。relation graph と内部の pairwise 計算は距離判定・可逆性・説明可能性のため保持するが、抑制対象 pair の client-facing read は generic な unavailable として扱い、opt-out 状態を相手へ開示しない。既定は無効、設定 / 解除は可逆、適用した距離境界と根拠は設定 user 本人に説明可能でなければならない。
- distance opt-out は **relation graph からの離脱ではなく、プライバシー機能でもない**。relation edge、user / post、social graph canonical を削除せず、P2P・別 CN・network 全体への秘匿性を保証しない。**trust にも影響しない**（troll 判定を回避する手段にしない）。既存の `PUT/DELETE /v1/relation/optout` はこの node-local surfacing 選択の設定 / 解除として解釈する。
- **distance opt-out の安定エラーコード（#712）**: `/v1/relation/optout` は trust read とは独立の門を持ち、未構成は `RELATION_VISIBILITY_NOT_CONFIGURED`、有効化（準備完了記録）の失効は `RELATION_VISIBILITY_NOT_ACTIVATED`（いずれも 404）で縮退を判別できる。client はこのコードで未提供・失効・認証（`AUTH_REQUIRED`）・同意（`CONSENT_REQUIRED`）を見分けて案内し、サーバの英文メッセージを生表示しない。コード名は `cn-protocol` の定数が単一定義で、通信境界の実値はサーバ契約試験（`activation_gate.rs` / `trust_relation.rs`）が `body.code` で固定する。
- **private channel における扱い（Decision）**: private channel（ADR 0024 admission / ADR 0025 §6.3）由来の co-participation は **channel メンバー + その CN の authority に閉じ、`Local` 固定**とする。当該 channel メンバー可視の relation に閉じ、**public relation / trust には混ぜない**。private channel の secret を提示できる権限者の scope を超えて private 由来 signal を露出させない。

## 7. 改訂追補（#1051、2026-09-15）: advisory-only category（nsfw / objectionable）

本節は 2026-09-15 の製品決定（Issue #1051、ADR 0028 §8）のうち trust に関する部分を記録する。§2.3 / §2.7 と
必須 scenario の失効注記は本節を後継とする。

### 7.1 振り分けの 3 分岐
- risk signal の category による振り分けを次の 3 分岐にする。
  - critical safety（`Csam` / `Cse` / `Grooming`）= **絶対成分**（不変）。
  - `Spam` / `Malware` / `Phishing` と node-local 観測 = **相対成分**（relation 重み付け・半減期減衰。不変）。
  - `Nsfw` / `Objectionable`（ADR 0028 §8.2）= **advisory-only**。評価計算に入れず寄与 0。
- 文化圏で判断が変わる指標を viewer 非依存の減点にしないため、relation 重み（cluster 近接度）が実装された後も
  nsfw / objectionable は重み付けの対象にしない。

### 7.2 利用者向け read での扱い
- `GET /v1/trust/users/{pubkey}` の basis には、nsfw / objectionable の signal を **`raw_contribution = 0` /
  `contribution = 0`** の行として残す（§6.2 の `Cleared` と同じ「実効寄与 0 の説明用 basis」）。利用者は判定・
  confidence・appeal 状態を確認し、ADR 0028 §2.8 の申し立てへ進める。
- `TrustBasisEntry` の既存欄（`component` は `Relative` のまま）で表現し、wire に破壊的変更を加えない。
- 評価値（`relative` / `trust`）には入れない。別ノード向け pull（§6.3、confirmed 絶対成分のみ）にも入れない。

### 7.3 appeal / 失効との関係
- `Disputed` / `Cleared` / `expires_at` の扱い（§6.2）は不変。nsfw / objectionable は元から寄与 0 なので、
  `Cleared` になっても評価値は動かず、basis の状態表示だけが変わる。
- #1050 が所有する重複 signal の圧縮後も、寄与 0 の契約は signal 件数に依存しない。
- 再 scan が現在の判定から外した scanner 由来の nsfw / objectionable signal は失効する（#1109、ADR 0028 §8.14）。
  評価値は変わらず、失効行は basis から外れる。

### 7.4 contract / scenario
- 追加: `general_advisory_contributes_zero_to_trust`（ADR 0028 §8.11 と共有）、
  `trust_read_lists_advisory_only_basis_with_zero_contribution`。
- 維持: `trust_separates_absolute_and_relative_indicators`、`trust_relative_indicators_are_relation_weighted`
  （対象は spam / malware / phishing と node-local 観測）、`trust_reflects_risk_signals_split_by_category`
  （3 分岐）、`cross_node_pull_discloses_only_confirmed_absolute_component`。
- superseded: 必須 scenario「hate / アダルトなど相対指標は viewer の cluster によって評価が変わる」。

### 7.5 変更しないもの
- 絶対成分の入力・非減衰・`w_abs_negative = 2.0` の合成式、最終クランプ `[-1, 1]`。
- cross-node 開示範囲、viewer 相対 read の署名検証、distance opt-out、private channel の `Local` 固定。

## 8. 改訂追補（#1061、2026-09-18）: trust 絶対値 T と閲覧者別 relation 値 R の別管理・合算、ブロック/ミュート観測

本節は Issue #1061（Scope revision 2026-09-15-r2）の決定を記録する。§2.3 / §6.2 / §7 の既存成分は変更せず、
その上に閲覧者別の relation 値と、CN 側で合算した利用者向けの信頼値を定義する。

### 8.1 三つの値と責務
| 値 | CN 内の単位 | 内容 | 更新元 |
| --- | --- | --- | --- |
| T_N(B)（trust 絶対値） | CN × 対象 B | §6.2 の合成式 `compose_trust(absolute, relative)` の値。`relative` は spam / malware / phishing と node-local risk signal を**一様重み 1.0** で集計する。閲覧者に依存しない | `cn_safety.risk_signals`（appeal・operator 調整・失効を含む） |
| R_N(A,B)（relation 値） | CN × 閲覧者 A × 対象 B | 閲覧者 A と関係の深いユーザー群による B へのブロック / ミュート観測から求める調整値。値域は `[-1, 0]` | `cn_trust.observations` と relation graph の proximity |
| S_N(A,B)（返却する信頼値） | CN × A × B × 評価版 | `S = clamp(-1, 1, T + R)` | read 時に CN が合算する |

- 既存 wire の `absolute` / `relative` は **T の内訳**（説明用）であり、R ではない。ここでの `relative` は §2.3 の
  相対指標（risk signal 由来）を指し、#1061 の relation 値とは別の量である。
- wire の `trust` は S を返す。クライアントの表示判断に使う評価値は `trust` と、§8.4 の `hide_recommended` だけであり、
  クライアントは `absolute` / `relative` から評価値を再合成しない。別 CN の値とも混ぜない。
- T の算出は #1061 以前の production の `trust` と同一である。既存評価を落とさず、relation 寄与を二重に計上しない。
- R は T を変更しない。T の変化は R を上書きしない。S は T・R・重みの入力へ戻さない。critical safety の強制制限
  （ADR 0027）は S とは別に維持し、正の relation で相殺しない。

### 8.2 R の算出（初期決め打ち、operator 可変）
```
R_base = 0                                  # 近接度そのものを信頼値にしない
w(A,U) = 1                    if U == A
       = proximity(A,U)       otherwise     # relation snapshot の値。edge が無ければ 0
w(A,U) = 0                    if w(A,U) < min_weight            # 既定 0.1
s(U,B) = max(block ? 1.0 : 0, mute ? 0.5 : 0)                    # 同一 U→B の両操作は強い方
d(U,B) = 0.5^(age_days / relative_half_life_days)               # 観測時刻からの半減期減衰（既定 30 日）
c(U)   = w(A,U) × s(U,B) × d(U,B)
penalty = 1 - Π_{上位 K 件の c(U)} (1 - c(U))                    # noisy-OR、K の既定 5
R = clamp(-1, 0, R_base - penalty_scale × penalty)                # penalty_scale の既定 1.0
```
- 重みは非負で、Aと関係の遠い（proximity の低い）ユーザーの観測は小さくしか効かない。負の relation を負の重みにして
  B を加点しない。観測が 1 件なら penalty は `w × s × d` で、w に対して単調に増える（重み付き平均のように w が相殺されない）。
- 上位 K 件の noisy-OR にするため、低重みの観測が件数だけで結果を支配しない。
- 重みは relation graph の proximity（co-participation 由来）だけから求め、R / S を入力に戻さない。したがって
  評価は循環せず、同じ入力（観測集合・proximity・評価時刻）は入力の順序によらず同じ R を得る。
- relation snapshot は直近の `relation analyze` の実行結果とする。解析中の read で更新前後の proximity が混在しうることは
  許容し、`relation_version` には直近で成功した実行の id を載せる。
- 観測が無い、または active な観測がすべて除外された場合 R = 0 とする。本文 hash / blob hash の重複は観測を生まず、
  R を下げない。
- nsfw / objectionable（§7）は R の入力にしない。距離 opt-out（§6.3）は R にも T にも影響しない。

### 8.3 ブロック / ミュート観測の契約
- 観測は **observer 本人が署名した envelope** だけを受け付ける。
  - block: 既存の `block-edge` envelope（ADR 0043。author replica に書かれる署名 edge）をそのまま送る。
  - mute: `mute-observation` envelope（kukuri-core。subject = observer、target、status `active` / `revoked`、created_at）。
    docs sync・gossip・author replica には書かない（ADR 0022 の local mute canonical は変えない）。
- 受付 `POST /v1/trust/observations` は次をすべて満たす場合だけ保存する。満たさない場合は保存せず拒否する。
  - bearer identity の pubkey と envelope の署名者（subject）が一致する。
  - 必須同意が成立し、かつ任意文書 `trust_observation_sharing`（§8.5）の現行版に同意済みで、その同意後に取消していない。
- 保存は `cn_trust.observations` に `(observer, target, kind)` 単位で upsert する。`(created_at, envelope_id)` が保存済みより
  新しい場合だけ置き換え、再送・複数端末・順序逆転で増幅せず、古い active で復活しない。
- 取消 `DELETE /v1/trust/observations` は、その observer の観測を全削除し、任意同意の取消時刻を記録する。取消後の受付は
  再同意まで拒否する。任意同意の版が変わった observer の観測は、再同意まで評価に使わない。
- 保持: revoked は 30 日、active は観測時刻から 180 日で評価対象から外し、定期 cleanup で削除する。
  revoked の行を削除した後は、それより古い active envelope の再送を古いと判定できない。client の送信待ちは
  対象・種別ごとに最新 1 件へ集約して送るため、通常の再送では起きない。
- 観測時刻が受信時刻より 5 分以上未来の envelope は拒否する（後の解除を古いと誤判定させないため）。
- 評価では 1 対象あたり新しい順に 200 件までの active 観測を使う。
- 匿名通報（`cn_admin.reports`）から observer 付き観測を作らない。観測を「phishing 検出済み」等の risk category に変換しない。
- 観測・R・observer は CN 間 pull（§6.3）に返さない。利用者向け read にも observer の一覧・件数を返さず、
  R の寄与は理由の種類（§8.4 `reasons`）としてだけ示す。

### 8.4 評価の版・期限と表示 policy
- read 応答は `evaluation` を同伴する（旧 node の応答では欠落し、クライアントは未評価として扱う）。
  - `policy_version`: 合算・表示 policy の parameter から決まる識別子。
  - `trust_version`: T に寄与する入力（signal id・appeal 状態・operator 調整・失効）の digest。
  - `relation_version`: 直近で成功した relation 解析の id と、B に対する観測の最新 revision の組。
  - `computed_at` / `expires_at`: 評価時刻と、クライアントが結果を再利用してよい期限（既定 600 秒）。
  - `hide_recommended`: `S <= hide_threshold`（既定 -0.5）。
  - `reasons`: `risk_signals`（T が負）/ `related_users_block_or_mute`（R が負）。数値・observer は含めない。
- `hide_threshold` は **node-local な表示 policy の parameter** であり、§6.2「断定閾値を置かない」を変更しない。
  CN は利用者を troll と断定するラベルを返さず、クライアントは折りたたみと再表示・例外設定を提供する。
- 一括 read `POST /v1/trust/evaluations`（最大 100 件、viewer 認証必須）は target ごとに `trust` と `evaluation` を返す。
- parameter の env: `COMMUNITY_NODE_TRUST_RELATION_MIN_WEIGHT`、`_RELATION_TOP_K`、`_RELATION_PENALTY_SCALE`、
  `_BLOCK_STRENGTH`、`_MUTE_STRENGTH`、`_HIDE_THRESHOLD`、`_EVALUATION_TTL_SECONDS`（接頭辞 `COMMUNITY_NODE_TRUST`）。

### 8.5 観測提供の同意
- 観測提供は CN の同意カタログの**任意文書**（kind `trust_observation_sharing`、slug `trust_observation_sharing`、
  `required: false`）への同意で有効になる。文書を公開していない CN には観測を送らない。
- 文書には送信項目（observer pubkey・target pubkey・block / mute の種別と状態・時刻・署名）、宛先（その CN だけ）、
  利用目的（閲覧者別 relation 値の調整）、保持期間と取消方法を記載する。
- 既存のローカル mute / block は、利用者が有効化時に明示的に選んだ場合だけ送る。CN の採用順位の変更だけでは送らない。

### 8.6 contract / scenario
- 追加 contract（右は固定する test）:
  - `trust_value_is_absolute_plus_viewer_relation`（S = clamp(T + R)）:
    `cn-trust/tests/relation_adjustment.rs` の `trust_component_is_unchanged_by_observations`、
    `cn-user-api/tests/trust_observations.rs` の `trust_read_sums_absolute_and_viewer_relation`
  - `relation_observation_penalty_scales_with_viewer_relation`: `viewer_with_higher_relation_gets_larger_penalty`、
    `single_observation_penalty_is_not_cancelled_by_weight`、`many_low_weight_observers_do_not_dominate`
  - `relation_observation_does_not_change_trust_absolute`: `relation_update_does_not_touch_trust_component`
  - `relation_observation_is_idempotent_and_order_independent`: `adjustment_is_order_independent`、
    `block_and_mute_same_observer_takes_max`、`observation_upsert_is_idempotent_and_ignores_stale`
  - `relation_observation_requires_signed_observer_and_sharing_consent`:
    `observation_intake_requires_matching_signer_and_sharing_consent`、`observation_revocation_deletes_rows_and_blocks_intake`
  - `relation_observation_expires_and_is_purged`: `revoked_and_expired_observations_are_excluded`、`observation_retention_purges_expired`
  - `relation_observation_is_not_cross_node_pullable` / `trust_read_does_not_disclose_observers`:
    `trust_read_sums_absolute_and_viewer_relation`（pull・単体 read・一括評価の本文に observer が現れない）
  - `trust_hide_recommendation_is_node_local_policy`: `hide_recommendation_follows_node_local_threshold_and_reasons`
- 追加 scenario（`trust_read_sums_absolute_and_viewer_relation` で固定）: A→U の relation が高く C→U が低いとき、
  U が B をブロックすると A から見た B の S が C より大きく下がり、T は共通で、U が解除すると元に戻る。
  提供同意を取り消した U の観測は評価に使わない。

## 9. 改訂追補（#1221 R5-E、2026-09-26）: relation の観測を 2 者間のアクションの差分へ移す

2026-09-26 のユーザー決定により、§2.4・§6.1 の relation の観測元（index の co-participation の全件集計）を置き換える。
旧案（supported topic ごとの共起、全体の上位 limit 件だけを upsert、dominant topic は author の辞書順で先頭 limit 人）は
失効し、件数に依存しない差分処理へ移す。

- 観測: 取込みが public topic の投稿・リアクションから 2 者間のアクション（返信・repost・引用・リアクション・フォロー）を
  `cn_index.relation_actions` に 1 行ずつ保存する。返信先・リアクション先の著者は index 真実源から点読で求め、索引に無ければ
  数えない。自分自身へのアクションと private channel 由来は保存しない。起点の投稿の索引行が消えると（撤回・送信防止・
  scope 解除）trigger が行を消す。取り消したリアクションは変更通知で読み直して消す。
- フォロー: 新しいアクション actor→target を保存したときだけ、target の author replica の `graph/follows/<actor>` を
  remote の reader から 1 key（doc と署名つき envelope）読み、有効なら target→actor の行を置き、取り消しなら消す。
  相互フォローだけのペアは作らない。§2.4 の follow projection（ADR 0013）はこの形で relation に入る。
- ペア: 双方向（方向ごとに種類は異なってよい）にアクションがあり、フォロー以外のアクション（public topic のアクション）が
  1 件以上あるときだけ edge を作る（相互フォローだけのペアは作らない）。値は key を維持して意味を置換し、
  `shared_topics`＝その 2 者間のアクションがあった public topic の数、`co_participation_events`＝アクションの件数とする。
  proximity の計算式・重み・API は変えない。成立しなくなったペアの edge は消す（`RelationStore::remove_edge`）。
- cluster: author の dominant topic（最多の索引件数の public topic、同数は scope_id の辞書順）。参加が 0 になった author の
  帰属は外す（`RelationStore::clear_cluster`）。
- 解析: 行の追加・削除を trigger がペアの方向別件数・共有 topic 数と author の参加数へ差分反映し、印（`dirty_seq`）を付ける。
  `cn-cli relation analyze` は印の付いたペアと author を古い順に最大 limit 件ずつ反映し、反映した印だけを外す。
  途中で止まっても残った印から続き、変化の無いペアは読まない。実行記録は直近 100 件と最新の成功だけを残す。
- 移行: 既存 index からの参加数だけを移行時に 1 回作り、全 author の cluster を印の対象にする。過去の投稿のアクションは
  遡って作らない（取込みの窓から先の観測で増える）。旧定義の graph（`TrustUser` / `RelatesTo`）は意味が違うため読まず、
  `ensure_schema` が型ごと削除し、新しい型（`RelationUser` / `InteractsWith`）へ作り直す。
