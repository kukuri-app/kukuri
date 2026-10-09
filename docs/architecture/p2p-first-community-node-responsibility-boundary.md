# P2P-first community node の責任境界

最終更新日: 2026-08-25

## この文書の位置づけ

この文書は、kukuri の community node が負う責任と負わない責任の境界を固定するための恒久的なマニフェストである。

community node operator の法務・運用・safety・通報に関わる設計（operator docs / safety architecture / report routing / default node policy など）は、すべてこの責任境界を共通前提とする。

ここで境界を固定しないと、これらの設計が暗黙のうちに「kukuri network 全体の運営者」を仮定してしまう。kukuri はそのような中央運営者を **持たない** のではなく、P2P を基盤としているために **構造的に持ち得ない**。network 全体を所有する中央のチョークポイントが存在しないため、誰かを network 全体の運営者・統治者として据えることはそもそも不可能である。

このページは法的助言ではなく、central governance model / network-wide moderation policy を導入するものでもない（そもそも P2P 基盤上で導入し得ない）。あくまで実装・文書・UX が共有すべき責任境界の定義である。

## 1. kukuri は Mastodon clone ではない

- community node は Mastodon 的な home server **ではない**。
- user identity / profile / social graph は **node-independent** である。これらは特定の community node に属さず、user の鍵に紐づき、P2P network 上で流通・同期される。
- community node は、P2P network に補助能力を提供する **service provider** である。user の所属先（home）でも、user の存在の前提でもない。

kukuri の基本通信優先度は `Direct P2P -> Relay Supported P2P -> Relay Fallback` であり、community node はこの経路を補助するための層に位置する。詳細は `AGENTS.md` の「通信経路」および `docs/adr/0010-kukuri-protocol-v1-boundary-definition.md` を参照する。

### node-independent であることの含意

- user を識別・認証する canonical source は user 自身の鍵であり、いずれかの community node ではない。
- ある community node が停止・消失しても、user identity / profile / social graph は失われない。別の node や直接 P2P 経路から引き続き参照・再構築できる。
- したがって、community node を「アカウントの所在地」「退会先」「凍結権者」として設計してはならない。

## 2. Community node の責務（capability scope 内）

community node は、operator の設定に応じて以下の capability を**提供し得る**。ただし、いずれも node が明示した capability と authority scope の範囲に限定される。

- auth / consent
- bootstrap assist
- relay assist
- topic rendezvous
- community index
- moderation
- trust signal publication
- media cache
- report endpoint
- optional gateway / bridge

これらは「有効化したものだけ」「自分が関与した対象についてだけ」責任を持つ、という形で限定される。node manifest が宣言した capability / authority scope が、その node の責任範囲の上限である。

- 例: `community index` を有効化した node は、自分が index した content についてのみ index 責任を負う。他 node が index した content には責任を負わない。
- 例: `moderation` を有効化した node の moderation event / trust signal は、その node の authority scope 内でのみ意味を持つ。

### 投稿者による撤回と node-local な送信防止

投稿者は、自分の署名鍵で対象投稿を指定した撤回 envelope を発行できる。client は検証済みの最新世代を canonical state とし、timeline、bookmark、通知、返信・引用 preview、添付を placeholder へ置き換える。これは署名済みの撤回意思を後から同期する仕組みであり、既に他 peer や別 node が保持した暗号化済み object / blob を network 全体から消去する機能ではない。

community node operator は、権利侵害等への対応として、自 node の capability ごとに送信防止を決定できる。決定は Postgres の node-local ledger と immutable operator audit に記録し、対象が `community_index` / `search` / `discovery` / `recommendation` に含まれる場合は Postgres の index truth と ArcadeDB projection から除外する（ArcadeDB からは indexer の次の巡回で消す）。`moderation` と `blob_cache` も個別 scope として指定できるが、他 node、Direct P2P、既に他者が保持する copy には効力を持たない。

