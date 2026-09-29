# ADR 0046: 18歳以上の自己申告と成人向け表現の既定非表示

## Status
Accepted

**2026-09-15 改訂**（#1051）: ラベル源に、投稿者 self-label に加えて **設定済み / 購読 Community Node の
content advisory（ADR 0028 §8.6）を第 2 の源**として合成する。advisory 付きメディアにも §4 の取得ゲートと
§5 の代替表示を適用する。詳細は「§6 改訂追補」を正本とし、Context の「CN 判定は配信されない」と §3 の
「CN advisory はスコープ外」は失効注記で後継を示す。自己申告の必須化、表示設定の既定 OFF、ラベル無し
コンテンツの fail-open、`content_labels` の署名保護は変更しない。

**2026-09-25 改訂**（#1221 R5-A）: remoteのラベル根拠は投稿projectionの参照または期限付き表示cacheへ結び、
無期限のhash台帳にしない。最後の根拠を回収したhashの表示済みURLは破棄し、元投稿を再検証できない
添付は再検証まで非表示にする（ユーザー判断A）。本人の保護投稿に結び付く根拠と旧保存分は通常remote
cacheの回収から除外し、旧保存分の整理はR5-Iで行う。

## Context
- Issue #858(Parent #853 Phase A)。無償 Preview 公開の利用者保護ブロッカーとして、Community Node の利用有無に依存せず、初回起動時に 18 歳以上である旨の自己申告を必須にし、成人向け表現を明示的に許可するまで安全に非表示とする。
- 現行実装には成人向けを示すクライアント可視のラベルが存在しない。CN 側の NSFW 判定(ADR 0028)は index からの除外(`SafetyPolicy::on_high_confidence_nsfw` 既定 `Exclude`)であり、ラベルとして client へ配信されない。ADR 0025 §2.3 / ADR 0028 §2.6 は成人向け・暴力表現のサムネイル代替表示を client UI 設計に委譲している。
- アプリ同意ゲート(`docs/legal/app-consent-data-classification.md`)は、同意まで `DesktopRuntime` を構築せず iroh endpoint を bind しない fail-closed 構造を既に持つ。

## Decision

### 1. 年齢自己申告(age attestation)
- 初回起動時、アプリ利用開始の前提として「18 歳以上である」旨の明示的な自己申告を必須にする。申告が完了するまで `DesktopRuntime` を構築せず、ネットワーク接続を開始しない(既存アプリ同意ゲートと同一の fail-closed)。
- 自己申告は利用規約・プライバシーポリシーへの文書同意とは**別の記録**として `<db_path>.app-consent.json` に保存する(slug 方式の文書レコードとは独立した `age_attestation` レコード)。
- 生年月日・公的身分証等は収集しない。自己申告は公的な年齢確認ではないことを UI 文言と法務文書の双方に明記する。
- 記録はローカルのみ・非複製。新規端末では再申告を求める。

### 2. 成人向け表現の表示設定(adult content display)
- 「成人向け表現を表示する」設定は自己申告とは**別の状態**として扱い、既定 OFF とする。自己申告を済ませただけでは ON にならない。設定画面から明示的に変更できる。
- 設定の canonical source は Rust 側のローカル JSON(`<db_path>.content-display.json`)とし、frontend は mirror に徹する。バイト列を取得しない保証を UI 層のフラグに依存させないため、取得ゲートは Rust 側で enforce する。

### 3. 成人向けラベル(self-label)の信頼元
- v1 のラベル源は**投稿者自己申告**とする。投稿 envelope の署名対象 content(`KukuriPostEnvelopeContentV1`)に `content_labels`(文字列配列、現行の既知値は `adult` のみ)を追加し、投稿者が投稿時に付与する。
- ラベルは投稿者の署名で保護されるが、**申告の真正性は検証できない**。タグの付いていないコンテンツが安全であることを保証しない(fail-open の限界)。複数 Node / peer から同一投稿を観測した場合も、ラベルは署名済み envelope 由来であるため競合しない(envelope が異なれば別投稿として扱う)。
- CN advisory / VLM 判定結果をラベル源として合成することは本 ADR のスコープ外とする(ADR 0026 の relative trust の領分)。
  （2026-09-15 superseded: 設定済み / 購読 Community Node の `content_advisories` を第 2 のラベル源として合成する。§6）

