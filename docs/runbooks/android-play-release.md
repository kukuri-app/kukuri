# Android の Google Play 配布

## 対象

Android 版（#1193）を Google Play で配布するための、開発者アカウントの登録、配布 identity、公開設定、署名証明書の受け渡し、closed test から production access までの工程を扱う。登録・identity・公開設定の記録は #1200 AC-1 が所有する。公開の手順（候補の識別、実機試験、審査、配布停止と修正版、公開後の確認）は #1204 AC-2 が本書へ足す。

Console での登録・支払い・本人確認・設定は登録者の外部作業（#1200 の OP-1・OP-2）であり、repository の変更や CI の成功で代替しない。登録の準備、アプリの作成、内部配布、一般公開は別の状態として記録する。

法的氏名、住所、連絡用の電話とメール、本人確認書類、支払い情報、upload 鍵とその password を repository・Issue・log に書かない。OP の記録は #1200 の Current status に、実施日と各確認の状態だけを残す。配布の署名鍵と upload 鍵は kukuri の account 鍵とは別に管理する。

公式の要件は 2026-10-09 に確認した。

## 固定の値

| 項目 | 値 | 根拠・設定の正本 |
| --- | --- | --- |
| 配布 | Google Play のみ。AAB を提出し、直接配布の APK と別の package は作らない | #1193 D3 |
| 開発者アカウント | 個人（Personal）として新規登録 | D4 |
| package 名（applicationId） | `app.kukuri.android`。最初の成果物の upload で固定され、変更・削除・再利用できない | D5。Android の設定は #1194 AC-1 が置く。desktop の identifier `app.kukuri.desktop` は変えない |
| アプリ名 | kukuri（英語・日本語とも） | D5 |
| 配布の署名鍵 | Google が生成・保管する（Play App Signing の既定）。Console で鍵を変えない | D3 |
| upload 鍵 | 最初の release に署名した鍵が upload 鍵になる。生成と CI の秘密は #1199 | D3 |
| minSdk・配布 ABI | 29・arm64-v8a | D7（#1194） |
| 価格・課金・広告 | 無料、IAP なし、広告なし。無料で公開した app は有料に変えられない | D8 |
| 掲載の言語 | 既定は英語（en-US）。日本語（ja-JP）を翻訳として足す | D8、2026-10-09 確定 |
| カテゴリ | アプリ／ソーシャル | 2026-10-09 確定 |
| 公開する名義と連絡先 | 開発者名 KingYoSun、開発者メールと掲載の連絡先メール `ops@kukuri.app`、website `https://kukuri.app/`。電話番号は載せない | 2026-10-09 確定 |
| 配布地域 | 「配布地域」の表 | D8 |

## 担当の境界

| Console の作業 | 担当 | 本書との関係 |
| --- | --- | --- |
| 開発者アカウントの登録、本人確認、連絡先と端末の確認 | #1200 OP-1 | 「開発者アカウントの登録」 |
| アプリの作成、Store settings（カテゴリ・連絡先）、価格、国・地域、Play App Signing、package 名の登録の確認 | #1200 OP-2 | 「アプリの作成と配布設定」「配布地域」 |
| upload 鍵、署名した AAB、versionCode、Play 版の更新の経路、実機の install と upgrade | #1199 | 証明書の fingerprint を渡す |
| App content の申告（Data safety、権限、広告、コンテンツのレーティング、対象年齢、アプリへのアクセス、アカウント削除）と privacy policy の URL | #1203 OP-1。Child Safety は #1201 OP-1 の記録を使う | 広告なし（D8）を値として渡す |
| 掲載の文言と素材、審査手順、internal test から公開後の確認まで | #1204 | アプリ名、既定の言語、配布地域、closed test の工程を入力にする |

## 開発者アカウントの登録（OP-1）

