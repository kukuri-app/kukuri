# ADR 0049: Linux GUI配布とCLIのローカル制御経路

## Status

Accepted

2026-09-05改訂: CLIの責務を1入力からの重複実行防止に限定するユーザー決定により、
入力間の重複排除、永続的な冪等性台帳、復元後の時刻による実行制限を撤回した。
改訂前の実装・検証記録は `docs/progress/2026-09-05-issue-887-cli-protocol-v1.md` に保持する。
この改訂の実装・検証は #888 で行う。仕様の改訂だけを実装完了とは扱わない。

## Context

Issue #885は、Linux x86_64 GUIをAppImageとして配布し、GUIとは別profileを所有する
単一所有の常駐プロセスと薄いCLIを介してKukuriクライアントを操作できるようにする。

この追加は既存の投稿、DM、private channel、Live、Game、Metaverse／Dome、Community Nodeの
canonical sourceを変更しない。一方で、CLI profileの購読期待状態、ローカルIPC、command登録簿、
要求単位の実行状態、複数platformの配布成果物という新しいデータ境界を持つため、ADR 0002に
従って実装前に分類する。

## Feature Data Classification

### Linux GUI／CLI配布成果物

- Feature 名: Linux GUI／CLI Preview Release成果物
- Durable / Transient: Durable。公開したPreview Release単位で保持する
- Canonical Source: 配布対象tag／source commitと、そのcommitから検証・署名・集約した同一実行のGitHub Preview Release成果物一式。`latest-preview.json`は公開した成果物のversion、target、URL、signatureを参照する
- Replicated?: Kukuri protocol上はNo。GitHub Release／CDN上の配布copyはapplication replicaとして扱わない
- Rebuildable From: 固定source commit、固定したtoolchain／runner、成果物一覧、署名鍵から論理的に再生成可能。ただしtimestampや署名を含むbyte単位の再現性は完了条件にしない
- Public Replica / Private Replica / Local Only: 公開配布物。Kukuriのpublic／private replicaは増やさない
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要
- 必須 contract: x86_64 AppImage、x86_64／aarch64 CLI archive、checksum、必要な署名、Windows成果物、platform manifestを同一sourceから完全に生成し、一部欠落時はmanifest／Releaseを公開しない。署名用secretを成果物、cache、log、untrusted eventへ渡さない
- 必須 scenario: Ubuntu 22.04／Debian 12における配布済みAppImageの実環境smoke test、x86_64／aarch64 CLI smoke test、Windows回帰、不正なupdater署名、成果物欠落、取得失敗を含む検証環境での更新

### Deb配布と権限承認付き更新（#905）

- Feature 名: Linux amd64 Debと形式別自動更新（2026-09-07承認）
- Durable / Transient: Deb・署名・manifest・収録物報告は公開Release単位のDurable。確認／取得sessionと検証済みbytes、昇格結果はprocess内のTransient
- Canonical Source: 固定sourceの署名済みDeb、backendが確認した形式別manifest、実package状態はdpkg database。WebViewからURL・鍵・bytes・installer引数を指定させない
- Replicated?: No。公開CDN copyはKukuri replicaではない
- Rebuildable From: 公開資材は固定source／toolchainから再生成。更新sessionは再確認・再取得し、未完了の権限承認をrestart後に自動再実行しない
- Public Replica / Private Replica / Local Only: 配布資材は公開、更新session／昇格処理はLocal Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要。GUIのidentifier・profile・鍵・DBと保存形式は変更しない
- 必須 contract: AppImageとDebを同sourceで併産し、`.deb.sig`を既存Tauri公開鍵で検証する。Debは`linux-x86_64-deb`だけを使用し、entry欠落時のAppImage fallbackを拒否する。未検証bytesはinstallしない。明示適用後のpolkit昇格は1回だけとし、取消・拒否・agent不在で別認証や自動retryへ進まない。OS passwordを独自UI・IPC・logへ扱わない。検証済みbytesは変更不能なsealed memfdでroot dpkgへ渡し、GUIをrootで起動しない。適用・状態照合成功後だけhostを停止し通常userとして再起動する
- 必須 scenario: Ubuntu 22.04 CIのinstall／reinstall／remove、手持ちUbuntu 24.04.4 LTSの旧Deb→新版更新／非root再起動／同じprofile保持と認証取消。不正署名・別形式・package失敗は隔離fixtureで確認する。任意のdpkg部分失敗の自動rollbackは保証せず、失敗表示・実状態確認・手動回復を案内する

