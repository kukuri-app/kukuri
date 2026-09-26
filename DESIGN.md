# kukuri UI/UX Design Contract

> kukuri desktop（Tauri + React + Tailwind v4 + shadcn/ui）で、利用者に何をどう見せ、どの品質を守るかを定義する製品固有の設計契約。

## この文書の位置づけ

- 本書は[AGENTS.md](AGENTS.md)の作業原則・設計原則に従うUIの設計契約である。関連する作り方・証跡は[`docs/adr/0014-uiux-dev-flow.md`](docs/adr/0014-uiux-dev-flow.md)を参照し、原則に反する既存規則は維持理由にしない。
- CSSとstateの配置、import順、実装境界は[`docs/architecture/desktop-ui-implementation.md`](docs/architecture/desktop-ui-implementation.md)に置く。
- `apps/desktop/src/styles/tokens.css`は実行値、StorybookのFoundationsは描画確認面である。変更対象の契約について本書・実行値・確認面を照合する。対象外の不一致や将来案を見つけても、自動的に今回の実装へ追加しない。
- 本書は達成すべき結果を定める。文書の更新だけで実装済みとは扱わず、現在の挙動は実装と検証で確認する。提案は明示的に依頼された目的・導入条件・移行範囲を持つ場合だけ記録し、採用前に実装・token・wrapperを先行追加しない。
- 作業前に対象surface・主操作・入力/環境・期待結果と終了条件を有限に定める。本書の合意済み契約のうち対象へ適用する条件を選び、未依頼のエッジケースや全状態の直積を追加しない。必要な操作・状態継続・Accessibility・データ境界を満たす候補の中で、完成後の実装・テスト・補助コードの総量を最小にする。変更差分の小ささは優先しない。

## 1. 利用者、利用文脈、画面目的

kukuriの通常画面は、コンテンツを継続して閲覧・作成・操作する高密度なOperate型UIとする。マーケティング用landing pageの表現を通常画面へ持ち込まない。

| 利用者／文脈 | 主な目的 | UIが優先すること |
|---|---|---|
| 閲覧者 | topic、post、thread、profileを追う | 内容、投稿者、scope、取得状態を理解できること |
| 投稿者 | 投稿、返信、DM、channel内操作を完了する | 投稿先、draft、pending、成功、失敗、再試行を失わないこと |
| Stream／Metaverse参加者 | live sessionやDomeへ参加し操作する | resource状態、input ownership、退出／復帰、縮退状態を明示すること |
| 設定利用者 | account、community node、appearanceを管理する | 変更結果、同意、認証、再起動要否、回復方法を明示すること |
| 開発者／運用者 | diagnosticsを調査する | Developer modeまたは診断面に技術情報を集約し、通常導線を妨げないこと |

各surfaceは単一の主目的を持つ。

「見つける」の検索・発見・おすすめの切替は、本文文字サイズを維持した高さ2rem・全丸のコンパクトなボタンで1行に配置する。選択状態を維持し、画面幅に合わせて余白を調整する。切替だけで検索実行の契約を変えない。

| Surface | 単一目的 | 主な操作 |
|---|---|---|
| Timeline／Bookmarks | 選択scopeの投稿または保存済み投稿を読む | 閲覧、投稿、返信、保存、scope切替 |
| Thread | 1つの会話文脈を追う | 返信、親投稿へ戻る、投稿者を開く |
| Profile | 1人の公開情報と投稿を理解する | 関係操作、投稿閲覧、自分の編集へ進む |
| Notifications | 自分に関係する変化を処理する | 対象文脈を開く、既読状態を理解する |
| Messages／Conversation | 相手を選び、privateな会話を継続する | 会話選択、送信、再試行 |
| Explore | topicや参加先を発見する | 検索、選択、参加、対象Columnを開く |
| Stream | live sessionを視聴・操作する | 再生、参加、fullscreen、退出 |
| Metaverse | Domeへ入り、空間を操作する | entry、focus取得、移動、fullscreen、退出 |
| Control Center／Settings | workspaceとapp設定を管理する | Column操作、認証、同意、設定変更、診断 |

## 2. 情報階層とkukuri固有性

- topic、post、thread、profile、channel、conversationを主役にする。node URL、peer id、ticket、hash、capability、sync内部状態は通常表示の主階層へ置かない。
- 技術識別子は通常画面では人が判別できる名前と短い補助情報に置き換える。完全値はcontext menu等の明示操作でコピー可能にし、Developer modeと診断画面では表示してよい。
- product UIとdiagnostics UIを視覚的・構造的に分け、diagnosticsはControl Center、Settings、inline Noticeの補助階層へ置く。
- 開発者設定ではモードの有効／無効を文字で示し、有効時は接続・ディスカバリー・コミュニティノードの診断へ同じ設定drawer内で移動できるようにする。有効化だけで自動遷移せず、移動後は対象sectionへfocusを引き継ぐ。設定drawerとbackdropはworkspaceのdock／page indicatorより前面に置き、診断操作を妨げたり背景の操作を受け付けたりしない。
- darkはNeutral / Teal（Issue #1001）とし、背景`#121212`、Column本文・パネル`#292929`、本文`#ffffff`を基盤とする。focus・selected・activeと通知の局所的アクセントに`#03dac5`を使い、その塗り上の文字は`#00332e`とする。primary塗りボタンは`#d77d45`と文字`#20160e`を使う（Issue #1003）。補助文字は`#b3b3b3`、装飾境界と操作識別境界は別tokenにする。lightはGraphite / Orange（Issue #996）の白・中立グレー・warm-orangeを維持する。大面積の有彩色や色付き光彩を追加せず、Column Canvasとtopic-firstの情報構造、dark既定と明示的なtheme選択を維持する。
- 装飾用の弱い境界と操作識別用の強い境界を分ける。成功・警告・エラー・接続状態は意味色とlabel／iconを維持し、ブランド色に一括置換しない。
- 半透明gradient、過剰なcard nesting、装飾目的の巨大見出しで階層を作らず、solid surface、境界、余白、弱い拡散影で表す。
- 表示の選択は合意したbriefとtoken・component契約に基づく。外部trendや一般論だけで要件を追加せず、既存ADRも現在の作業原則・設計原則に照合する。

## 3. Column文脈と状態継続

Columnの外底面とCanvasの横スクロールバーの間には`--space-sm`の余白を置く。余白はCanvasの高さ内に含め、最終投稿・composer・横scrollの操作を妨げない。狭幅では固定の操作群も同じ余白を考慮し、投稿ボタンと下辺を揃える。