投稿者撤回、法的送信防止、safety verdict、operator policy は独立した gate であり、最終表示・送信は**全 gate が許可した場合だけ**許可する。競合時の説明理由は `投稿者撤回 > 法的送信防止 > critical safety > operator policy` の順で決定論的に選ぶ。解除しても過去の index を自動復活させず、fresh ingest とその時点の全 gate の再評価を要求する。

## 3. Community node の非責務

community node は、少なくとも以下を**負わない**。これらは operator docs / safety / report flow のいずれにおいても node に帰属させてはならない。

- kukuri network 全体の運営
- user identity の所有
- profile の canonical store
- social graph の canonical store
- 全 content の truth source
- third-party node の活動に対する責任
- global moderation authority
- central abuse intake（中央通報窓口）

つまり、ある node は「自分が index / moderate / cache / relay / recommend した対象」についてのみ責任を負い、network 全体や他 node の振る舞いについては責任を負わない。通報・moderation・trust signal はいずれも node 単位の authority scope に閉じる。

## 4. Default community node の位置づけ

kukuri は onboarding を成立させるために default community node を用意し得るが、その位置づけは限定的である。

- default community node は **onboarding compromise / onboarding infrastructure** である。新規 user が最初の接続・bootstrap・初期体験を得るための補助に過ぎない。
- default node は **network-wide authority ではない**。「default である」ことは「kukuri network 全体の運営者・通報先・moderation 権者である」ことを意味しない。そもそも P2P 基盤上には network 全体の authority という座が存在しないため、default node であってもその役割を担うことは構造的に不可能である。
- default node policy は、**default node が提供する capability にのみ**適用される。
- default node は、third-party node の content / index / moderation / cache / trust signal に対して責任を持たない。
- 通報先は「default node があるから default に集約する」のではなく、対象を実際に表示・索引・moderation・cache・recommend した node の authority scope に基づいて決まる。provenance 不明時に default node / kukuri project へ fallback してはならない。

## 5. Operator safety work の位置づけ

operator docs / safety / report flow の整備は、community node を中央 SNS 運営者にするためのものではない。P2P 基盤上には network 全体を統治する中央運営者が構造的に存在し得ないため、これらは node-local な補助層の説明責任を扱う。

これらの目的は、**P2P network の補助層を、個人・小規模グループでも現実的かつ説明可能に運用できるようにする**ことである。

- operator docs: operator が有効化した capability に対応した説明責任（terms / privacy / 外部送信表示 / 届出補助など）を決定論的に生成し、分散運用の説明責任負荷を下げる。これは node を network 全体の運営者にするものではない。P2P 基盤上ではその役割自体が成立し得ない。
- safety moderation: operator が自分の index / discovery / recommendation / relation から critical safety risk を排除できるようにする。verdict / moderation event / risk signal は node の authority scope 内で機能する。global moderation authority は「作らない」のではなく、P2P 基盤上では成立し得ないため、そもそも存在しない。
- 分散通報ルーティング: 通報を中央集約せず、対象に実際に関与した責任ある node へ route する。network-wide の中央通報窓口は構造的に存在しないため、default node や kukuri project をその窓口として据えることはできない。

## このマニフェストが満たすべきこと

- P2P-first responsibility boundary を明文化している。
- community node が home server ではないことを明記している。
- user identity / profile / social graph が node-independent であることを明記している。
- community node の責務と非責務を分離している。
- default community node が onboarding infrastructure であり、network-wide authority は P2P 基盤上で成立し得ないことを明記している。
- operator docs / safety / report flow が、P2P 補助層を分散運用可能にするためのものであることを明記している。

## 関連文書

- `AGENTS.md`（通信経路の優先度と community node の役割）
- `docs/adr/0009-community-node-relay-auth-data-classification.md`（community-node connectivity/auth のデータ分類）
- `docs/adr/0010-kukuri-protocol-v1-boundary-definition.md`（kukuri Protocol v1 の境界）
- `docs/adr/0032-author-withdrawal-transmission-prevention.md`（署名付き投稿撤回、node-local 送信防止、合成規則）
- `docs/runbooks/community-node-self-host-vps.md`（community node self-host 運用）