### 4. 取得・キャッシュ制御
- 表示設定 OFF の間、成人向けラベル付き投稿の添付メディアについて、バイト列の取得・プリフェッチ・キャッシュ・デコードを行わない。
  - frontend: プリフェッチ対象から成人向けラベル付き添付を除外する。
  - Rust: `blob_media_payload` は、対象 hash が成人向けラベル付き投稿の添付として保持中のprojection・表示cache・保護記録から観測済みで、設定 OFF の場合、blob 取得を行わず `None` を返す。元投稿のprojectionが回収され再検証できない要求も、bytes取得前に `None` を返す。
- 表示設定 ON で成人向けメディアを取得する場合は ephemeral fetch(`fetch_blob_ephemeral`)を使い、ローカル blob store(`blobs.db`)へ永続化しない。
- 設定を OFF へ戻した場合、以後の取得を停止し、frontend の in-memory object URL(デコード済み表示)を破棄する。ephemeral fetch のためディスク上に成人向けメディアのキャッシュ残余は発生しない(ラベル付与前に通常経路で取得済みの blob は本 ADR の対象外)。
  （2026-09-30 superseded（#1419 AC-4）: 表示設定 ON の間に表示した成人向けの添付は、通常の添付と同じ remote cache（3 GiB・非利用 7 日）に「成人向け」の印を付けて置く。スクロールで再表示するたびに作者から取り直し、作者に届かないと表示できなかったため。同じ topic の ON の参加者へも提供する。OFF へ戻したら、印の付いた非保護の blob を索引で選び、1 回 128 件以内で消す。削除は OFF の反映と起動時に背景で始め、途中で止まっても次の反映で再開する。上の「ephemeral fetch で永続化しない」「キャッシュ残余は発生しない」は、この範囲で失効した。OFF の間の取得・保存・デコードを行わない点は変えない。）
- remoteのラベル根拠を回収した場合も、該当hashの表示済みobject URLを破棄する。通知を取りこぼしたときは全remote URLを破棄し、次の表示需要で元投稿を再検証する。

### 5. 表示
- 成人向けラベル付き投稿は、タイムライン一覧・スレッド詳細・引用/埋め込み・返信プレビュー・検索結果解決・ブックマーク・プロフィールタイムライン・object-backed 通知の各表示経路で一貫して代替表示にする。メディアはプレースホルダー(取得もデコードもしない)、テキストと通知 preview は安全な代替文言にする。
- Community Index が返す `IndexEntryView.text` は canonical content / safety label の信頼元にしない。署名済み投稿をローカル解決してラベルを確認するまで本文を表示せず、解決待ち・失敗・欠落はいずれも安全な代替文言にする。
- object-backed 通知は署名済み envelope 由来の `content_labels` を notification projection へ保存する。既存行などラベルを解決できない通知も設定 OFF では preview を表示しない。follow / DM 通知と actor avatar は v1 では object label の対象外だが、Rust 側取得ゲートは hash 単位で適用される。

## Consequences
- 成人向けラベルの必須 contract / scenario は `docs/legal/age-attestation-data-classification.md` と `docs/legal/adult-content-display-data-classification.md` に定める。
- ラベルなしコンテンツは通常表示(fail-open)であり、本機能はラベル付きコンテンツに対する保護に限られる。この限界は利用規約・UI 文言で明示する。
- CN advisory との合成、通報起点のラベル付与、DM の self-label は将来の別 issue の領分。
  （2026-09-15: CN advisory との合成は #1051 で決定し §6 に記録。通報起点のラベル付与と DM の self-label は引き続き別 issue）

## 6. 改訂追補（#1051、2026-09-15）: Community Node content advisory の合成

### 6.1 ラベル源
- ラベル源は次の 2 つとする。(1) 投稿者の署名済み self-label（`content_labels`、§3）。(2) 利用者が設定済み /
  購読している Community Node が発行した `content_advisories`（ADR 0028 §8.6。`label = adult`（nsfw）/
  `sensitive`（objectionable）、issuer_node_id / category / confidence / signal_id / basis 付き）。
- (2) は投稿の canonical でも署名対象でもない node-local な advisory であり、`content_labels` へ書き戻さず、
  投稿 projection の署名済み欄にも混ぜない（`content_advisories_are_separate_from_signed_content_labels`）。
- (2) の採用は node 単位の opt-in（設定済み / 購読 node のみ）とし、network 全体の判定として表示しない
  （ADR 0027 §2.1 / §2.8 の trust-semantics）。

### 6.2 取得ゲートの拡張
- 表示設定 OFF の間、advisory 付き blob hash も self-label 由来の hash と同じ Rust 側ゲート
  （`blob_media_payload`）で bytes 取得・プリフェッチ・キャッシュ・デコードを行わない
  （`advisory_labeled_media_respects_adult_display_gate`）。