- Columnのscope、target、active、focus、pin、preferred span、親子関係を別のstateとして扱う。activeとDOM focus、partially visibleを同一視しない。
- live session / game roomのmanifestは表示範囲内のカードと開いている詳細だけが取得を要求する。非表示Column・画面外・document非表示では解除する。未取得候補は通常の参加・更新操作を持たない状態として示し、取得後に検証済みカードへ置き換える。候補があるときは一覧全体をemptyと表示しない（ADR 0052、#1262）。
- Column間を移動しても、入力中draft、選択中scope、会話文脈、未保存状態、session内scrollを不用意に失わない。
- 戻る操作は親Columnまたは直前の文脈へ戻し、無関係な既定画面へ飛ばさない。focusは移動元または操作を開始したcontrolへ復元する。
- canonical URLはfocus中Columnの共有targetだけを表す。Column配列、幅、順序、scroll位置、draftをURLへ載せない。
- local layoutとcanonical URLの責務は[`docs/adr/0031-variable-span-column-workspace.md`](docs/adr/0031-variable-span-column-workspace.md)に従う。
- 投稿通知をOS通知またはアプリ内通知から明示的に開くと、通知元の投稿を含むThreadを選択し、その投稿をColumnの表示範囲へ移す。取得待ちは解決後に一度移動し、通常refreshでは閲覧位置を奪わない。同じ通知の再クリックは新しい移動要求として扱い、欠落した投稿の代わりに別投稿をtargetにしない。

## 4. Componentと画面状態

自分のプロフィールは、カラムの初回表示・再open、明示的な更新・再試行、公開投稿やプロフィール・関係の変更に応じて取得する。開いたままのカラム間の選択変更だけでは再取得しない。取得成功を投稿件数と分けて保持し、0件も取得済みとして扱う。再取得中は概要・件数・投稿または0件表示を保持し、本文へloading行を追加しない。失敗でも直前の確定値と未保存の編集内容を保持する。

自分と他ユーザーのプロフィールの公開投稿は、表示窓の終端へ近づいた需要に応じてcursorから次ページを読む。1回の取得回数・候補数・bytes・待機時間と保持窓の上限を実装前に定める。表示可能な行が0件でも続きのcursorがあれば終端と断定しないが、投稿が出るまで自動読込みを繰り返さない。上限で止まった場合は部分取得としてcursorと現在の表示を保ち、次の閲覧要求や明示的な再試行から再開する。失敗時は自動再試行を止める。refreshは現在の表示窓を差分更新し、先頭から閲覧位置までの全履歴の連続性確認・再取得を前提にしない。

自分のプロフィールカラムのヘッダーに更新アイコンボタンを置く。取得開始から最低1秒は同じ寸法のままアイコンを回転させ、処理が1秒を超える場合は完了まで続ける。データは取得でき次第反映し、最低表示時間で反映を遅らせない。更新中のaccessible nameとbusy状態を示し、表示中・保存中の重複操作を抑止する。非選択カラムの更新で選択・route・scrollを変更しない。reduced motionでは連続回転を止め、静止アイコンと更新中の状態名を最低1秒表示して伝える。

設定「リリースと更新」の確認ボタンにも同じ最低1秒の規則を適用する。確認開始から最低1秒は「確認中」のaccessible nameとbusy・無効状態を保ち、1秒を超える確認は完了まで続ける。結果（最新／更新あり／失敗）はstoreの状態が確定し次第反映し、最低表示時間で遅らせない。結果には、その確認が完了した時刻を時:分:秒で添え、結果が前回と同じでも確認ごとに更新する。時刻の表示だけを目的に自動scrollやfocus移動を追加しない。

プロフィール概要のアバター右隣は表示名とユーザー名の2段とし、概要内の固定見出し「プロフィール」は置かない。カラムヘッダーの名称は維持する。表示名は既存の表示名→ユーザー名→不明なユーザーのfallbackを使い、未設定のユーザー名は未設定であることを示す。長い名前は折り返してカラム内に収める。狭幅・200% zoomでもアバターと名前のまとまり・編集ボタンは同じ行に保ち、名前の領域を可変幅にする。アバターを含む概要ヘッダー行の下端には既存余白に加えて4 CSS pxを一度だけ加え、アバターの3rem・全丸形状を維持する。

### 4.1 Component状態

投稿の返信先は、直前の返信対象のアバター・名前・最大2行の本文を、返信カードの外側かつ前に簡略表示する。簡略表示から接続線を伸ばし、今回の投稿ヘッダー・本文・操作へ続ける。親本文からスレッドを開く操作を維持し、keyboard focus中は省略を解除して内部参照のfocusを隠さない。スレッドrootを直前の親の代用にしない。本文なし添付は短い添付表示とし、簡略行でmediaを再生しない。取得不能・表示制限とThread内の親表示抑制を維持する。

Threadの返信ツリーは深さに応じて字下げし、各カードの幅はカラム本文幅と22remの小さい方を下限とする。深い返信で下限を下回る場合はカードを縮めず、カラム本文を横スクロールして表示する。カラム幅を広げた場合は、収まる深さまで横スクロールなしで表示する。

投稿カード右上には端末timezoneの年月日と時分秒を常時表示する。日付順は選択localeに従い、日本語は年月日順とする。狭幅では折り返しを許容し、投稿者・公開範囲・日時を重ねたり切り捨てたりしない。