DebにCLIは同梱しない。Deb payloadはfirst-party ELF、desktop／iconとnoticeだけとし、shared system librariesはDependsで解決する。実Debの全payload inventoryとsourceを結び付け、AppImage runtimeや同梱Ubuntu librariesの資料をDebの対応sourceの根拠としない。実装・検証中の状態は[#905作業記録](../progress/2026-09-07-issue-905-linux-deb-updater.md)、公開判定は#890が所有する。

### CLI専用profileと購読期待状態

- Feature 名: CLI専用クライアントprofile
- Durable / Transient: Durable。所有lock、process／session、socketはTransient
- Canonical Source: GUIと分離したprofile directory内のaccount registry、identity backend、既存featureごとのcanonical store、およびprofile内の購読期待状態
- Replicated?: 既存製品データだけが各featureの既存契約に従ってreplicateされる。profile管理情報と所有状態はNo
- Rebuildable From: 既存製品データは各featureのcanonical sourceから復元可能。identityと購読期待状態はそれぞれのローカル永続状態がなければ再構築しない
- Public Replica / Private Replica / Local Only: 制御情報はLocal Only。製品データの区分は既存featureの分類を維持する
- Gossip Hint 必要有無: 制御情報には不要。購読後の製品データは既存契約を維持する
- Blob 必要有無: 制御情報には不要。製品データは既存契約を維持する
- SQLite projection 必要有無: 既存profile DBを維持する。購読期待状態の具体的な保存形式は子Issueで固定するが、別profileやglobal設定へ暗黙適用しない
- 必須 contract: GUI profileとのpath／identity非共有、一profile一所有者、同profileの二重起動拒否、別profile同時起動、restart後のidentity／購読期待状態復元、同意前network I/O 0件
- 必須 scenario: 新規profile、同意待ち、通常起動、restart、signal、同一profileの競合、別profileの同時実行、Secret Service不在

### ローカルsocketの要求／event stream

- Feature 名: 常駐プロセスのローカル制御protocol
- Durable / Transient: Transient。request／response／NDJSON event stream自体は保存しない
- Canonical Source: 常駐プロセスの版管理されたcommand登録簿、schema、dispatcher metadata。CLI parserや表示文言を正本にしない
- Replicated?: No
- Rebuildable From: command登録簿とruntime eventから再生成する
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要。大容量mediaはprotocolへbase64で埋め込まず、明示したローカルfile／blob参照を返す
- SQLite projection 必要有無: 不要
- 必須 contract: Unix socketのみ、runtime directory 0700、socket 0600、接続元UID一致、TCP listen 0件、版管理されたJSON／NDJSON、stdout／stderr分離、型付きerror、容量／timeout／backpressure上限、remote contentと制御情報の分離。v1は通常frameとsecret frameを各1 MiB以下、既定timeoutを30秒、指定可能な上限を5分、同時接続を64件以下とする。timeoutは接続、frame入出力、dispatcher、event stream全体へ適用する。stream継続はresponseの`more`で示す。secret frameはrequest headerのschema／profile／version／guardと出力先宣言を検証した後、相関付きの受信許可を返してからだけ送受信する
- 必須 scenario: 正常／不正なenvelope、protocol不一致、常駐プロセス不在、timeout／SIGINT、低速な購読側、上限超過入力、接続元UID不一致、secret／untrusted contentの漏えい検査

### command登録簿と操作安全情報

- Feature 名: CLI command登録簿と操作安全情報
- Durable / Transient: 実行ファイルに含まれる再生成可能な定義。要求単位の検証結果はTransient
- Canonical Source: 常駐プロセスのcommand登録簿とdispatcher metadata。CLI parserや呼び出し元の状態を正本にしない
- Replicated?: No
- Rebuildable From: sourceとschema生成処理から再生成する
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要
- 必須 contract: read／write／destructive／secret-bearing metadataをdispatcherと同じ定義から生成する。操作可否は既存のaccount、同意、audience、credential、domain authorizationで判定する。command schemaの検証subsetは`type`、`properties`、`required`、`additionalProperties`、`items`、`enum`、`const`、数値／文字列／配列のmin/max制約とannotationに限定し、未対応keywordは登録時に拒否する。request payloadの契約に合わせ、input schemaのroot `type`は未指定または`object`に限定する
- 必須 scenario: command登録簿とdispatcherの不一致、既存domain authorizationによる許可／拒否、argv／payload／remote contentを制御情報として誤解釈しないこと

### 要求単位の実行状態

- Feature 名: CLI要求の実行と終了処理
- Durable / Transient: Transient。要求と実行中taskの対応はメモリ内だけで保持する
- Canonical Source: 常駐プロセスが所有する要求単位のtask。製品データの正本は既存domainのstoreとする
- Replicated?: No
- Rebuildable From: No。終了した要求をrestartやbackupから再生成・自動再実行しない
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要。入力間の重複排除のための台帳・payload digest・結果cacheを作成しない
- 必須 contract: 1回の入力から受け付けた1要求について、CLI／dispatcherはcommand handlerを最大1回だけ起動する。timeout、応答喪失、切断、再接続、restartを理由に同じ要求を自動再実行しない。別々に入力された要求は内容やrequest IDの一致でまとめず、それぞれ既存domainの規則に従って処理する。request IDは応答との対応付けにだけ使用する。変更操作の排他と容量制限を維持し、切断によって進行中のrollback／cleanupを破棄しない。成否不明は成否不明として返し、未実行や取消成功と断定しない
- 必須 scenario: 1入力1回のhandler起動、同内容の2入力を別々に処理、拒否時handler起動0回、timeout／切断時の再起動0回と後処理完了、正常shutdown、restart後の自動再実行0回、復元直後の明示的な新規操作

### CLI protocolの改訂境界

- CLIは未リリースのため後方互換は不要とする。#888で現行protocol v1の定義を直接修正し、この変更のための版更新・旧版処理・移行層は追加しない。
- Secret出力の有無と変更種別は独立に定義する。招待exportのように既存domainで鍵世代を更新する操作は変更操作とし、秘密値を永続的な結果cacheへ保存せず、専用frameでのみ返す。切断後も変更操作の後処理を完了する。
- `--idempotency-key`、requestの `idempotency_key`、metadataの `idempotency_required` を定義・実装・schema・testsから撤去する。
- 通常frame／secret frame、Unix socket、入力サイズ・timeout・接続数の上限、既存domainの署名／wire／認可規則はこの変更で拡張しない。

## Decision

- GUIとCLIはprofile／identityを共有しない。GUI processと常駐processが同じprofileを同時所有する方式は採用しない。
- 常駐プロセスだけがprofile内のDesktopRuntimeを所有し、薄いCLIはローカルUnix socket越しにrequestを送る。公開TCP制御APIは提供しない。
- app／Community Node同意、age gate、restore gateを共通hostで共有し、成立前はruntime、scheduler、remote取得、background通知を開始しない。
- protocolの正本は常駐プロセスのcommand登録簿／dispatcherとし、schema／command metadataを同じ定義から生成する。
- account、同意、private audience、credentialなど既存の製品側guardは、GUIと同じ意味で常駐プロセス側にも適用する。
- Tauri updater署名は必須とする。2026-09-05の#889計画承認により、AppImage埋込みGPG署名は追加しない。Linux AppImageもTauri updaterの公開鍵による検証を行い、署名なし・不正署名の更新はインストールしない。
- Ubuntu 22.04をLinux GUI build基盤とし、Ubuntu 22.04／Debian 12のX11／XWaylandを実環境smoke test対象にする。

2026-09-06の#889検証範囲変更承認により、今回用意できない追加OS環境とXWaylandの実行は延期する。既存Ubuntu 24.04とWindowsでの確認範囲を記録し、延期環境の動作保証や検証済みという主張はしない。Ubuntu 22.04でのbuild基盤は維持する。詳細は [#889作業記録](../progress/2026-09-05-issue-889-linux-appimage.md) に置く。

2026-09-07の#889 Scope revision v5では、ユーザー承認により追加の網羅的な手動実機確認を実行ゲートから外す。代表実機の成功証跡を再利用し、今回の変更と影響先の不足は自動tests・隔離サービス・実OS資源で検証する。署名・実行file置換・保存状態・process停止をmockだけで成功とせず、既存の安全契約、CI、独立監査を維持する。未変更の全device／GPU／codec・OS連携の手動確認やComputer Useを必須にしない。追加手動確認は、固定条件に関わる具体的な問題を既存証跡と自動検証では判定できない場合に限る。未確認機能・延期環境の動作保証は追加しない。以下の実desktop確認の記述も、成功済み代表証跡の採用とこの追加条件に従う。

## Linux GUIの終了と更新

- main window の close 動作は端末ローカルの `null` / `quit` / `tray` 設定で決める。未設定または破損値は `null` として、windowを表示したまま「終了／タスクトレイへ格納／キャンセル」を確認する。確認時の「以降同じ質問をしない」を選んだ場合だけ、選択した `quit` / `tray` を保存する。Control Centerのシステム設定では `null` を「毎回確認する」としていつでも再選択できる。
- close確認要求はprocess内で上限1件の保留状態とし、要求IDが一致する応答だけを一度適用する。frontendの購読前に発生した要求は読み戻せるようにする。設定保存に失敗した場合はclose副作用を開始せず、同じ確認から再試行できる状態を維持する。
- トレイのオブジェクト生成成功だけでclose-to-trayを有効にしない。Linuxでは表示先と当該processの登録を確認し、利用不能・確認不能なら正常終了へ進む。非表示中に表示先を失った場合はwindowへ復帰させる。
- Quit、ウィンドウ終了、SIGTERM／SIGINT／SIGHUP、更新後の再起動はGUIの終了処理へ集約する。終了要求後は新しいアプリ操作を受け付けず、既存の起動・復元・アカウント切替の排他処理を待ってから現在のhostを停止する。進行中バックアップの取消入口は維持する。
- Linux AppImageの更新は署名検証済みのdownload結果だけをinstallし、成功後にhost停止と再起動を要求する。install失敗時は再起動せず、失敗を表示する。GUIの再起動はCLI／daemonの操作として公開しない。
- downloadと署名検証後の再起動待ちは、定期／手動checkや重複downloadで破棄しない。「あとで」は案内だけを閉じ、検証済み更新と明示適用の操作を維持する。asset取得の404はfile欠落として案内し、manifest取得失敗や接続障害と区別する。
- 終了要求・終了完了・close確認の保留要求・トレイ確認結果はprocess内だけのTransient／Local Only状態である。close設定だけをapp data directoryのJSONへDurable／Local Onlyとして原子的に保存する。SQLite、backup、gossip、peerへの複製対象にせず、既存のprofile・同意・鍵の分類と保存形式は変更しない。
- 二重起動の受付ログにargvやdeep-link本文を記録しない。

### Feature Data Classification
- Feature 名: main window close preference
- Durable / Transient: 選択済み設定はDurable。確認要求、要求ID、終了処理、tray可用性はTransient
- Canonical Source: app data directoryの`window-close-preference.json`。process内ではTauri `WindowCloseState`が読み込み済みsnapshotを所有
- Replicated?: No
- Rebuildable From: 設定file欠落・破損時は`null`（毎回確認）へ安全に戻る。利用者の過去選択自体は再構築しない
- Public Replica / Private Replica / Local Only: Local Only
- Gossip Hint 必要有無: 不要
- Blob 必要有無: 不要
- SQLite projection 必要有無: 不要
- 必須 contract: `null`で副作用前に確認、記憶指定時だけ保存、保存失敗時はhide／shutdownなし、`quit`は正常終了、`tray`は利用可能時だけhide、要求IDの単発consume
- 必須 scenario: Tauri unit／IPC testとfrontend component test。外部peerを使うscenarioは不要

## LinuxのOS通知利用可否

- 通知の権限状態と通知サービスの稼働を混同しない。Linuxではsession D-Busの `org.freedesktop.Notifications.GetServerInformation` を期限付きで照会し、接続できた場合は `available`、サービス不在・接続拒否・不正応答・期限超過は `unavailable` とする。OS側の表示設定や拒否状態を、この照会だけで `granted`／`denied` と推定しない。
- 利用可否の照会・再確認は通知本文を送信せず、OS設定・アプリの通知設定・本文previewを変更しない。Windowsの既存権限結果と明示操作での有効化経路は維持する。
- 照会結果はprocess内のTransient／Local Only状態。新しい永続保存・peer送信は追加せず、既存診断への状態表示だけを許す。サービス情報の生の値やD-Busエラー本文を診断へ追加しない。
- Linuxの通知表示には既存の本文保護・失敗通知を維持し、`silent` は標準の `suppress-sound` hintへ反映する。実表示・音・クリック動作は実desktopで別に確認する。

## Windows OS通知からの復帰

- #889の2026-09-06追加承認により、NSIS版の通知はWindowsのprotocol activationを使い、既存の `kukuri:` とsingle-instance／deep-link経路へ戻す。バナーと通知センターで同じ通知対象を開き、メモリ内のToast Activated callbackだけに依存しない。
- activation URIはWindowsの正規化後と一致する `kukuri://notification/?id=<encoded notification ID>`。受信側はこの形式と以前に生成したroot slashなしの形式だけを許可し、他のpathへ一般化しない。そのIDは現在のアカウントの通知一覧に一致する場合だけ既存通知handlerへ渡す。profile／accountの指定や自動切替は許さず、不正URI・未知IDで別データを開かない。既存の同意・復元・停止gateを維持する。
- notification IDはLocal Onlyの参照metadataとしてOSの通知XML／起動引数に渡る。通知本文、秘密鍵、招待token、保存先をURIに含めず、argv／URI本文をログへ出さない。新しいDB、backup項目、peer送信、COMサーバーは追加しない。
- 最後に処理したnotification URIだけをWebViewのsessionStorageへ記録し、同じ起動URIが再mount／account切替時のreloadで再実行されることを防ぐ。この参照はsession内だけのLocal Only状態で、profile backupやpeerへ複製しない。新しいlive clickは同じURIでも処理する。
- NSISは自アプリのStart menu shortcutへAUMIDと通知保持用stub CLSIDを設定する。通知UIの入力欄は提供せず、quiet／preview設定とLinuxの通知経路は変更しない。
- #889のScope revision v4では、OS／アプリ内の共通投稿通知handlerから通知元object_idをThreadのfocusへ渡す。追加pageも同じtopic／threadの既存読取りAPIだけで取得し、実在する対象へ一度スクロールする。明示的な再クリックは新要求、通常refreshは移動要求ではない。要求識別子はsession内のLocal Only状態で、OS通知URI・永続化・外部送信の項目は増やさない。DM個別messageへのjumpや既読化の変更は含めない。

## GUIの外部リンク起動

- Releaseの公開資料・feedbackと通報画面の運用者policy／権利侵害受付リンクは、明示操作でGUI専用 `open_external_url` commandからOSの既定ブラウザーへ渡す。Ready／終了gateを維持し、HTTP(S)の絶対URL・hostあり・credentialなしだけを許可する。file、任意scheme、shell／program指定の起動権限は公開しない。
- LinuxはOpenURI portalで起動し、AppImageのlibrary／GIO環境を外部processへ継承しない。WindowsはShellExecuteを使用する。サービス不在・拒否・期限超過は局所的な失敗表示とし、自動retryや成功の推定はしない。通常のbrowser版anchor、内部navigation、downloadは対象外。
- URLとpending／errorは操作中だけのTransient状態であり、新しいDB／backup／peer送信やログ保存を追加しない。OSへ渡した公開URLの先では外部ブラウザーのcookie・network等の規則が適用される。通報本文、診断本文、identity等をURLへ付加しない。

## バックアップ／復元境界

- CLI profileのidentity、既存製品データ、購読期待状態はprofile backup対象に含める。CLIの操作台帳は作成・更新・移行しない。
- 所有lock、socket、PID、実行中session、bearer token、endpoint secretは移行しない。
- app／Community Node同意とage attestationは既存のbackup／restore契約どおり移行せず、restore後に再設定を要求する。
- 復元は、移行対象のローカル状態をバックアップ生成時点へ戻す。既に他端末やCommunity Nodeへ伝わったデータは巻き戻さない。復元をまたぐ入力間の重複排除は行わず、復元後の経過時間によるCLI操作の保護期間を設けない。必要な明示的再同意等が成立した操作は、待機時間を追加せず実行できる。
- 未リリースCLIの旧台帳・旧backupを救済する互換処理は追加しない。GUIの既存バックアップ契約はこの変更の対象外とする。

## セキュリティ境界

- runtime directory 0700、socket 0600、接続元UID検証は、別OS userからの接続と偶発的なprofile間接続を防ぐ。
- 同じOS userで動くprocessは同じローカル権限を持つ。分離が必要な運用では別OS userと別profileを使用する。

## Consequences

- CLI追加のために既存featureのdocs／blobs／gossip／SQLite、署名canonical、private audience、P2P三経路を変更してはならない。
- 常駐プロセス、protocol、command登録簿、要求単位の実行状態はLocal Onlyの制御経路であり、Community Nodeやpeerへ制御指示を送信しない。
- untrustedなpost／DM本文をcommand、profile、file path、log指示として解釈しない。
- E2Eで使う一時profileはtest harnessが作成・破棄する。
- 共通host抽出では、#855／#857後のrestore／consent／background actionを含む全callerを現行基準から再生成する。

## 参照

- [Tauri AppImage](https://v2.tauri.app/distribute/appimage/)
- [Tauri Updater](https://v2.tauri.app/plugin/updater/)
- [Tauri Linux signing](https://v2.tauri.app/distribute/sign/linux/)
- Issue #885、#886、#887、#888、#889、#890