- 表示設定 ON では ephemeral fetch で取得し、ローカル blob store へ永続化しない。OFF へ戻した場合は以後の取得を
  停止し表示済み object URL を破棄する（§4 と同一）。
  （2026-09-30: advisory 付きの添付も §4 の #1419 の改訂と同じく、ON の間の表示は印付きで remote cache に置き、OFF で消す。）
- advisory 付き hash の集合は表示 state と取得ゲート判定のための一時状態であり、永続 projection にするか
  in-memory にするかは実装 child（C3 / C4）で決めて `docs/legal/adult-content-display-data-classification.md` に記録する。

### 6.3 表示
- advisory 付き投稿は、見つける（index entry の `content_advisories` 由来、C3）と P2P タイムライン・スレッド・
  引用・返信プレビュー（自 node への一括照会由来、C4）で self-label 付き投稿と同じ代替表示（プレースホルダー）
  にする。
- 代替表示は発行 node、category、confidence、basis を説明でき、異議申し立て（`POST /v1/report` の
  `appeal.risk_signal_id`）へ導線を持つ。断定表現（「成人向けと認定」）にせず「Community Node の推定」と示す。
- 一覧では説明を常時展開しない（#1108、2026-09-17）。メディア枠を持つ投稿は枠の上に「成人向け画像 / 動画: 詳細は
  クリック」だけを示し、枠を持たない投稿は本文欄に同じ趣旨の短い操作だけを置く。枠・操作から開く詳細 dialog に、
  代替表示の説明文、推定の出所（投稿者の申告でもネットワーク全体の判断でもないこと）、発行 node、category、
  confidence、basis、異議申し立てをまとめて示す。短いラベルは分類名であり、推定であることの説明は dialog 内で
  欠かさない。dialog を開いてもメディアは描画せず、表示設定 OFF の取得ゲートは変えない。
- 一括照会（仮名 `POST /v1/advisories/lookup`）は認証 + 同意済み client から可視 post id / blob hash を受け、
  その node 自身が発行した advisory のみ返す（`advisory_lookup_returns_only_configured_node_signals`）。照会と
  見つけるは同じ subject について同じ現在の判定を返す（再 scan 後の整合は ADR 0028 §8.14）。送信する
  識別子は可視 post id / blob hash に限り、本文や social graph を含めない。外部送信表示
  （`docs/legal/external-transmission-notice.md` / `docs/legal/app-data-flow-inventory.md`）へ行を追加する。

### 6.4 法務文書
- 利用規約 第3条 4 項「成人向け表現の識別は投稿者の自己申告によるラベルに基づきます」は、第 2 のラベル源を
  含む文言へ C4 で改訂し、`LEGAL_BUNDLE_VERSION` を更新して再同意と i18n ミラー更新を行う。C1〜C3 の時点では
  現行文言のままとし、advisory の合成は C4 の merge と同時に有効化する。
- Community Node の moderation-policy 文書（Rust 生成、required=false）への文言追記と文書 version 2 化は C2 で行う。

### 6.5 変更しないもの
- 18 歳以上の自己申告の必須化、表示設定の既定 OFF、`<db_path>.content-display.json` を canonical とする構造。
- self-label も advisory も無いコンテンツは通常表示（fail-open）。advisory の有無は node 依存であり安全を保証しない。
- 必須 contract / scenario の正本は `docs/legal/adult-content-display-data-classification.md`（本追補で更新）。

### 6.6 C4 実装時の補足（#1056、2026-09-16）
- 採用は node 単位の設定 `content_advisory_enabled`（既定 true）で行う。node を設定すること自体を §6.1 の opt-in
  とし、利用者は設定画面で node ごとに採用を外せる。採用しない node へは一括照会を送らず、見つけるの index 応答に
  含まれる advisory も採用しない（`lookup_skips_nodes_with_content_advisory_disabled`）。
- 一括照会は `POST /v1/advisories/lookup` として実装した。読み口は verdict 行ではなく risk signal であり、
  `Cleared` と `expires_at` 失効を除外する。門は索引参照と同じ構成・有効化条件・安定コードを使う。
- client は照会が確定するまで、その投稿のメディアを取得せずスケルトンにする。確定後に advisory があれば代替表示、
  無ければ通常表示へ切り替える。照会先の有無が未確定（起動直後）の間も同様に取得しない。照会に失敗した場合は
  §6.5 の fail-open に従い通常表示へ戻す。
- 合成は利用規約 version 6 と同じ変更で既定有効にした（§6.4）。