投稿カード右上のフォロー関係と公開範囲は文字chipではなくiconで示し、意味はローカライズ済みのaccessible nameとhover tooltipで伝える。相互フォローは`users-round`、フォロー・フォロワーは`user-round-arrow-left`、フォローを介したつながりは`user-round-group`、公開は`book-open`、プライベートチャンネルは`book-lock`とし、tooltipはチャンネル名が分かれば「プライベート: <チャンネル名>」とする。状態表示であり操作ではないためtab移動の対象にしない（[#1345](https://github.com/kukuri-app/kukuri/issues/1345)）。

投稿・DMの画像／動画と画像viewerは、自動取得が固定回数で失敗したら、その部分だけを「取得に失敗しました」と再取得のicon buttonへ置き換える。投稿本文、scroll位置、focus、表示制限による代替表示は変えず、表示制限中は失敗表示も再取得操作も出さない。icon buttonはaccessible nameを持ち、再取得中は同じ位置に残したままbusyを示して重複操作を受け付けない。reduced motionではiconの回転を止め、状態名で伝える。avatar・カスタムリアクションは既存のfallbackを使い、再取得操作を置かない。回数とリセット条件は`docs/architecture/blob-cache.md`に従う。

投稿本文と返信先previewの本文blobも、取得できないときは該当部分を同じ失敗表示と再読み込みicon buttonへ置き換える。投稿カードの操作群の一番右には「投稿を再読み込み」icon buttonを常時置き、そのカードで欠けている本文・直前の返信先本文・添付だけを1回再試行する。投稿単位の操作でtopic全体を同期せず、取得済み内容、scroll、focus、draftを保持する。再読み込み中は同じ位置でbusyを示し、対象カードの操作を重複実行しない。

返信先previewの本文が欠けた表示中の投稿は、有限backoffで局所再取得する。Bookmarksは20件単位のページを置き換えて表示し、ページ移動で過去のカードを残さない。これにより自動再取得の監視とtimerは表示ページの上限内に収める。

投稿本文の資格情報を含まない絶対HTTP(S) URLは、全文を折り返せるlinkとして表示する。公開・表示可能・settledな投稿がviewport内にある場合は、primary contentの先頭URL 1件だけにsite、title、任意description／imageのpreview cardを表示できる。取得中は本文を押し下げるskeletonを置かず、失敗時はinline linkだけを維持する。private channel／DM、Composer参照preview、adult-content gate／trust collapse中、withdrawn／missing／local pending、viewport外では自動取得しない。cardとinline linkはpointer／keyboardから元URLをOS browserへ開き、親のthread操作を重複発火しない。remote HTML／imageをWebViewから直接読み込まず、送信・取得境界はADR 0051に従う。

#### 自分のアカウント操作（#1005）

Control Center左隣の丸いアバターボタンからアカウントメニューを開く。最上部は「プロフィール表示」、中段は管理対象accountのアバター・表示名・ユーザー名、最下部は「アカウント追加」「アカウント管理」「ログアウト」。本人のプロフィールカラムがあれば画面とkeyboard focusを合わせ、なければ追加する。account行の選択は確認を挟まず切り替え、処理中と失敗を明示する。

Control Center表示中は背景のアバターを含む操作群を非表示にし、pointer・Tabで操作できないようにする。センターを閉じた後に再表示し、Escapeまたは閉じるボタンではセンターのtriggerへfocusを復元する。設定等への明示遷移は遷移先のfocusを優先する。

logout確認はローカルデータ保持と同じ鍵のimportによる再ログインを説明し、「はい」「キャンセル」を置く。初期focusはキャンセル。成功後は直前の登録accountへ戻り、他の登録がない場合だけ新規鍵を生成する。新規accountの初回プロフィールDialogはCN同意完了または明示skipの後に開き、他Dialogと重ねない。保存操作は本文のscrollから分離する。「あとで」はsession内の自動再表示を抑止し、未完了なら次回起動で再案内する。完了状態はaccount単位で保持する。

「アカウント追加」Dialogには既存鍵のimportに加え「新しいアカウントを作成」を置く。新規作成は既存accountを残して作成・切替し、必要な初回CN同意／skipの後にプロフィール設定へ進む。処理中は重複操作を無効化し、失敗時は同じ操作を再試行できる。

menu表示のために非active accountの通信を起動しない。local profile/imageの欠落は既存fallbackと取得不能表示で扱い、取得失敗を空一覧としない。

interactive componentは、該当する`default`、`hover`、`focus-visible`、`pressed`、`selected`、`disabled`、`pending`、`error`を定義する。

- 状態を色だけで区別しない。文字、icon、境界、形、accessible stateのいずれかを併用する。
- `disabled`と`pending`を混同しない。pendingは処理中であることと重複操作の扱いを伝える。
- errorは原因の要約と次に可能な行動を持つ。成功通知は実際に確定した操作名と一致させる。

### 4.2 画面状態

次の表は状態の意味を定める。対象操作から到達する合意済みの状態だけを実装・検証し、すべてを各componentへ追加しない。loading・reconnectingの終了は対象要求の期限で判定し、全履歴や全peerの処理完了を待たない。

| 状態 | 表示契約 | 終端／回復 |
|---|---|---|
| initial loading | 初回取得中であることと対象を示す | success、empty、partial、offline、errorのいずれかへ必ず移る |
| refreshing | 直前の有効値を保持し、更新中を補助表示する | 新しい値または既存値を維持したerrorへ移る |
| empty | 指定した条件・取得窓の成功結果が0件の場合だけ表示する。未取得の続きがある場合はpartialとして示す | 作成、参加、条件変更等の次の行動を示す |
| partial | 取得済み範囲と不足範囲を区別する | 追加取得、再試行、現状利用の選択肢を示す |
| offline | localで利用可能な値を保持し、network不在を示す | 再接続待ちまたは明示再試行を示す |
| reconnecting | 直前の値を保持し、接続回復中を示す | connected、degraded、offlineのいずれかへ移る |
| degraded | 利用できる機能と利用できない機能を分ける | 影響範囲と回復方法を示す |
| permission denied | 拒否された対象と理由の安全な要約を示す | 戻る、権限取得、設定変更のうち可能な行動を示す |
| error | 既存の有効値を消さず、失敗した部分を局所化する | retry、戻る、設定確認等の行動を示す |
| retry | 同じ操作を安全に再実行できることを示す | 重複副作用を起こさずsuccessまたはerrorへ移る |
| success | 完了した対象と結果を示す | 次の通常状態へ戻る |

false empty、無期限skeleton、取得不能な補助面が主要面を占有し続ける状態を禁止する。P2Pアプリでは「peerがいない」「まだ届いていない」「local cacheだけ」「一部取得済み」を0件と同一表示にしない。

一覧（ブックマーク、フォロー中、フォロワー、ミュート中、ブロック中など）の初回取得は、表示できる値がまだない間、loadingの表示開始から最低0.5秒はloadingを保ち、取得がそれより長い場合は完了までloadingを続けてからempty／success／errorへ移る。値が既にある再取得は一覧を消さず、取得完了時に差し替える。上記4節の操作ボタンの最低1秒は結果の反映を遅らせないが、一覧の初回loadingは値の初回表示自体を最低時間まで遅らせる点で異なる（点滅の抑止が目的）。

emptyの案内で実在する操作を指す場合は、その操作のiconとローカライズ済み操作名を持つ非操作のchipで示し、文章だけで説明しない。chipは実ボタンと同じ見た目（pill、secondary面）を使い、focusと操作を持たず、iconは装飾として隠し、操作名は可視文字で持つ。

接続診断では、接続候補・同期補助先の設定、実際のpeer接続、保存データの配送状態を分けて示す。経路やリレー許可だけで接続成功とせず、過去のエラーは現在の状態と区別する。未取得・更新失敗では現状未確認を示し、前回の値があれば保持する。topic診断の欠落を未使用と断定せず、購読・受信停止の既知状態から説明する。要約と次の操作を先に置き、原文・内部コードはDeveloper modeの技術詳細に置く。「診断を更新」は状態取得だけを行い、接続設定・認証・同意・購読を変更しない。別のアプリ操作のエラーは同期状態と分けて残す。

### 4.3 アプリ初回同意

年齢の自己申告が必要で未選択の間は、checkboxと同意ボタンの近くに続行できない理由を常時表示し、accessible descriptionにも結ぶ。無効ボタンへのhoverやclickを説明の唯一の入口にしない。native disabledを維持し、背景・境界・影と理由文で有効状態と区別する。申告済みの文書更新では不要な再チェックや理由を表示しない。

未選択の同意ボタン自身にも鍵アイコンと「年齢確認が必要」相当の状態名を表示し、通常の同意actionと色以外でも区別する。無効時は中立色の背景と破線の境界を使い、primaryの影を残さない。選択後は「同意して続行」、保存中は既存の処理中文字へ切り替える。無効理由をクリックして初めて知らせる構造にはしない。

規約全文を読めるscroll領域と年齢申告・同意・拒否の操作領域を分け、標準の初期画面で主要操作を見える位置に置く。footerは本文やfocusに重ねず、低い高さやzoomではpage scrollへ退避して全文と操作への到達を保つ。保存中は重複操作を防ぎ、失敗時はチェックを保持して明示的に再試行できるようにする。同意・年齢条件や文書の版、runtime開始条件は表示上の都合で変更しない。

言語が未設定ならOSのUI言語、WebViewの言語候補、英語の順で初期言語を決め、最初の同意表示より前に確定する。保存済みの対応言語はOSより優先し、過去の自動決定と明示選択を推測して書き換えない。OS取得の失敗で起動を止めず、遅着した結果で利用者の選択を巻き戻さない。

同意画面のheaderには本文scrollと独立した言語選択を置く。「日本語／English／简体中文」の自称表記と、現在の言語を読めなくても認識できるlabelで選べるようにする。切替は同梱の文書全文と正文／参考訳表示、操作、HTMLの言語へ反映し、それ自体では同意・年齢申告・network開始を行わない。同意保存中は言語を固定し、実際に表示した言語を受諾記録へ渡す。言語の保存失敗ではsession内の表示を維持し、保存の再試行を提示する。

通常画面ではControl Centerの設定入口に言語とテーマを扱うことを示し、「表示と言語」でthemeより先に言語選択を置く。既存の設定section ID、deep link、draft、workspaceと戻る文脈を保持する。

「システム」設定では、main windowを閉じたときの動作を「毎回確認する」「kukuriを終了する」「タスクトレイに格納する」から選べるようにする。未設定時のclose確認は終了とtray格納を同じDialogで明示し、「以降同じ質問をしない」は初期状態を未選択にする。保存を選ばない応答は今回だけ適用し、キャンセル／Escapeではwindowと設定を維持する。保存失敗はhideや終了を開始せず、選択を再試行できる状態で示す。trayを利用できない環境で復帰不能な非表示状態を作らない。

### 4.4 Community Nodeの初回案内と復旧

app規約・年齢申告・復元gateを終え、ローカル同意の集約結果から、どの設定済みNodeにも撤回されていない同意記録がないと確認できる場合は、Nodeが利用者発見・端末接続・必要時の中継を手助けするサーバーであると短いDialogで説明する。「規約を確認する」は設定一覧index 0の既存規約Dialogへ進み、それ自体は同意ではない。既定候補と利用者追加Nodeを区別せず、一覧順を変えない。

この有無の判定は索引での存在確認や既存の差分集約を使い、画面表示のたびに全Nodeを読み直さない。同じ同意状態の正本を二重に持たず、結果が未確認なら全Node未同意と推測せずに既存の同意gateを維持する。

「あとで」/閉じるはそのアカウントの起動session中の自動再表示を抑止し、設定・見つけるから手動で再開できる。説明と規約を重ねず、他の開いているDialogの終了を待つ。同意済みでも接続失敗/機能非提供なら初回説明を繰り返さず、理由別の回復を示す。一覧0件や状態不明を全Node未同意と見なさない。

同意保存、接続準備、検索機能の利用可能を別状態にする。同意・runtime event・状態取得・Node情報取得の完了と検索先の解決を同期し、autoを「明示選択先が利用不可」と表示しない。明示選択先への問い合わせ停止は、検索先を無断変更しないためであると説明し、対象のCommunity Node設定・別Nodeの選択・autoへの復帰を提示する。規約、接続/再試行期限、入場制限、公開Node情報、機能非提供の理由を区別し、検索の空表示はquery成功時に限る。

フィードバックの送信可否は検索の可否と独立して示す。送信先がない場合は、未設定、状態確認中/取得失敗、認証・同意待ち、接続不調、受付非対応を区別し、設定済みノードごとの理由を上限つきの表示窓で示し、既存のコミュニティノード設定への操作を入力欄より前に置く。受付非対応は運営者側の提供機能であり、検索や規約同意だけでは有効にならないことを説明する。設定へ移る際はDialogを閉じ終えてから設定内へfocusを渡し、入力が消えることと設定確認後の再開方法を示す。表示・設定遷移だけで送信・認証・同意を実行しない。

Nodeの規約Dialogでは、文書を見出しだけの折りたたみ一覧として初期表示する。見出しを開閉するボタンは、Enter/Spaceで操作でき、展開状態をスクリーンリーダーへ伝える。見出し行には必須/任意、更新、版、同意状況を常時表示し、折りたたんだ文書も一覧中の全文書が同意対象であることを示す。版が上がった更新は旧版と現行版を、版が同じまま内容だけ更新された場合は「内容が更新されました」を示す。本文はNodeから受け取ったMarkdownとして見出し・箇条書き・引用・表・コード・リンクを描画する。raw HTMLは要素として解釈せず文字列のまま示す。リンクは絶対HTTP(S)だけをOSのブラウザで開く。開閉は表示だけの状態であり、同意ボタンの有効条件と送信する文書・版・snapshotを変えない（#1106）。

## 5. 内容、国際化、文言

### Domeの管理・稼働状態（#1020）

所有Domeの管理は入室と独立した操作とし、停止中・再起動後の一覧にも「Domeを管理」を表示する。Context内の所有数制限は既存Domeの管理導線とともに説明する。作成直後は停止理由と稼働開始の主操作を先に示し、開始・authoritative admission成功後にだけ空間へ表示位置とfocusを移す。管理を開く操作やrefreshだけでJoin、authority reclaim、移管、削除を行わない。

稼働状態、入室状態、外部ピアとの接続状態を分けて表示する。外部ピアが0件でもローカルhostが動いていれば稼働中と説明する。取得失敗を未作成・停止と同一視しない。稼働開始後の入室失敗には入室だけの再試行を用意する。

「稼働を終了」と「Domeを削除」を別操作とし、削除には対象名・影響・取消を含む確認を置く。失敗時は対象と再試行操作を保持し、再起動後も未完了の削除操作へ到達可能にする。他peerの取得済みcopyを消せると表示しない。管理対象はaccount/Context/Instance/generationへ束縛し、遅着応答で別対象の状態やfocusを変更しない。

- カラム本文とControl Center本文の基準は`--text-body`、行高は1.5とする。投稿本文は`--text-body-reading`、カラム一覧・参加先一覧の補助情報は`--text-caption`を使い、ヘッダーの文字階層とportal内Dialogのサイズを分けて維持する。文字密度を変えるために`html`の基準サイズやカラム幅を縮めない。
- 見つけるのフォーム・タブ・Node案内はカラム内部の実幅に収める。長いNode URLを含む規約操作は全文を折り返し、buttonの高さも内容に追従する。フォームの横並びは実幅に余裕がある場合に限り、viewportが広いことだけを条件にしない。
- user-generated contentは、変更した表示と合意済み入力条件に必要な長さ・locale・URL・改行・添付等を有限の例で確認する。列挙した種類の直積や、未依頼の新しい形式を追加しない。
- 長い語や識別子は`overflow-wrap: anywhere`等でcontainmentを守る。省略時は完全値へ到達できる手段を持つ。
- 日本語localeでは、kukuri、固有名、技術識別子以外の未意図な英語を混ぜない。他localeも同一情報と操作結果を保持する。
- 言語を読めない利用者の回復導線として、`settings:appearance.languageLabel` の `Language` 併記と `settings:appearance.languageOptions` の各言語の自称表記を許容する。
- ボタン、pending表示、成功通知、errorで同じ操作名を使う。曖昧な「実行」「失敗」だけで終わらせない。
- emptyとerrorには、利用者が次に行える具体的な行動を示す。値が空であることと取得できなかったことを区別する。
- icon-only操作はローカライズ済みの操作名をaccessible nameとtooltipの双方に使い、tooltipをpointer hoverとkeyboard focusの両方で表示する。

### 5.1 製品表示用語

通常の製品UIと利用者向け文書では、実装・プロトコル上の名前より、SNS利用者が対象と操作を理解できる語を優先する。

| 概念 | 英語 | 日本語 | 簡体字中国語 | 表示契約 |
|---|---|---|---|---|
| 人物・プロフィール | User | ユーザー | 用户 | `Author`、著者、作者は使わない。投稿行為の主体を文章で説明する場合は投稿者とする |
| 未解決の人物 | Unknown user | 不明なユーザー | 未知用户 | 取得中、取得失敗、名前未設定を同じ状態にしない |
| 利用者が保持するもの | Account and your posts | アカウントと自分の投稿 | 账号和自己的帖子 | 詳細説明ではプロフィールとフォロー関係も具体的に列挙する |
| アカウントを識別するもの | Account-identifying key | アカウントを識別する鍵 | 用于识别账号的密钥 | 通常UIでidentity、アイデンティティ、身元情報を単独で使わない |
| 双方向のフォロー | Mutual follow | 相互フォロー | 互相关注 | friendshipを保証しないためFriends／友だちとは呼ばない |
| 投稿操作 | Post | 投稿 | 发布 | 投稿ボタンをPublishと呼ばず、表示・pending・成功・失敗で同じ操作名を使う |
| 引用付き再投稿 | Add a comment | コメントを追加 | 添加评论 | 内部object kindや既存のリポスト機能名は変更しない |

`P2P`、`DM`、`VRM`、URL、ID、公開鍵、プロトコル形式、製品固有名`Dome`は必要な画面で表示できる。完全な識別子、raw code、通信内部状態、英語の実装用語はDeveloper modeまたは診断面へ限定し、通常説明では利用者が行う操作と影響へ言い換える。例外はlocale key単位で有限に管理し、名前空間全体や一般英単語を許可しない。

## 6. Accessibilityと入力

WCAG 2.2 AAを基準とする。対象操作・platformと適用される条件を先に定め、自動検査と必要な描画・入力・読み上げの観測を選ぶ。満点だけで適合を宣言せず、変更のない全surface・入力方式の反復確認を要求しない。

- 通常文字は4.5:1以上、大きい文字と意味を持つ非text UIは3:1以上のcontrastを持つ。theme、hover、disabled、selected、focusを含む実際のforeground／background pairで確認する。
- semantic HTMLを優先し、roleとARIAはnative semanticsを補う場合だけ使う。見出し、landmark、label、description、errorの関係を保つ。
- focus順は視覚順と操作順に一致させ、DialogやColumn移動後にfocusを復元する。sticky header、footer、overlayでfocusを隠さない。
- keyboard trapを禁止する。Escape、Tab、矢印、Enter、Space等はcomponentの既知の操作モデルに合わせる。
- drag、swipe、pinchにはclick、tap、keyboardの代替を提供する。Column並べ替えは実pointer操作とkeyboard代替を持つ。
- 状態更新を必要に応じてlive regionで伝える。過剰なannounceや同じ通知の反復を避ける。
- 200% zoom／reflow、Windows High Contrast、screen reader、reduced motionは、対象変更の適用条件と既存証拠を照合する。必要な結果を自動検証等で判断できない場合に、その操作だけを手動で補う。

### 6.1 操作領域profile

- WCAG 2.2 AAの最小targetは24×24 CSS pxとし、隣接targetとのspacingおよび例外条件を含めて判断する。
- touch向けの目標は44×44 CSS pxとする。Mobileのprimary action、page indicator、icon-only controlはこの目標を優先する。
- Desktopの高密度UIでは36px／40pxの見た目を許容できるが、実hit area、spacing、誤操作リスク、同等操作を確認し、24px未満にしない。

## 7. Responsiveとplatform profile

| Profile | Layout | 入力と密度 |
|---|---|---|
| Mobile（759px以下） | 1 Column＝1 viewport、horizontal scroll snap、safe areaを確保 | touch優先、44px目標、edge／indicatorがpaging gestureを所有 |
| Small Desktop（760〜899px） | Column Canvasを維持し、Column実幅を縮小可能 | pointer／keyboard併用、高密度だがdocument-level overflowを出さない |
| Medium Desktop（900〜1099px） | 440px Columnを比較できる標準確認幅 | 複数Column間のfocus、drag、keyboard移動を確認 |
| Large Desktop（1100px以上） | 複数Columnとwide surfaceを表示 | 情報を無制限に横へ広げず、Column単位の読解幅を保つ |

- desktop Column unitは`--column-unit`、gapは`--column-gap`を使い、複数spanの式は`width = span * columnUnit + (span - 1) * gap`とする。
- Timeline、Notifications、Profile、Threadは1 span、Messages／Conversationは1〜2、Streamは2、Metaverseは3、focused Metaverseは最大4を基準とする。
- internal layoutはColumn自身の実幅に応答し、viewportだけに依存しない。Column Canvasの意図的な横scrollは維持し、document-levelの横scrollを発生させない。
- Metaverseのフォーム・管理groupは最大40rem、roomカードは最大24remとし、短い操作を3列全幅へ伸ばさない。実幅32rem以下では作成フォーム・接続slotを縦積みにする。HUDとchatは同じDOM・draft・開閉状態を保持し、実幅50rem以下ではbadge、toolbar、camera、回復案内、HUD、chatを縦に分ける。HUD内部は縦scrollを許容し、chatの送信・閉じるを本文scrollから分離する。
- 表示中の選択Columnのspan変更では、header操作部へ到達できる最小のCanvas水平補正を行う。手動で選択Columnから離れた場合は引き戻さない。補正だけでfocus、本文縦scroll、入室・network session、Dome設定を変更しない。
- 直前まで全幅を表示していた選択Columnは、Canvas幅やWebViewの拡大率が変わっても表示範囲へ追従させる。その補正scrollをMobileのpage移動と誤認しない。利用者が手動scrollで選択Columnから離れている場合は、閲覧位置を引き戻さない。
- overlay、Control Center、Composer、fullscreen controlはsafe areaと互いのhit areaを塞がない。
- Mobileの下部操作は、Column footerの投稿ボタン（primary action）を右寄せ、アバター・Control Center・フィードバックのclusterを左下に固定し、各ボタンの高さ（44px）と下辺を投稿ボタンへ揃える。更新が利用可能な間だけ、フィードバックの右へ「リリースと更新」を開くprimary塗りのボタンをclusterの最後に加える。幅狭ではclusterの各ボタンをアイコンのみにし、読み上げ名は保つ。Composer入力中はclusterの各ボタンを隠す。
- Tauri／WebView固有の結果はbrowser mockだけで確認済みとしない。合意した対象OS／WebViewで、変更したinput ownership、fullscreen、resource縮退等の必要な証拠を確認する。
- 入室済みMetaverseの全画面ではheaderと補助面の開閉操作を除いた高さを3Dへ割り当てる。Domeの管理・接続・hostingは開閉式の補助面にまとめ、閉じた内容へfocusを入れない。同じscene、camera、chat draft、フォーム入力を保持し、退出後は元のColumnと本文scroll、focusへ戻る。未入室時はdiscoveryと入室導線を維持する。

## 8. Component設計

- native要素、既存primitive/variant、共通componentへの統合、置換・削除を含めて最終コード量を比較する。既存componentを残すためだけのwrapperや別実装を増やさない。
- 同じ状態・判断・操作を個別経路で繰り返し直す場合は、共通の責務へ集約する。見た目だけが似て意味が異なるものを無理に統合したり、将来用途だけの抽象化を作ったりしない。
- boolean propの増殖で暗黙の組合せを作らず、product上の意味を持つ明示variantまたはcompound componentを使う。
- shared componentは実際のconsumerに必要な構成・variant・状態・入力・overflowの契約を持つ。合意した挙動の検証に必要なstoryを用意し、未使用variantや仮想的な全状態を追加しない。
- data取得、domain state、actionsとpresentationの境界を保ち、見た目の都合でdomain契約を変更しない。
- color、type、spacing、radius、border、shadow、motionは現行tokenから取る。helperやadapterは現在の責務分離・可読性・検証に必要な場合に使い、将来要件だけの抽象化は増やさない。
- プロフィールの関係一覧は冗長な総称見出しを置かず、戻る操作、4種の切替、ユーザー一覧の順に並べる。切替は通常2列、広いColumnでは4列とし、paddingは縦4px／横8px、gapは4px、高さは最低32pxを確保する。一覧との間には8pxを置く。
- 関係一覧のユーザー行は最上段に状態バッジと右上メニューを置き、次の段にアバター・名前・自己紹介と右寄せの主操作を置く。名前と自己紹介のgapは4px。主操作は選択中種別に対応し、フォロー中／フォロワーはfollow操作、ミュート中はmute操作、ブロック中はblock操作を使う。現在の状態に応じて解除操作を表示し、副操作はメニューに格納する。
- shellの投稿カードはpaddingを8px、ヘッダーの折り返しgapと本文・操作間の基本間隔を4pxに揃える。余白はゼロにせず、本文・media・状態・操作のまとまりを区別する。Column本文の左右paddingは16pxずつとし、内側のstackで同じ余白を再度幅から差し引かない。Column本文やその主な一覧を丸ごと包むだけのpanelは置かず、一覧はColumn本文へ直接並べる。panelは1つのColumn内の別々の区画を仕切る場合と、内容そのものであるカードに限る（[#1271](docs/ui-reviews/2026-09-21-1271-column-body-frame.md)）。

### Metaverseのアバター操作（#1021）

Domeの所属先は「トピック」または「チャンネル」と呼ぶ。3D表示の移動と見回しをまとめて「アバター操作」、視点だけをボタンで調整する欄を「視点の調整」とする。状態・所属先・入力を曖昧な同じ総称で表さない。

カメラは自分のアバターの描画位置へ追従する三人称を既定とし、初期状態とリセット後は全身・足元・周囲が見える距離を、モデルの描画寸法とcanvasの縦横比から決める。視点の回転・距離・リセットは端末内の表示状態だけを変え、アバター座標・向きやDomeの共有状態を変更しない。移動中に視点の向きを強制的にリセットしない。

「アバター操作を再開」またはactiveなcanvasの明示clickでポインタ固定を取得する。実際の固定中だけマウス移動で見回し、ホイールで距離を変え、Rで視点をリセットする。ドラッグに見回しを割り当てない。WASD／矢印による既存の移動を維持する。

3D表示にfocusがある時、Tabでカテゴリメニュー、Enterでチャットを開き、固定を解除して対象UIへfocusを移す。UI内の通常Tab移動、Enter、IME入力を奪わない。EscapeでUIを閉じる場合はdraftを保持して3D表示へfocusを戻すが、自動で再固定しない。閉じるボタンで最後のUIを閉じた時は再取得を試み、拒否時は明示再開と視点調整ボタンを表示する。

### Metaverseのカテゴリメニュー（#1024）

通常の入室ではチャットと詳細を閉じ、状態・回復案内、退出／Return Home、既存のアバター操作と「メニュー（Tab）」・チャット入口を表示する。ユーザー向け文言には「Dome」「アバター操作」「閉じる」等の具体的な対象・操作を使い、「空間」を新しい操作名に使わない。

メニューはDome設定・稼働管理・接続・アバター・共有物・診断の6カテゴリを円周上に配置する。文字labelとiconを持ち、pointer／tap、通常Tab移動、矢印・Home/End・Enter/Spaceで選択可能にする。詳細は同じ6カテゴリのタブで直接切り替える。通常表示・チャット・カテゴリメニュー・詳細は排他的に開く。詳細の「カテゴリ」はメニューへ、Escapeは最前面の既存dialog／popoverを優先してから3D表示へ戻る。

カテゴリの切替・閉じる・幅変更・全画面切替で、同じDomeの未保存設定・選択・詳細scrollを失わない。非表示paneをfocus順とAccessibility treeから外す。dirtyな設定をrefreshで上書きせず、競合した場合は取消で最新設定を読み込んでから保存する。account／Context／Instance／generationが異なる対象へdraftや遅着した素材importを適用しない。取消は保存を行わない。

稼働管理と診断は同じ取得状態を参照し、カテゴリ移動でpollerやmutationを増やさない。接続・host・共有物・素材・同意の既存actionを明示操作だけで実行する。稼働移管／終了は影響説明付きのgroup、削除は日常操作と区別した表示にする。入室外のDome管理も維持し、入室中に一覧から管理を開くと同じDomeの稼働管理タブへ移る。

peer／hash／lease epoch／session／resource metricsは診断へ集約する。通常面には利用可能性、参加人数、選択した委任先を識別するURLと回復に必要な説明を残す。主照明・環境光は倍率、重力はm/s²、霧は対応する設定範囲内の割合（%）で編集する。保存値のmilli／microsと既存の範囲を変えず、未編集の整数値を丸めない。数値入力とsliderの両方を用意する。

狭幅でも全カテゴリのlabelと操作領域を保ち、詳細内をscroll可能にする。詳細の下にチャット入口の領域を確保し、overlayで覆わない。カラム再選択やfullscreenの可視状態変更だけでは、開いているchatへfocusを奪い戻さない。

### Metaverseの接続方位とマップ（#1025）

接続カテゴリは現在Domeを中央とする北・東・南・西の方位選択、確認済みcomponentの接続マップ、選択方向の詳細で構成する。候補選択、提案、承諾、撤回、解除は既存の明示actionを使う。方位・マップ閲覧だけでは共有状態を変更せず、未知のContextを探索しない。許可された情報は現在の表示・操作に必要な範囲で読む。既存の全同期方式を維持する条件にはしない。

マップは北を上、東を右とし、現在Domeに対する相対座標を使う。番号とDome名、文字の接続一覧でも現在地・方向・隣接先を識別できる。接続状態と通行状態は分け、解除処理中・ブロック・ホスト停止・取得失敗を理由付きで示す。未取得を空きと断定せず、最後の確認値を保持する場合は更新失敗を表示する。

同じ対象の方位と候補はカテゴリ往復で保持する。account／Context／Instance／generation変更後は旧対象の選択や遅着結果を表示しない。方位buttonは通常Tabと矢印、Enter／Spaceで操作できる。広幅は図と詳細を並べ、狭幅は同じ詳細pane内に縦配置してscrollで全操作に到達できる。

別window・別Column・非表示・suspend・退室では自分のcanvasの固定だけを解除し、押下中キーと取得待ちを無効化する。単なるfocus復元で固定を奪わない。操作状態と解除方法を文字で表示し、視点調整にはkeyboard／tapでも使えるボタンを用意する。

## 9. Motion

- motionはstate変化、hierarchy、空間的な移動を説明する場合だけ使う。高頻度操作とkeyboard操作は即応性を優先する。
- `transition: all`を使わず、原則としてtransformとopacityを対象にする。操作で中断でき、最終stateが一意であること。
- `prefers-reduced-motion: reduce`とreview用`data-reduced-motion='reduce'`ではdurationを1ms、移動距離を0にする。focus、active、open／closed等の最終状態は省略しない。
- viewport端のdrag auto-scroll、scroll snap、fullscreen復帰で長いanimationを強制しない。

## 10. React／Tauri性能

- 表示・操作に必要な対象だけを取得し、取得・描画・計数・派生stateのためにtopic/author/peerや履歴の全件読込み・整列・書換えを行わない。未取得や部分取得のまま表示と操作を成立させる。
- 独立した取得も、同時実行・待機・1回の件数/bytes・期限の予算内で進める。件数分のtaskを作って待たせず、partial failureを画面全体の失敗へ拡大しない。
- 一覧はcursorと上限つきの表示・保持窓で構成する。DOMだけのvirtualizationや`content-visibility`で、累積配列・cache・変換処理の増加を隠さない。窓から外れた表示cacheを解放し、閲覧位置はanchor/cursorで保つ。draft・保存済み投稿・未送信outbox等の利用者データを表示cacheと一緒に削除しない。
- Stream／Metaverse等は必要時に読み込み、画面外では描画・再生を停止または縮退する。表示需要と参加中network sessionの寿命を区別する。
- selectorなしの全store購読、重複global listener、render中のlayout read、無制限intervalを作らない。取得・retry・購読は共通の所有者と解除条件、保持上限を持ち、需要の消失後に不要な処理を残さない。
- localStorage等は用途・schema・保持する量と削除条件を定める。設定の保存と取得データのcacheを区別し、保存失敗や未取得で主要操作を塞がない。
- 性能は固定した操作とデータ件数を変えたときの処理回数・待ち・保持量で評価する。小さいデータで十分速いことや、再取得頻度を下げただけの結果を件数非依存の証拠にしない。文書の改訂だけで現行実装が達成済みとは扱わない。
- Next.js、RSC、SSR、SEO固有の最適化は対象外とする。

## 11. 現行visual foundation

- dark-first。`<html data-theme='dark|light'>`で切り替え、OSの`prefers-color-scheme`へ自動追従しない。
- fontは`--font-sans`、技術識別子は`--font-mono`とtabular numeralsを使う。
- surfaceはbase、accent、muted、softの段階で構成し、darkの汎用primary／accent／focusはteal、lightはwarm-orange、dangerは独立したdestructive familyを使う。primary塗りボタンは両themeでwarm-orangeと専用文字色を使う。Columnの上辺グローは選択・固定状態にかかわらず表示しない。選択状態は既存の外周枠・ラベルで識別する。Control Centerボタンの未読数と通知Columnの未読item枠は既存themeのaccent（darkはシアン、lightはwarm-orange）を使う。
- panelは`--radius-panel`、input／Noticeは`--radius-input`、pill controlは`--radius-pill`を使う。avatarは大きさに関わらず常に`--radius-pill`（全丸）とし、角丸へ戻さない。
- textボタンは全丸のまま高さ2rem（`sm`は1.75rem）、横paddingは0.625rem（`sm`は0.5rem）を基準とし、既に小さい文字を縮めずに周辺の余白で密度を確保する。icon-only controlは2rem前後、投稿カードのavatarは1.75rem、profile overviewのavatarは3remを基準にする。
- elevationは`--shadow-panel`、`--shadow-dropdown`、`--shadow-button-primary`に限定する。

### 11.1 現行token契約

次の表は`tokens.css`のroot／dark／lightで実行されるcustom propertyをミラーする。機械検査のため、scope、token、値の3列とmarkerを変更しない。`@theme inline`のTailwind aliasとreduced-motion overrideは派生値であり、この表の対象外とする。

同期は[`design-contract.test.ts`](apps/desktop/src/styles/design-contract.test.ts)が確認する。設計変更では本書・実行値・描画確認面を同じ変更で整合させ、表だけを独立した値の正本にしない。

<!-- TOKEN_CONTRACT_START -->
| Scope | Token | Value |
|---|---|---|
| global | `--font-sans` | `"IBM Plex Sans", "Hiragino Kaku Gothic ProN", "Yu Gothic", "Noto Sans JP", "Meiryo", "Segoe UI", sans-serif` |
| global | `--font-mono` | `"IBM Plex Mono", "Cascadia Code", "Consolas", SFMono-Regular, monospace` |
| global | `--text-display` | `clamp(1.9rem, 4vw, 3.5rem)` |
| global | `--text-h1` | `1.5rem` |
| global | `--text-h2` | `1.25rem` |
| global | `--text-h3` | `1rem` |
| global | `--text-body-reading` | `0.9375rem` |
| global | `--text-body` | `0.875rem` |
| global | `--text-caption` | `0.75rem` |
| global | `--radius-xs` | `0.5rem` |
| global | `--radius-sm` | `0.75rem` |
| global | `--radius` | `1rem` |
| global | `--radius-panel` | `22px` |
| global | `--radius-input` | `14px` |
| global | `--radius-pill` | `999px` |
| global | `--space-2xs` | `0.25rem` |
| global | `--space-xs` | `0.5rem` |
| global | `--space-sm` | `0.75rem` |
| global | `--space-md` | `1rem` |
| global | `--space-lg` | `1.5rem` |
| global | `--space-xl` | `2rem` |
| global | `--space-2xl` | `3rem` |
| global | `--column-unit` | `27.5rem` |
| global | `--column-gap` | `var(--space-md)` |
| global | `--motion-duration-fast` | `120ms` |
| global | `--motion-duration-standard` | `200ms` |
| global | `--motion-duration-slow` | `280ms` |
| global | `--motion-easing-standard` | `cubic-bezier(0.2, 0, 0, 1)` |
| global | `--motion-easing-enter` | `cubic-bezier(0, 0, 0, 1)` |
| global | `--motion-easing-exit` | `cubic-bezier(0.3, 0, 1, 1)` |
| global | `--motion-distance-column` | `1.5rem` |
| global | `--motion-distance-control-center` | `2rem` |
| global | `--blur-hud` | `14px` |
| global | `--surface-metaverse` | `#101318` |
| dark | `--background` | `#121212` |
| dark | `--shell-background` | `#121212` |
| dark | `--foreground` | `#ffffff` |
| dark | `--foreground-strong` | `#ffffff` |
| dark | `--muted-foreground` | `#b3b3b3` |
| dark | `--muted-foreground-soft` | `#b3b3b3` |
| dark | `--surface-panel` | `#292929` |
| dark | `--surface-panel-solid` | `#292929` |
| dark | `--surface-panel-accent` | `#303030` |
| dark | `--surface-panel-muted` | `#242424` |
| dark | `--surface-panel-soft` | `#292929` |
| dark | `--surface-input` | `#202020` |
| dark | `--surface-raised` | `#363636` |
| dark | `--surface-button-primary` | `#d77d45` |
| dark | `--surface-button-primary-hover` | `#c86f38` |
| dark | `--button-primary-foreground` | `#20160e` |
| dark | `--surface-button-secondary` | `#363636` |
| dark | `--surface-button-ghost` | `#292929` |
| dark | `--surface-button-ghost-hover` | `#3d3d3d` |
| dark | `--surface-active` | `#203a37` |
| dark | `--surface-overlay` | `#141414` |
| dark | `--surface-avatar` | `#363636` |
| dark | `--surface-media-loading` | `#292929` |
| dark | `--surface-media-ready` | `#26382e` |
| dark | `--surface-skeleton` | `#363636` |
| dark | `--surface-selection` | `#24524d` |
| dark | `--surface-accent-soft` | `#203a37` |
| dark | `--surface-warning-soft` | `#463423` |
| dark | `--surface-destructive-soft` | `#4a2b22` |
| dark | `--surface-info-soft` | `#203449` |
| dark | `--surface-badge-neutral` | `#292929` |
| dark | `--surface-contrast` | `#363636` |
| dark | `--border-subtle` | `#3d3d3d` |
| dark | `--border-subtle-strong` | `#858585` |
| dark | `--border-accent` | `#03dac5` |
| dark | `--border-warning` | `#bf9358` |
| dark | `--border-destructive` | `#cc8979` |
| dark | `--primary-start` | `#03dac5` |
| dark | `--primary-end` | `#03dac5` |
| dark | `--primary-foreground` | `#00332e` |
| dark | `--accent` | `#03dac5` |
| dark | `--accent-foreground` | `#03dac5` |
| dark | `--destructive` | `#ffb4ab` |
| dark | `--warning` | `#e6b066` |
| dark | `--danger` | `#ffb4ab` |
| dark | `--ring` | `rgba(3, 218, 197, 1)` |
| dark | `--shadow-panel` | `0 18px 60px rgba(0, 0, 0, 0.18)` |
| dark | `--shadow-dropdown` | `0 12px 32px rgba(0, 0, 0, 0.18)` |
| dark | `--shadow-button-primary` | `0 10px 28px rgba(0, 0, 0, 0.12)` |
| dark | `--scrollbar-track` | `#292929` |
| dark | `--scrollbar-thumb` | `#858585` |
| dark | `--scrollbar-thumb-hover` | `#a0a0a0` |
| light | `--background` | `#f4f4f3` |
| light | `--shell-background` | `#f4f4f3` |
| light | `--foreground` | `#242424` |
| light | `--foreground-strong` | `#171717` |
| light | `--muted-foreground` | `#62625f` |
| light | `--muted-foreground-soft` | `#62625f` |
| light | `--surface-panel` | `#ffffff` |
| light | `--surface-panel-solid` | `#ffffff` |
| light | `--surface-panel-accent` | `#ececea` |
| light | `--surface-panel-muted` | `#f7f7f5` |
| light | `--surface-panel-soft` | `#f7f7f5` |
| light | `--surface-input` | `#ffffff` |
| light | `--surface-raised` | `#e5e5e2` |
| light | `--surface-button-primary` | `#d77d45` |
| light | `--surface-button-primary-hover` | `#c86f38` |
| light | `--button-primary-foreground` | `#20160e` |
| light | `--surface-button-secondary` | `#ececea` |
| light | `--surface-button-ghost` | `#f7f7f5` |
| light | `--surface-button-ghost-hover` | `#e5e5e2` |
| light | `--surface-active` | `#f8e8dd` |
| light | `--surface-overlay` | `#d6d6d2` |
| light | `--surface-avatar` | `#e5e5e2` |
| light | `--surface-media-loading` | `#ececea` |
| light | `--surface-media-ready` | `#e5eee7` |
| light | `--surface-skeleton` | `#e5e5e2` |
| light | `--surface-selection` | `#f0c9ac` |
| light | `--surface-accent-soft` | `#f8e8dd` |
| light | `--surface-warning-soft` | `#f6e7d9` |
| light | `--surface-destructive-soft` | `#f6dfd4` |
| light | `--surface-info-soft` | `#dce7f4` |
| light | `--surface-badge-neutral` | `#ececea` |
| light | `--surface-contrast` | `#e5e5e2` |
| light | `--border-subtle` | `#d6d6d2` |
| light | `--border-subtle-strong` | `#85857f` |
| light | `--border-accent` | `#a44a21` |
| light | `--border-warning` | `#a67839` |
| light | `--border-destructive` | `#b56a50` |
| light | `--primary-start` | `#d77d45` |
| light | `--primary-end` | `#d77d45` |
| light | `--primary-foreground` | `#20160e` |
| light | `--accent` | `#a44a21` |
| light | `--accent-foreground` | `#713717` |
| light | `--destructive` | `#9d4d36` |
| light | `--warning` | `#845e21` |
| light | `--danger` | `#9d4d36` |
| light | `--ring` | `rgba(164, 74, 33, 0.9)` |
| light | `--shadow-panel` | `0 18px 48px rgba(0, 0, 0, 0.08)` |
| light | `--shadow-dropdown` | `0 12px 32px rgba(0, 0, 0, 0.10)` |
| light | `--shadow-button-primary` | `0 10px 24px rgba(0, 0, 0, 0.08)` |
| light | `--scrollbar-track` | `#f4f4f3` |
| light | `--scrollbar-thumb` | `#85857f` |
| light | `--scrollbar-thumb-hover` | `#62625f` |
<!-- TOKEN_CONTRACT_END -->

### 11.2 提案token

現時点で提案tokenはない。明示的な変更要求に必要な場合だけ、対応する受入条件、導入条件、実consumer、dark／light値を記録し、採用時に現行契約へ反映する。子Issueや未使用tokenを先に作る必要はない。

## 12. エージェント向けクイックリファレンス

```text
AGENTS.mdの作業原則・設計原則を先に適用し、対象操作と終了条件を固定する。
kukuriはdark-firstのOperate型UI。
topic／contentを主役にし、diagnosticsと完全な技術識別子は後景へ置く。
色、余白、radius、shadow、motionはtokens.cssの現行tokenを使う。
partial／offline／reconnecting／degradedをemptyと区別し、直前の有効値を保持する。
Columnのscope、focus、draft、戻る文脈を不用意に失わない。
pointer、touch、keyboard、screen readerで同じ操作結果へ到達できるようにする。
取得・表示・保持を有限窓にし、全履歴の取得完了を待たない。
合意した入力・環境の検証を選び、未依頼のエッジケースや状態の直積を追加しない。
共通化・置換・削除を含む最小の最終コード量を選ぶ。
実装と検証の手順はADR 0014、CSS／stateの配置はdesktop UI architectureを参照する。
```