公式: [登録](https://support.google.com/googleplay/android-developer/answer/6112435)、[必要な情報](https://support.google.com/googleplay/android-developer/answer/13628312)、[本人確認](https://support.google.com/googleplay/android-developer/answer/10841920)、[端末の確認](https://support.google.com/googleplay/android-developer/answer/14316361)。

1. 18 歳以上の Google アカウントで Play Console に登録し、Developer Distribution Agreement に同意する。
2. 登録料 US$25 を 1 回払う。クレジットカードかデビットカードを使い、プリペイドカードは使えない。
3. アカウントの種類に個人を選ぶ。
4. Google payments profile（法的氏名・住所）を結び付ける。profile が確認済みでなければ政府発行の身分証明書を出す。書類の内容は profile と完全に一致させる。
5. 連絡用のメールと電話、開発者メールをワンタイムコードで確認する。電話の確認は本人確認と端末の確認の後になる。
6. root 化していない Android 10 以上の実機に Play Console アプリを入れ、所有者のアカウントで端末を確認する。
7. Play Console の Settings > Developer account で本人確認が済んだことを確かめる。確認が済むまで app を提出できない。

個人アカウントで Play に公開される情報:

| 項目 | 公開 | 値 |
| --- | --- | --- |
| 開発者名 | される。後から変えられる | KingYoSun |
| 開発者メール | される | `ops@kukuri.app` |
| 法的氏名と国（法的住所の国） | される | 登録者の値。repository・Issue に書かない |
| 住所 | 収益化しないので公開されない。日本の特定商取引法の表示も有料 app と IAP の場合だけ | 同上 |
| 連絡用のメールと電話 | されない（Google からの連絡用） | 同上 |

## Android developer verification の適用

公式: [概要](https://developer.android.com/developer-verification)、[Play Console での手続き](https://developer.android.com/developer-verification/guides/google-play-console)、[Play の package 名の登録](https://support.google.com/googleplay/android-developer/answer/16984799)、[FAQ](https://developer.android.com/developer-verification/guides/faq)。

| 条件 | kukuri の値 | 適用される要件 | 必要な登録状態と確認場所 |
| --- | --- | --- | --- |
| 配布経路 | Play のみ（D3） | Play Console で本人確認と package 名の登録をする。Play の外で配布しないので、Android Developer Console のアカウントと鍵の手動登録は要らない | なし |
| 本人確認 | 個人（D4） | OP-1 の本人確認が開発者確認の本人確認を兼ね、追加の作業は無い | Settings > Developer account で確認済み |
| package 名 | `app.kukuri.android`、Play App Signing（D3） | 2026-09-30 以降、Play の package はすべて登録が必要で、未登録の app は Play から削除される。Play App Signing を使う app は自動登録の対象になる | 「Android developer verification」の画面で `app.kukuri.android` が登録済み。本人確認が済むまで登録状態は通知されない |
| 地域と端末 | 英国・EU 加盟国を除く配布地域（D8）、minSdk 29（D7） | 2026-09-30 からブラジル・インドネシア・シンガポール・タイで、Google Play を含む参加ストアからの install を Android 7 以上の認定端末で検査する。2027 年に世界へ広げる。4 か国は配布地域に含まれ、対象端末はすべて Android 7 以上 | 上と同じ |
| 公開予定日 | 未決 | 2026-10-09 より後のどの日付でも施行の後なので、要件は日付で変わらない | なし |

自動登録されなかったときだけ、Console の案内に従って手動で登録する。

## アプリの作成と配布設定（OP-2）

公式: [アプリの作成](https://support.google.com/googleplay/android-developer/answer/9859152)、[カテゴリ](https://support.google.com/googleplay/android-developer/answer/9859673)、[価格](https://support.google.com/googleplay/android-developer/answer/6334373)、[Play App Signing](https://support.google.com/googleplay/android-developer/answer/9842756)。

1. Home > Create app で、既定の言語を English (United States)、アプリ名を kukuri、種類をアプリ、価格を無料、連絡先メールを `ops@kukuri.app` にし、Developer Program Policies・米国の輸出法・Play App Signing の利用規約に同意する。
2. Main store listing に日本語（ja-JP）の翻訳を足し、アプリ名を両言語とも kukuri にする。文言と素材は #1204 AC-1 が用意する。
3. Store settings でカテゴリをアプリ／ソーシャルにし、連絡先をメール `ops@kukuri.app`、website `https://kukuri.app/` にする。電話番号は空のままにする。
4. App pricing を無料のままにし、アプリ内の商品を作らない。
5. Production の Countries / regions を「配布地域」の表に合わせる。Console の一覧が表と違えば、増えた項目が英国か EU 加盟国に当たるかだけを確かめる。
6. 最初の release には #1199 の upload 鍵で署名した AAB を使う。Play App Signing は Google が生成する鍵の既定のままにする。
7. 「Android developer verification」の画面で package 名が登録済みであることを確かめる。

## 配布地域

公式: [国・地域の指定](https://support.google.com/googleplay/android-developer/answer/7550024)、[配布できる国・地域](https://play.google.com/supported-locations)（2026-09-28 公開の版）、[EU 加盟国](https://european-union.europa.eu/principles-countries-history/eu-countries_en)。

配布できる国・地域をすべて選び（Rest of the world を含む）、次の 28 項目だけを外す。

| D8 の除外対象 | Console の項目 |
| --- | --- |
| 英国 | United Kingdom (GB) |
| EU 加盟 27 か国 | Austria (AT)、Belgium (BE)、Bulgaria (BG)、Croatia (HR)、Cyprus (CY)、Czechia (CZ)、Denmark (DK)、Estonia (EE)、Finland (FI)、France (FR)、Germany (DE)、Greece (GR)、Hungary (HU)、Ireland (IE)、Italy (IT)、Latvia (LV)、Lithuania (LT)、Luxembourg (LU)、Malta (MT)、Netherlands (NL)、Poland (PL)、Portugal (PT)、Romania (RO)、Slovakia (SK)、Slovenia (SI)、Spain (ES)、Sweden (SE) |

別の項目になっている属領・特別地域は外さない。

| Console の項目 | 理由 |
| --- | --- |
| Gibraltar (GI)、Bermuda (BM)、British Virgin Islands (VG)、Cayman Islands (KY)、Turks & Caicos Islands (TC)、Rest of the world に含まれる Jersey (JE)・Guernsey (GG)・Isle of Man (IM)・Anguilla (AI)・Montserrat (MS)・Falkland Islands (FK) など | D8 の英国は GB の 1 項目。[王室属領は英国の一部ではない](https://www.gov.uk/government/publications/crown-dependencies-jersey-guernsey-and-the-isle-of-man) |
| Aruba (AW)、Rest of the world に含まれる Curaçao (CW)・Saba (BQ)・French Southern Territories (TF) | [EU 加盟国に結び付く海外の国・領域は EU の領域に含まれない](https://international-partnerships.ec.europa.eu/countries/overseas-countries-and-territories_en) |

EU の領域に含まれる[最遠隔地域](https://eur-lex.europa.eu/EN/legal-content/glossary/outermost-regions.html)（9 地域）は Play の一覧に別の項目が無い。EU に加盟していない欧州の国（Norway、Switzerland など）は外さない。

- 国の指定は利用者の Play の国（アカウントの登録国）で決まり、現在地では決まらない。アプリ・desktop・P2P・CN の地域制限や、サービス全体の法的な適用除外を意味しない。
- closed testing と open testing の国は既定で production と同期する。追加の closed track は国の指定に対応しないので、closed test には既定の closed testing の track を使う。
- internal testing には国の指定が効かず、どの国の tester にも届く。

## 署名証明書の fingerprint の受け渡し

| 証明書 | 確認場所 | 渡し先 |
| --- | --- | --- |
| upload 鍵の証明書 | #1199 が作る keystore と、Console の Play app signing の画面の Upload key certificate | #1199 AC-1: 署名した AAB の証明書の照合（debug 鍵・配布の鍵との取り違えの拒否） |
| 配布の署名鍵（app signing key）の証明書 | Console の Play app signing の画面の App signing key。新しい app は量子耐性のハイブリッド署名に自動で登録され、Android 16 以前の従来の鍵、Android 17 以降の従来の鍵と ML-DSA の鍵の 3 つの証明書になる | #1199 AC-4: Play から install した候補の証明書の照合。#1204 AC-3: 候補の記録 |

- fingerprint（SHA-256）は公開の値で、repository に置いてよい。照合に使う値の置き場所は #1199 AC-1 が決め、本書には写さない。
- Google の API（Maps、OAuth、Firebase など）を使わず、App Links も採用しない（#1197）ので、API の提供元への登録と `assetlinks.json` は要らない。
- upload 鍵を失ったか漏れたときは、Console の Play app signing の画面から upload 鍵の再設定を依頼する。

## versionCode

Play へ出す AAB の versionCode は、その候補の release tag（`vX.Y.Z-preview.N`）から 1 つの規則で作る（#1199 AC-2）。versionName は app の版（`tauri.conf.json` の `version`）のままで、Tauri が設定する。

| 項目 | 規則 |
| --- | --- |
| 値 | `(major × 1,000,000 ＋ minor × 1,000 ＋ patch) × 100 ＋ 段階`。段階は preview 番号 N（1〜98）で、99 は将来の正式版（`vX.Y.Z`）に取っておく。例: `v0.4.3-preview.2` は 400302 |
| 生成 | `cargo xtask release-check <tag>` が版と tag を照合した後に計算し、`android_version_code=` に出す（`xtask/src/release.rs`）。major 21 以上、minor・patch 1000 以上、preview 番号 0・99 以上・0 始まりの tag は、別の候補と同じ値や逆の順になるので拒否する |
| build への渡し方 | `tauri android build` に `--config '{"bundle":{"android":{"versionCode":<値>}}}'` を付ける。Tauri はこれを `gen/android/app/tauri.properties` に書き、Gradle が読む。付けない build は Tauri の既定（0.4.3 なら 4003。同じ版の候補どうしが同じ値）になるので Play へ出さない |

- 同じ版で別の候補を出すとき（失敗した release のやり直し、作り直し、統合 branch の新しい候補）は、次の preview 番号の tag を切る。同じ tag の再 build は同じ値で、upload 済みの値の別の build は Play が拒否する。
- 使った値は release tag そのもので、別に記録しない。tag は git と GitHub が重複を拒否し、版は上がる一方なので、tag を切った順に値が増える。
- track ごとに別の値を作らない。同じ AAB を internal → closed → production へ昇格し、新しい build は新しい tag にする。
- 統合 branch（`integration/android-1193`）の開発中の候補も、その commit の版で未使用の preview 番号の tag から作る。tag の push では desktop の `Kukuri Release` も動いて draft ができるが、統合 branch の draft は公開しない。署名と CI の lane は #1199 AC-1。
- 同じ版の preview 番号が 98 に達したら版を上げる（これまでの最大は 4）。

## closed test から production access まで

公式: [個人アカウントの試験の条件](https://support.google.com/googleplay/android-developer/answer/14151465)、[試験の設定](https://support.google.com/googleplay/android-developer/answer/9845334)。実施と記録は #1204 AC-4 が所有する。

1. internal test（任意）: tester は最大 100 人。アプリの設定が終わる前から使える。最初の upload で package 名が固定される。
2. Dashboard の設定（本書の OP-2、#1203 の App content、#1204 の掲載）を終える。closed test はその後に始められる。
3. 既定の closed testing の track で、tester をメールの一覧か Google Groups で指定し、feedback の窓口（URL かメール）を opt-in の画面に出す。
4. 12 人以上の tester が連続 14 日以上 opt-in を続ける。途中で抜けた tester は、抜ける前の日数を数えない。
5. Dashboard の Apply for production から、closed test・アプリ・公開の準備の 3 つの節に答えて申請する。審査は通常 7 日以内に終わる。
6. 承認されると production と open testing が使える。production の release、審査、公開、公開後の確認は #1204 が本書へ足す手順に従う。

## 未決の値

| 値 | 決める担当と時点 |
| --- | --- |
| 公開予定日 | #1204 の公開の工程。開発者確認の要件は日付で変わらない |
| closed test の feedback の窓口 | #1204 AC-4 の closed test の開始時 |
