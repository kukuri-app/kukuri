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

**2026-10-09 改訂**（#1201 AC-1）: Google Play 版の公開に向け、Play の UGC・成人向け表現・Child Safety の
要件と現行の実装の対応、採用した判断を「§7 改訂追補」に記録する。Android 版は年齢の自己申告を生年月日の
入力で行い、「見つける」の「おすすめ」「発見」から成人向けの対象を除く。自己申告の必須化、表示設定の
既定 OFF と opt-in、ラベル無しコンテンツの fail-open は変更しない。

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

## 7. 改訂追補（#1201、2026-10-09）: Google Play 版の UGC・成人向け表現・Child Safety

### 7.1 対象と前提
- 対象は #1193 D1 の初回の機能（投稿・返信・リアクション・DM・公開 / 非公開 channel・画像 / 動画・暗号化端末
  backup・鍵の書出し / 取込み・QR / 専用リンクの移行・起動中の OS 通知）と、#1201 の固定対象 T1〜T4（通報 /
  block、初回起動と成人向け表示の設定・再起動・復元、表示の経路、公開基準と CSAM の受付・対応）。live・game・
  metaverse・Dome は入口を出さないため対象外とする。
- #1193 D6 により、成人向け表示は opt-in（18 歳以上の自己申告、表示設定の既定 OFF）を維持する。D8 により英国と
  EU 加盟国は配布の対象外とし、その地域固有の年齢確認は扱わない。
- 照合した Play の要件（2026-10-09 取得。番号は Play Console ヘルプの記事）: User Generated Content（9876937）と
  その解説（12923286）、対象年齢と neutral age screen（9867159）、Age-Restricted Content and Functionality
  （16302250）、Child Safety Standards（14747720・14585136）、Content Ratings（9898843）。
- 判定は Play の文面と、基準 commit `41fdb34f3` の symbol・画面・文書の照合である。Android 実機と Play Console
  の確認は含まない。実機は #1204、Console の申告は #1203 OP-1 と #1201 AC-5（OP-1）が行う（担当の境界は
  `docs/runbooks/android-play-release.md`）。

### 7.2 要件と現行の対応
| ID | Play の要件 | 現行の実装・文書 | 判定 | 差分の担当 |
| --- | --- | --- | --- | --- |
| R1 | UGC を作る前に利用規約へ同意させる | 起動時の同意画面（`ConsentGateView`、`accept_app_consents`）。同意まで runtime を作らない（`consent_required_rejects_network_sink_and_ready_allows_it`） | 満たす | — |
| R2 | 好ましくない内容と行為を規約で定め、禁じる | 利用規約 第5条（CSAM / CSE、権利侵害、詐欺・脅迫・嫌がらせ等）。憎悪・暴力の扇動、成人向け表現の申告、性的な画像の無断の共有の定めが無い | 不足 | §7.3 の 6、#1201 AC-4 |
| R3 | 投稿とユーザーをアプリ内で通報できる | 投稿と作者詳細の通報（`ReportRoutingDialog`）。送信先は Community Node の索引で観測した投稿・profile にだけ決まり、P2P で届いた投稿・返信・非公開 channel の投稿は送信先が無い（`resolveReportTargets`、ADR 0030） | 不足 | §7.3 の 2、#1201 AC-2a |
| R4 | 1 対 1 の機能（DM）の相手を block できる | 署名つきの block（`block_author`）は表示を隠すが、DM の可否は相互フォローだけで決まり（`direct_message_send_enabled`）、block の後も DM の受信・表示・通知・送信が続く（ADR 0022 の #961・#992） | 不足 | §7.3 の 3、#1201 AC-2b |
| R5 | 通報された内容とユーザーに対応する | 受付は Community Node の `cn_admin.reports`、確認は `cn-cli reports`、対応は自 node の送信防止と索引からの除外 | 満たす（R3 が前提） | — |
| R6 | アプリ内の課金が不適切な行為を促さない | 課金も広告も無い（#1193 D8） | 該当なし | — |
| R7 | 性的表現を既定で filter の後ろに隠す | 表示設定の既定 OFF。投稿者の申告と採用した Community Node の推定が付いた投稿・添付は代替表示にし、バイト列を取得しない（§4〜§6、`adult_labeled_media_payload_is_blocked_until_display_enabled`、`advisory_labeled_media_respects_adult_display_gate`）。ラベルの無いもの、推定の照会の失敗、Community Node の未設定、DM の添付、avatar、カスタムリアクションの画像は通常表示 | ラベル付きは満たす | §7.3 の 5 |
| R8 | filter の完全な解除に 2 操作以上を要する | 設定を開く → 「セーフティ」 → checkbox の 3 操作（Play の解説の例と同じ数え方）。代替表示からは解除できない | 満たす | — |
| R9 | 性的表現を勧めない・目立たせない | 「見つける」の「おすすめ」「発見」は Community Node の新着の一覧（`index_recommendations`）で、推定の付いた投稿も含む。表示を ON にした利用者には成人向けの投稿が並ぶ | 不足 | §7.3 の 4、#1201 AC-3b |
| R10 | 主として性的でない内容を扱う | 一般の SNS | 満たす | — |
| R11 | 子どもの利用を、必要な年齢を示さない年齢確認（neutral age screen）などで禁じる | 「私は18歳以上です。」の checkbox。必要な年齢を示すため、Play の定義の neutral age screen に当たらない | 不足 | §7.3 の 1、#1201 AC-3a |
| R12 | 年齢の申告と成人向け表示の許可を他の端末から引き継がない（T2） | backup の復元は同意と申告を戻して表示を OFF にする（ADR 0048）。QR / 専用リンクの移行は移さない（ADR 0062 §7） | 満たす | Android の OS の自動 backup からの除外は #1195 AC-2 |
| R13 | rating の質問票の UGC への正確な回答と対象年齢 | Play Console の申告 | — | #1203 OP-1 |
| R14 | CSAE を禁じる公開の基準（Web で読め、app 名を含み、Play Console に URL を登録する） | 利用規約 第5条 2 は CSAM / CSE を禁じるが、LP に公開の基準のページが無い | 不足 | #1201 AC-4 |
| R15 | アプリを離れずに運営者へ懸念を送れる | 「フィードバックを送る」（tester feedback。公式の Community Node が受け付ける）。Community Node の未設定・未同意では送れず、運営者の連絡先は規約の本文の中にしか無い | 不足 | #1201 AC-4 |
| R16 | CSAM を知ったら対応し、確認した CSAM を NCMEC または地域の機関（日本はインターネット・ホットラインセンター）へ通報する手順を持つ | 既知の hash と一致したものは自 node の索引・発見から除く（ADR 0027 §7）。通報の手順・担当・期限は runbook に無い | 不足 | #1201 AC-4 |
| R17 | Child Safety の担当窓口を Play Console に登録する | Play Console の申告 | — | #1201 AC-5（OP-1） |

