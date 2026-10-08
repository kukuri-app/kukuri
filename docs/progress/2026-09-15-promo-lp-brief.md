# LP・告知素材の共通brief（Issue #1037）

## この文書の位置づけ

- 所有Issue: #1037（統括 #1036）。本書は後続 #1039 / #1040 / #1041 / #1042 / #1043 / #1044 が参照する共通briefの正本。
- Scope revision: 2026-10-08-promo-lp-brief-v8（v7からの変更: LP の構成と原稿を #1667 の 4 つの役割群へ改めた。LP の正本は「LP の原稿（4 つの役割群）」と「LP の FAQ」で、旧 7 セクションの原稿・FAQ 8 項目は失効した。Dome の予告・設定表・通信 3 経路の説明・6 段階の手順は LP に載せない。Dome 予告の画像・preset と AC-3 の出力一覧は #1668 が改める。Product Hunt・note・X の固定出力は変えない）。直前: 2026-09-19-promo-lp-brief-v7（v6からの変更: 配布候補を v0.2.8-preview.2 へ更新した。2026-09-19 の公開に合わせた操作者の依頼）。直前: 2026-09-18-promo-lp-brief-v6（v5からの変更: 配布候補を v0.2.7-preview.1 へ更新した。2026-09-18 の公開に合わせた操作者の依頼）。直前: 2026-09-18-promo-lp-brief-v5（v4からの変更: X の画像を #1041 の固定出力に合わせて 1600×900 の 1 枚から 1920×1080 の 2 枚へ変えた。各出力の設定の正本は `tools/promo/presets/stills.json`）。直前: 2026-09-18-promo-lp-brief-v4（v3からの変更: 配布候補を v0.2.6-preview.1 へ更新した。2026-09-18 の公開に合わせた操作者の依頼）。直前: 2026-09-18-promo-lp-brief-v3（v2からの変更: Dome予告を操作者が提供した実機の静止画1枚に限定し、動画から外した。S9の画面に限り、操作者本人とテスト用アカウントの名前の写り込みを許可した。いずれも 2026-09-18 の操作者の判断）
- 作業日: 2026-09-18
- 作業時の main: `d455bdd8e03541de9734f06a8e050a72bdb01bfd`
- 配布候補release: `v0.2.8-preview.2`（2026-09-19公開）。Issue起票時点の `v0.2.3-preview.2` から `v0.2.5-preview.3`、`v0.2.6-preview.1`、`v0.2.7-preview.1` を経て更新した（2026-09-19、公開に合わせて操作者が依頼）。`v0.2.8-preview.1` は release workflow の失敗で公開されていない。LP が案内する release の正本は `apps/lp/release.json`。
- 配布候補commit: `a9536dfdf508bedcd69931fdebd4fb622bf75e5e`（tag `v0.2.8-preview.2` のpeel先。main の祖先）。#1039 の画面は `v0.2.6-preview.1`（`4b7519460e8baf3c29d7ec0eadc6eb2c727a1065`）の上から撮った。v0.2.7-preview.1 の変更は告知素材・LP・CI と、画面の見た目を変えない修正（Windows 初回起動、添付の取得ゲート、Metaverse）。v0.2.8-preview.2 の UI の変更（URL の OGP プレビュー #1179、リアクション名の tooltip #1178、composer への画像の貼り付け #1177、画像 viewer のクリッピング #1175）は、デモの投稿に URL が無く、tooltip・貼り付け・viewer も撮影する画面に写らないため、どちらも撮り直さない。
- 本書は製品仕様の正本ではない。仕様は `docs/adr/`、実行手順は `docs/runbooks/`、視覚契約は `DESIGN.md` を正本とし、本書はそれらを素材制作の条件へ翻訳したものとする。

## AC-1: 配布候補の事実確認

### release と配布物