### 7.3 採用した判断（2026-10-09、ユーザーの選択）
1. 年齢確認（R11）: Android 版は、同意画面の文書より前に生年月日の入力（既定値を置かず、必要な年齢を示さない）を
   置き、端末内で満 18 歳以上かを判定する。生年月日は保存も送信もせず、§1 の申告の記録だけを残す。満たさなければ
   runtime を作らずに止める。入力のやり直しの防止は扱わない。desktop と Web は現行の checkbox のまま。§1 の
   「生年月日は収集しない」の改訂と実装は #1201 AC-3a が行う。Play Console では対象年齢を 18 歳以上だけにし、
   未成年のアクセス制限を有効にする（#1203 OP-1）。
2. 通報の送信先（R3）: 関与した node を特定できない投稿・ユーザーの通報では、設定済みで通報を受け付ける node を
   候補に出し、利用者が選んで送る。自動では送らない。選んだ node は自ら提供する機能の範囲でだけ対応でき、P2P 全体
   からは消せないことを示す（既定の構成では運営者の公式の node が受け取る）。ADR 0027 §2.8、ADR 0030、責任境界の
   文書（`docs/architecture/p2p-first-community-node-responsibility-boundary.md`）の改訂と実装は #1201 AC-2a が
   行う。全 client に適用する。
3. block と DM・通知（R4）: どちらかが block した相手とは DM を送受信しない（届いた frame を保存・表示・通知せず、
   送信と再送を止め、入力欄を無効にし、過去の履歴は残す）。自分が block した相手を actor とする通知を出さない。
   ADR 0020・0022・0023 の改訂と実装は #1201 AC-2b が行う。全 client に適用する。
4. 「おすすめ」「発見」（R9）: 表示設定で隠す対象（投稿者の申告または採用した Community Node の推定が付いた投稿）を、
   表示設定の ON / OFF によらず「見つける」の「おすすめ」「発見」から除く。検索の結果と §5 の各経路は変えない。
   §5 の改訂と実装は #1201 AC-3b が行う。全 client に適用する。
5. ラベルの無いもの（R7）: 現行の fail-open を維持する。Play の要件は filter の判定の完全さまでは求めておらず、
   限界は利用規約 第3条 4 項と設定の説明に示している。通報・block と採用した Community Node の推定で補う。審査で
   ラベルの無い性的表現が既定の表示で見つかる危険は受け入れる。
6. 規約の禁止事項（R2）: 利用規約 第5条に、憎悪・暴力の扇動、成人向け表現を成人向けの申告なしで共有すること、
   性的な画像を写っている本人の同意なく共有することを加える。CSAE の公開基準（R14）と同じ改訂で #1201 AC-4 が
   行う。
- 法務文書への反映（利用規約 第3条・第5条・第13条、プライバシーポリシー）は #1201 AC-4 が行い、legal bundle の
  版上げは 1 回にまとめる。同じ統合 branch で法務文書を改訂する #1203 AC-2 とは、配布の前なら同じ版にする。外部送信
  表示は、同意した Node への通報を既に含むため、2 の判断では変えない。
- rating の質問票（R13）は #1203 OP-1 が採用した機能に基づいて回答する。本 ADR から渡す事実: 利用者どうしが
  テキスト・画像・動画を公開・非公開の範囲で共有し、DM で 1 対 1 でやり取りできる。性的表現は既定 OFF の filter の
  後ろにある opt-in の表示だけで、18 歳以上だけを対象にする。課金と広告は無い。

### 7.4 変更しないもの
- 18 歳以上の自己申告の必須化、表示設定の既定 OFF と opt-in、`<db_path>.content-display.json` を正本とする構造、
  投稿者の申告と採用した Community Node の推定の 2 つのラベル源、ラベルの無いものの fail-open（§6.5）。
- desktop と Web の年齢確認と、年齢の申告と成人向け表示の設定を端末ごとに持つこと（ADR 0048、ADR 0062 §7）。
- 通報を自動で既定の node や運営者へ集めないこと、各 node が自 node の範囲でだけ対応すること（ADR 0027 §2.1・
  §2.8）。
- 対象外とするもの: DM のメッセージ単位の通報（相手は作者詳細から通報できる。Play は 1 対 1 の機能に block を
  求める）、カスタムリアクションの画像の通報と成人向けの申告、引用 repost の成人向けの申告、ミュートした相手の通知、
  英国・EU の年齢確認。