出典: `gh release view v0.2.8-preview.2`（2026-09-19取得）、[release一覧](https://github.com/kukuri-app/kukuri/releases/latest)、`CHANGELOG.md`、`README.md`。

| 事実 | 値 | 出典 |
| --- | --- | --- |
| tag | `v0.2.8-preview.2` | GitHub Release |
| 公開日時 | 2026-09-19T03:51:39Z | GitHub Release |
| commit | `a9536dfdf508bedcd69931fdebd4fb622bf75e5e` | `refs/tags/v0.2.8-preview.2^{}` |
| 表示バージョン | `0.2.8`（asset名）／`v0.2.8-preview.2`（tag） | asset名とtag |
| channel | preview | release notes「Preview channel: preview」 |

配布asset（素材に載せてよい配布物はこの一覧に限る）。

| 対象 | asset | 備考 |
| --- | --- | --- |
| Windows 10 / 11 | `kukuri_0.2.8_x64-setup.exe`（+ `.sig`） | NSIS installer。x64のみ |
| Linux GUI | `kukuri_0.2.8_amd64.AppImage`（+ `.sig`）、`kukuri_0.2.8_amd64.deb`（+ `.sig`） | x86_64 / amd64のみ |
| Linux CLI | `kukuri-cli_0.2.8_x86_64-unknown-linux-gnu.tar.gz`、`kukuri-cli_0.2.8_aarch64-unknown-linux-gnu.tar.gz` | GUIとは別profile |
| 更新 | `latest-preview.json`、`windows-x86_64-updater-key.pub`、`linux-x86_64-updater-key.pub` | 更新manifestは署名あり |
| 検証・表示 | `SHA256SUMS.txt`、`THIRD_PARTY_NOTICES.md`、`release-provenance.json`、`kukuri_0.2.8_linux-native-compliance.json` | |

macOS packageとaarch64 GUI packageは存在しない。素材でmacOS・iOS・Androidのアプリを示唆しない。Web版（`https://app.kukuri.app/`）は v0.4.0-preview.1 から公開され、LP はブラウザ版を案内する（v8）。

### 素材で使える主張と、その根拠

| # | 主張（LP・告知で使ってよい表現） | 根拠 |
| --- | --- | --- |
| F-1 | 話題（topic）を選んで参加する、話題起点のP2P型SNS | `README.md` 冒頭、`docs/architecture/p2p-first-community-node-responsibility-boundary.md` |
| F-2 | アカウントを識別する鍵は端末内にのみ保存され、中央での再発行・復旧はない | `README.md`「Preview Status and Limits」、`docs/runbooks/mvp-user-quickstart.md`「アカウント鍵だけの持ち出しと復元」 |
| F-3 | 接続の優先順位はDirect P2P → Relay Supported P2P → Relay Fallback | `README.md`「How kukuri Works」、`AGENTS.md`「通信経路」 |
| F-4 | Community Nodeは投稿・プロフィール・社会グラフの恒久的な保管先ではなく、初期接続・認証・rendezvous・接続支援・索引・moderation・通報の範囲で手伝う | `README.md`、`docs/architecture/p2p-first-community-node-responsibility-boundary.md` |
| F-5 | 公開投稿とプライベートチャンネルを同じ話題の中に置ける | `README.md`「What You Can Do」、`apps/desktop/src/components/extended/PrivateChannelPanel.tsx` |
| F-6 | プライベートチャンネルの公開範囲は「招待限定 / Invite only」「相互フォロー限定 / Mutuals」「相互フォロー+ / Mutuals+」の3種類で、既定は招待限定 | `crates/core/src/private_channels.rs:12-20`（`ChannelAudienceKind` の `#[default] InviteOnly`）、`apps/desktop/src/i18n/locales/{ja,en}/channels.json`（`audienceOptions`） |
| F-7 | 日本語・English・简体中文のUIと、light / darkテーマ | `README.md`「Available Today」、`docs/runbooks/mvp-user-quickstart.md` |
| F-8 | 初回起動時に年齢の自己申告とアプリ規約への同意が必要で、同意まではネットワーク接続を開始しない | `README.md`「Try It in 3 Minutes」手順2、`docs/legal/app-consent-data-classification.md` |
| F-9 | Community Nodeごとに、その文書への明示同意が別途必要 | `README.md`、`docs/runbooks/mvp-user-quickstart.md` 手順2 |
| F-10 | Community Indexで投稿と話題を検索・発見できる（提供するNodeに限る） | `README.md`「Available Today」、`apps/desktop/src/i18n/locales/ja/shell.json`（`topicSummary` ほか） |
| F-11 | 相互の関係がある相手とDMでき、画像・動画を添付できる | `README.md`「What You Can Do」 |
| F-12 | アカウントを識別する鍵の暗号化エクスポート／インポートと、端末バックアップ／復元がある | `docs/runbooks/mvp-user-quickstart.md` |
| F-13 | preview installerにOSコード署名はなく、Windows SmartScreenの警告が出うる。アプリ内更新のmanifestは署名され、改ざんされたmanifestは拒否する | `README.md`「Download the Builder Preview」、release notes「Known limits」 |
| F-14 | Builder Preview（テスター向け）であり、一般向けの安定版ではない | `README.md`、release notes |

未解決の主張は0件。次の表現は根拠がないため素材に使わない。

- 「どのサービスとも相互運用できる」（`README.md`「Nostr compatibility is intentionally limited」と矛盾する）
- 「サーバー不要」の断定（Community Nodeへの同意と接続支援が前提の経路がある）
- 「匿名性を保証」（`docs/legal/privacy-policy.md` の範囲を超える）
- 利用者数、導入実績、第三者の推薦、受賞。いずれも根拠がない。

### 既定で使える機能と、開発者モード限定の実験機能

| 区分 | 対象 | 根拠 |
| --- | --- | --- |
| 既定 | トピック発見、Community Index検索、公開投稿、返信・スレッド、リアクション、リポスト、引用、ブックマーク、ミュート・ブロック、プライベートチャンネル（作成・参加・共有）、DM、通知、プロフィール、フォロー、画像・動画添付、鍵のエクスポート／インポート、バックアップ／復元、更新確認、フィードバック送信 | `README.md`「Available Today」 |
| 開発者モード限定（実験中） | Live sessions、Game rooms、Streamカラム、Metaverse Dome一式（Dome hosting、seamless transition、spatial audio、follow camera、connection map、Dome management） | `README.md`「Preview Status and Limits」／「Available Today」の Experimental 行 |

実装上の切り分け（撮影条件の根拠）。

- 開発者モードの状態は `localStorage` の `kukuri.desktop.developer-mode`（`apps/desktop/src/lib/developerMode.ts:1`）。既定は無効。
- 無効時はColumn追加候補から `stream` と `metaverse` を除外する（`apps/desktop/src/shell/page/DesktopShellControlCenter.tsx:156-160`）。
- 無効時は `live` / `game` のroute遷移を受け付けない（`apps/desktop/src/shell/useDesktopShellRouting.ts:164`）。
- 切替は `設定 → 開発者`。

したがって、3場面（S1〜S3）は**開発者モードを無効のまま**撮影する。Dome予告（S9）だけ開発者モードを有効にした実機の静止画を使い、manifestの `developerMode` を `true` で記録する。

### 3場面を構成する既定機能の入口

| 場面 | 入口 | 実装上の根拠 |
| --- | --- | --- |
| ①話題を選ぶ | 初期トピック `kukuri:topic:general` / `kukuri:topic:dev` / `kukuri:topic:test`、および「見つける」カラムのCommunity Index検索 | `apps/desktop/src/shell/slices/shared.ts:24-29`、`apps/desktop/src/i18n/locales/ja/shell.json`（`topicSummary`） |
| ②公開で会話する | タイムラインカラムの投稿作成、返信、スレッド、リアクション | `README.md`「Available Today」、`docs/runbooks/mvp-user-quickstart.md` 手順5 |
| ③私的チャンネルへ移る | タイムラインカラム上部の「プライベートチャンネル」ボタン（`workspace.privateChannelEntry`）、またはコントロールセンター → 場所 → チャンネル作成・参加。参加後の共有は「設定と共有」 | `apps/desktop/src/i18n/locales/ja/shell.json:277-278`、`apps/desktop/src/components/extended/PrivateChannelPanel.tsx`、`docs/runbooks/mvp-user-quickstart.md` 手順6 |

### LP の FAQ

LP の FAQ は最大 3 項目とし、参加を判断する内容に絞る（#1667。v7 までの Q-1〜Q-8 は失効）。回答は閉じた状態で置く。

| ID | 問い（JA / EN） | 事実 | 根拠 |
| --- | --- | --- | --- |
| LQ-1 | どの環境で使えますか / Where can I use it? | ブラウザ版はインストール不要で、パソコンの Chrome・Firefox・Safari と Android の Chrome で動作を確かめている。アプリ版は Windows 10 / 11 と Linux（x86_64）向けで、macOS・iOS・Android のアプリは無い。端末全体のバックアップなど、アプリ版だけの機能がある。 | ADR 0060 §3・§4、`apps/lp/release.json` |
| LQ-2 | はじめるときに必要なことは / What do I need to get started? | 最初に18歳以上であることの申告とアプリ規約への同意が要り、同意するまでネットワークへ接続しない。コミュニティノードは、その文書を読んで同意したものだけを使う。 | F-8、F-9 |
| LQ-3 | アカウントはどこに保存されますか / Where is my account stored? | アカウントを識別する鍵は、使っている端末（ブラウザ版はそのブラウザ）にだけ保存される。中央での復旧は無いので、設定から暗号化して書き出して保管する。 | F-2、F-12、ADR 0060 §3 |

## AC-2: LP原稿（JA/EN）と撮影台本

### 共通の物語

『話題を選ぶ → 公開で会話する → 同じ話題の中で小さな私的チャンネルへ移る』。3場面すべてで同じ架空の話題と同じ2人のデモ参加者を通す。

- 舞台にする話題: `kukuri:topic:dev`（初期トピック。架空のトピックを新規作成せず、既定で存在する話題を使う）
- 会話の題材: 「デスクトップのカラム配置をどう使っているか」という架空のやり取り
- デモ参加者: 表示名 `ふたば（デモ）` / `Futaba (demo)`、`みなと（デモ）` / `Minato (demo)`。どちらも操作者所有のデモidentityであり、実利用者ではない。
- 私的チャンネル名: `カラム配置の相談（デモ）` / `Column layout chat (demo)`、公開範囲は既定の「招待限定 / Invite only」。

すべての画面素材に「デモ画面 / Demo screen」の表記を入れる（表記の置き方はAC-3の共通ルールに従う）。

### LP の原稿（4 つの役割群）

v8（#1667）から LP は次の 4 つの役割群で構成する。section を増やして要素を並べない。旧 7 セクション（選べること・分散型である理由・Preview の手順・Dome 予告・再 CTA）の原稿は失効した。

文量の予算（#1667 の AC-1）:

- Hero: 見出しは JA 20 字・EN 12 語以内、補足は JA 45 字・EN 24 語以内。
- 場面のキャプション: JA 40 字・EN 18 語以内。
- main の本文: JA 900 字・EN 200 語以内。FAQ の回答とアプリ版のダウンロードの詳細は閉じた状態で、`main.innerText` を数える（JA は空白を除いた字数、EN は空白区切りの語数）。CTA・注記は含め、header・footer・alt・画像の中の文字は数えない。

#### ① Hero

| 項目 | 日本語 | English |
| --- | --- | --- |
| 見出し | 話題でつながる。運営は選べる。 | Connect through topics. Choose your operators. |
| 補足 | 検索やおすすめを任せる相手を自分で選べる分散型SNS。全体BANは無し。 | A decentralized social app where you choose who handles search and recommendations. No network-wide bans. |
| CTA | OS ごとの主ボタン 1 つと副ボタン（Microsoft Store で入手 / Linux版をダウンロード / ブラウザで開く） | Get it from Microsoft Store / Download for Linux / Open in your browser |
| 注記 | テスター向けの Builder Preview（版）で、18歳以上の方が対象です。 | A Builder Preview for testers (version), for ages 18 and up. |
| 画面 | 話題・投稿・返信が読める実画面（#1668） | 同左 |

見出しと補足は 2026-10-08 にユーザーが確定した。「全体BANは無し」は「全体」に限る。ノードごとの BAN（そのノードの手助けだけが止まる。`crates/cn-core/src/admission.rs`、利用規約 第13条 2・3）は実在するので、「BANされない」とは書かない。

#### ② 利用場面

| 項目 | 日本語 | English |
| --- | --- | --- |
| 見出し | 話題から、会話へ | From topic to conversation |
| デモの表記 | 画面と会話はデモです。 | Screens and conversations are demos. |
| 場面 1 | **話題を見つける** ／ 検索やおすすめから、気になる話題と投稿を見つけます。 | **Find a topic** / Use search and recommendations to find topics and posts you care about. |
| 場面 2 | **会話に加わる** ／ 投稿に返信して、同じ話題が好きな人と話します。 | **Join the conversation** / Reply to posts and talk with people who like the same topic. |
| 場面 3 | **小さな輪で続ける** ／ 招待した人だけのチャンネルで、話の続きを楽しめます。 | **Continue in a small circle** / Keep talking in a channel open only to people you invite. |

各場面に画面を 1 枚ずつ置き、設定方法や機能の一覧を添えない。

#### ③ 運営を選ぶ

| 項目 | 日本語 | English |
| --- | --- | --- |
| 見出し | 運営を選ぶ | Choose your operators |
| 本文（1 文） | 検索やおすすめ、投稿の評価は、あなたが同意したコミュニティノードだけが担い、ノードにBANされても止まるのはそのノードの手助けだけです。 | Search, recommendations, and post ratings come only from Community Nodes you agree to, and a ban from a node only stops that node's help. |
| リンク | 仕組みを詳しく読む（責務境界の文書） | Read how it works |

説明は 1 文に限る（2026-10-08 のユーザー判断）。接続の優先順位・moderation の扱い・鍵の保存などの技術の説明は、リンク先の文書と FAQ に任せ、LP に別のカードで繰り返さない。

#### ④ はじめる

| 項目 | 日本語 | English |
| --- | --- | --- |
| 見出し | はじめる | Get started |
| CTA | Hero と同じ | Hero と同じ |
| アプリ版の詳細（閉じた状態） | アプリ版のダウンロード（Windows / Linux）: Microsoft Store、AppImage、deb、Linux CLI と各手順、リリースページ（チェックサム） | Download the app (Windows / Linux) |
| FAQ | 「LP の FAQ」の LQ-1〜LQ-3 | 同左 |
| 詳細リンク | はじめ方の詳しい手順（quickstart）・うまく動かないとき（troubleshooting） | Detailed getting-started guide · Troubleshooting |

規約・プライバシー・Community Node の文書（外部送信・通報方針・データ保持）・不具合の報告・質問と提案・変更履歴は footer に置く。footer にはデモの表記も置く。

### 撮影台本（shot list）

共通条件。特記のない限り、テーマはdark、言語はJA（EN版は同じ操作をEN UIで撮る）、開発者モードは無効、viewportは1600×1000（撮影解像度は#1038の環境契約に従う）。

| scene | cut | 目的 | 操作 | 画角 | 原素材 | 由来 | 字幕（JA / EN） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| S0 | S0-C1 | Hero用の全景 | 3カラム表示で静止 | ワークスペース全体 | 静止画 | mock | なし（Heroはコピーで説明） |
| S1 | S1-C1 | 話題の一覧から選ぶ | タイムラインカラムで話題を切り替える | タイムラインカラム中心 | 動画 | mock | 話題を選ぶ / Pick a topic |
| S1 | S1-C2 | Community Index検索 | 「見つける」で語句を入力し結果を表示 | 見つけるカラム | 動画 | mock | 同意したノードで検索する / Search through a node you consented to |
| S2 | S2-C1 | 公開投稿 | 投稿を作成して送信 | タイムラインカラム＋投稿欄 | 動画 | mock | 公開で投稿する / Post in the open |
| S2 | S2-C2 | 返信とスレッド | 投稿を開いて返信する | スレッドカラム | 動画 | mock | スレッドで続ける / Keep the thread going |
| S2 | S2-C3 | リアクション | 投稿にリアクションする | 投稿カード拡大 | 動画 | mock | 反応を返す / React |
| S3 | S3-C1 | 私的チャンネルの入口 | タイムライン上部の「プライベートチャンネル」を押す | カラム上部を含む | 動画 | mock | 同じ話題の中に、小さな輪 / A smaller circle, same topic |
| S3 | S3-C2 | 作成と公開範囲 | 名前と公開範囲を選んで作成 | チャンネル作成欄 | 動画 | mock | 公開範囲を選ぶ / Choose the audience |
| S3 | S3-C3 | 参加後の会話 | チャンネル内で投稿・返信 | チャンネルのタイムライン | 動画 | mock | 話題を離れずに話す / Stay in the topic |
| S4 | S4-C1 | 2台での実同期 | Windows実機で投稿し、Linux実機に届く | 2画面を並べる | 動画 | 実機（Windows 11 NSIS + Linux AppImage/deb） | 2台の実機で同期 / Synced across two real machines |
| S4 | S4-C2 | 私的チャンネルの実同期 | 招待リンクで参加し、投稿が届く | 2画面を並べる | 動画 | 実機 | 招待して、同じ輪へ / Invite, and join the same circle |
| S9 | S9-C1 | Dome予告 | 開発者モードを有効にした配布版で Dome を開き、タイムラインと並べる | Timeline カラムと Metaverse カラムを含む画面全体（切り抜かない） | 静止画1枚 | 実機（Windows 11、配布版 `v0.2.5-preview.3`、操作者が提供） | AC-2 の「必須の表記」と「画面の説明」。「画面の説明」は画像の外に置く |

- S1〜S3は#1039（mock撮影）が所有する。
- S4は#1040（実機撮影）が所有する。
- S9は操作者が配布版 `v0.2.5-preview.3` の実機で撮った静止画を使う（2026-09-18 に方針を変更）。`tools/promo/scripts/import-still.mjs` で由来付きの原素材として取り込み、spec を `tools/promo/device-captures/s9-dome-teaser.ja-dark.json` に置く。Dome の動画は撮らない。2 台で同じ Dome に入れないこと（#1140）とアバターのマテリアルが適用されないこと（#1141）が分かり、動画で見せられる状態ではないため。
- S9 の画面は、タイムラインと Metaverse が同じカラムの並びで両立することを示すため、切り抜かずに使う。左のタイムラインに操作者本人（KingYoSun）とテスト用アカウント（GrokTester・CliPeerA）の名前と投稿が写るが、2026-09-18 に操作者が公開候補への掲載を許可した。この例外は S9 の画面に限り、ほかの場面は引き続きデモ identity だけを写す。
- 実機素材（S4）とmock素材（S1〜S3）を1つのカットの中で混在させない。動画内で切り替える場合は、実機由来のカットに「実機 / Real devices」の表記を出す。

## AC-3: 固定出力一覧とscene IDの対応

### 共通ルール

- デモ表記: 画面を含むすべての出力に「デモ画面 / Demo screen」を、静止画・動画とも右上へ常時表示する（#1038 の実装で確定。動画の冒頭・末尾だけに出すと、途中を切り出した素材から表記が落ちるため常時表示にした）。実機由来の素材には「実機 / Real devices」を並べ、mockと区別する。人物名には表示名の時点で `（デモ）` / `(demo)` を含める。
- テーマ: アプリ画面はdarkで統一する（`DESIGN.md` §11.1の現行token。背景 `#121212`、パネル `#292929`、アクセント `#03dac5`、primaryボタン `#d77d45`）。LPの地の面は明るいニュートラル＋オレンジとし、デスクトップの高密度レイアウトをLPへ持ち込まない。
- 文字セーフエリア: 静止画は各辺6%、動画は各辺8%を文字の外側余白として空ける。Product Huntのgalleryは上下に各10%を空け、1枚目の見出しは上から18%〜38%の帯に置く。
- 字幕: 動画は無音で理解できることを必須とし、日本語版・英語版のどちらも焼き込み字幕を持つ。
- release表示: 画面内にバージョンが写る場合は `v0.2.8-preview.2` と一致させる。

### 固定出力一覧

| 出力ID | 媒体 | 形式・寸法 | 使用scene | 所有Issue | Dome |
| --- | --- | --- | --- | --- | --- |
| `lp-hero-ja` / `lp-hero-en` | LP | PNG 2400×1500 | S0-C1 | #1041 | 不可 |
| `lp-scene1-ja` / `lp-scene1-en` | LP | PNG 1600×1000 | S1-C1 | #1041 | 不可 |
| `lp-scene2-ja` / `lp-scene2-en` | LP | PNG 1600×1000 | S2-C2 | #1041 | 不可 |
| `lp-scene3-ja` / `lp-scene3-en` | LP | PNG 1600×1000 | S3-C2 | #1041 | 不可 |
| `lp-loop-ja` / `lp-loop-en` | LP | MP4 H.264/yuv420p 30fps 1600×1000 15〜20秒 | S1-C1, S2-C1, S3-C1 | #1042 | 不可 |
| `lp-loop-poster-ja` / `lp-loop-poster-en` | LP | PNG 1600×1000（動画停止時のfallback） | S1-C1 | #1041 | 不可 |
| `lp-dome-teaser` | LP ⑥内 | PNG（原素材 1906×1243 を元に #1041 が寸法を決める） | S9-C1 | #1041 | 掲出可（表記必須） |
| `ogp-ja` / `ogp-en` | LP | PNG 1200×630 | S0-C1 | #1041 | 不可 |
| `ph-gallery-1` | Product Hunt (EN) | PNG 1270×760（価値） | S0-C1 | #1041 | 不可 |
| `ph-gallery-2` | Product Hunt (EN) | PNG 1270×760（話題） | S1-C1 | #1041 | 不可 |
| `ph-gallery-3` | Product Hunt (EN) | PNG 1270×760（会話） | S2-C2 | #1041 | 不可 |
| `ph-gallery-4` | Product Hunt (EN) | PNG 1270×760（私的チャンネル） | S3-C2 | #1041 | 不可 |
| `ph-icon` | Product Hunt (EN) | PNG 240×240 | なし（アイコン） | #1041 | 不可 |
| `ph-video` | Product Hunt (EN) | MP4 1920×1080 30fps 30〜60秒 | S1〜S3 | #1042 | 不可 |
| `note-header` | note (JA) | PNG 1280×670 | S0-C1 | #1041 | 不可 |
| `note-body-1` / `-2` / `-3` | note (JA) | PNG 1280×720 | S1-C1 / S2-C2 / S3-C2 | #1041 | 不可 |
| `x-1-ja` / `x-2-ja` | X (JA) | PNG 1920×1080 ×2（価値 / 私的チャンネル） | S0-C1 / S3-C2 | #1041 | 不可 |
| `x-video-ja` | X (JA) | MP4 1600×900 30fps 20〜40秒 | S1〜S3 | #1042 | 不可 |

### Domeを含めてはならない出力

次はいかなる形でもDomeの画面・名称・3D空間を含めない。

- `lp-hero-ja` / `lp-hero-en`
- `lp-scene1` / `lp-scene2` / `lp-scene3`（JA/EN）
- `lp-loop` とその `lp-loop-poster`（JA/EN）
- `ogp-ja` / `ogp-en`
- `ph-gallery-1`（Product Huntの1枚目）
- `ph-icon`
- `note-header` / `note-body-1..3`
- `x-1-ja` / `x-2-ja`
- すべての動画（`lp-loop`、`ph-video`、`x-video-ja`）。2026-09-18 に、動画の末尾にも Dome を入れない方針へ変えた

Domeを掲出してよいのは `lp-dome-teaser` の静止画 1 枚だけとする。AC-2 の「必須の表記」と「画面の説明」をキャプションとして必ず添える。

`lp-dome-teaser` の原素材は画面全体で、下端近くまでタイムラインが写っている。重ねるとタイムラインが隠れ、両立を示す狙いが弱まるため、「画面の説明」は画像に焼き込まず、画像の外（LP の本文側）に置く（2026-09-18 の操作者の判断）。文言は取り込んだ manifest の `externalCaption` に残る。「必須の表記」の置き方は、タイムラインを隠さないことを条件に #1041 が決める。

## AC-4: 公開先と導線

### 現在の公開先（2026-09-18に確認）

| 対象 | 状態 | 確認方法 |
| --- | --- | --- |
| `kukuri.app`（apex） | A / AAAAレコードなし。`https://kukuri.app/` は到達不可 | `nslookup -type=A/-type=AAAA kukuri.app 8.8.8.8`、`curl -o /dev/null -w %{http_code}` → `000` |
| `www.kukuri.app` | NXDOMAIN | `nslookup www.kukuri.app` |
| 権威DNS | Cloudflare（`nova.ns.cloudflare.com` / `keanu.ns.cloudflare.com`） | `nslookup -type=NS kukuri.app 8.8.8.8` |
| `api.kukuri.app` | 稼働中（`35.190.227.178`）。Community NodeのAPIと法務文書を配信 | `nslookup`、各URLへのHTTPステータス |
| GitHub Release | 稼働中。配布物の唯一の公開元 | `gh release view` |

LPを置く `kukuri.app` のapexは現在なにも配信していないため、新LPの公開で既存の公開物を上書きしない。

### 新LPの配置とpreview／本番切替案

ユーザー確定事項（2026-09-18）: ドメインは `kukuri.app`、ホスティングはCloudflare Pages。

1. Cloudflare Pagesのプロジェクトを作り、リポジトリの `apps/lp`（#1043が実装先を確定する）からビルドする。
2. preview環境はPagesが発行するpreviewサブドメインを使い、`kukuri.app` へは接続しない。#1043・#1044の確認はpreview URLで行う。
3. 本番切替は、Cloudflare DNSで `kukuri.app` のapexをPagesプロジェクトへ向ける操作だけで行う。`www.kukuri.app` を作る場合はapexへリダイレクトする。
4. 切替はこのIssue群の完了条件に含めない（#1036の完成範囲はLP previewまで）。切替手順は#1044の公開作業引継ぎに記載し、実行はユーザーが判断する。
5. `api.kukuri.app` の設定は変更しない。LPは同ホストの法務文書へリンクするだけとする。

正規URL（本番切替後）: `https://kukuri.app/`（日本語）、`https://kukuri.app/en/`（英語）。言語切替は相互リンクを持ち、OGPは言語ごとに `ogp-ja` / `ogp-en` を割り当てる。

### LPから張るリンク

| 用途 | URL | 状態 |
| --- | --- | --- |
| ダウンロード（全体） | `https://github.com/kukuri-app/kukuri/releases/latest` | 稼働中 |
| Windows | 上記releaseの `kukuri_0.2.8_x64-setup.exe` | 稼働中 |
| Linux AppImage / deb | 上記releaseの `kukuri_0.2.8_amd64.AppImage` / `kukuri_0.2.8_amd64.deb` | 稼働中 |
| Linux CLI | 上記releaseの `kukuri-cli_0.2.8_*.tar.gz` | 稼働中 |
| quickstart | `https://github.com/kukuri-app/kukuri/blob/main/docs/runbooks/mvp-user-quickstart.md` | 稼働中 |
| troubleshooting | `https://github.com/kukuri-app/kukuri/blob/main/docs/runbooks/mvp-troubleshooting.md` | 稼働中 |
| 変更履歴 | `https://github.com/kukuri-app/kukuri/blob/main/CHANGELOG.md` | 稼働中 |
| feedback（不具合） | `https://github.com/kukuri-app/kukuri/issues` | 稼働中 |
| feedback（提案・質問） | `https://github.com/kukuri-app/kukuri/discussions` | 稼働中 |
| 利用規約 | `https://api.kukuri.app/terms` | HTTP 200 |
| プライバシー | `https://api.kukuri.app/privacy` | HTTP 200 |
| 外部送信 | `https://api.kukuri.app/external-transmission` | HTTP 200 |
| 通報方針 | `https://api.kukuri.app/abuse-policy` | HTTP 200 |
| データ保持 | `https://api.kukuri.app/data-retention` | HTTP 200 |
| 責務境界の説明 | `https://github.com/kukuri-app/kukuri/blob/main/docs/architecture/p2p-first-community-node-responsibility-boundary.md` | 稼働中 |

mobileからの閲覧では、ダウンロードの代わりに「PCで開くためのリンクを控える」導線を出す（実装は#1043）。

### 計測の計画（導入はしない）

#1036のNon-goalsに新規analyticsの導入が入っているため、本Issueでは計画の記載までとする。区別する3つ。

| 指標 | 取り方の案 | 備考 |
| --- | --- | --- |
| 訪問 | Cloudflare Web Analytics（Pagesに同梱、cookieなし・個人識別なし） | 導入可否は公開作業時にユーザーが判断する |
| DLクリック | LP内のダウンロードボタンのクリックを、OS別・言語別に区別して数える | 個人を識別する値を送らない |
| インストール | GitHub Releaseのasset別ダウンロード数（`gh api` で取得可能）を、LPのDLクリックとは別系列として見る | asset DL数はインストール完了数ではないため、近似として扱う |

3指標を1つの数字にまとめない。プライバシーポリシーに記載のない収集を追加しない。

## 維持する契約の確認

| ID | 内容 | 本書での担保 |
| --- | --- | --- |
| INVAR-1 | 未実装機能、架空の利用者数・推薦、デモを実績に見せる表現を含めない | AC-1「素材で使える主張と、その根拠」で使用可能な主張をF-1〜F-14に限定し、使わない表現を明示。デモ表記をAC-3の共通ルールで必須化 |
| INVAR-2 | 既存の製品仕様と公開先を変更せず、#602・#603・#604は過去のClosed Issueとして参照する | 本書は文書のみを追加し、製品コード・DNS・既存公開物を変更しない。`api.kukuri.app` は参照のみ |
| INVAR-3 | 開発者モード限定の実験機能を既定機能として描かず、掲出は末尾予告1件に限り、常に実験中・開発者モード限定の表記を伴う | AC-2でDome予告文・必須表記・画面の説明を固定し、AC-3で掲出可能な出力を `lp-dome-teaser` の静止画 1 枚に限定、非掲出出力を一覧化 |

## 未確認・後続への引き継ぎ

1. 撮影対象releaseは撮影直前に再照合する。`v0.2.8-preview.2` より新しいpreviewが出た場合、#1040 などの撮影の着手時に再固定し、本書のScope revisionを更新する。LP のダウンロードは `apps/lp/release.json` と `apps/lp/scripts/sync-release.mjs` で更新する（`docs/runbooks/lp-publish.md`）。Dome の静止画（S9）は `v0.2.5-preview.3` の実機で撮ったもので、開発中の実験機能の予告として使い続ける。
2. LPの実装先path（`apps/lp` など）とビルド方法は#1043が確定する。本書はホスティング（Cloudflare Pages）と正規URLだけを固定する。
3. Cloudflare Pagesプロジェクトの作成とDNS切替はユーザーの操作であり、#1044の引継ぎ文書に手順を書くまでが本Issue群の範囲。
4. 実機撮影（S4）に使うLinux実機の接続手段は#1040の着手時に確認する。本書では実機素材の由来表記の条件だけを固定した。
5. Remotionのライセンス確認は#1038が所有する。ユーザーの申告では開発者は1名でありFreeライセンスの条件内だが、適用条件と出典の記録は#1038で行う。
