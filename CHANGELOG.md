# Changelog

All notable changes to kukuri are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Releases use the preview tag scheme `vX.Y.Z-preview.N`.

Per-release sections under this header are generated automatically by the
`changelog` job in `.github/workflows/kukuri-release.yml`, which runs
`scripts/release/update-changelog.ps1` against the git history between the
previous release tag and the release tag, links each entry to its pull
request, and commits the result. See `docs/runbooks/release.md` for the
release workflow.

Changes released in `v0.1.1-preview.1` and earlier are tracked in the
[GitHub Releases](https://github.com/KingYoSun/kukuri/releases) instead of this
file; automated changelog entries start from the next preview release.

## [Unreleased]

## [v0.5.0-preview.1] - 2026-10-27

### Features

- 添付ごとのPushと全参加者の回数表示を追加 ([#1954](https://github.com/kukuri-app/kukuri/pull/1954))
- 開発者モードなしでライブを利用できるようにする ([#1935](https://github.com/kukuri-app/kukuri/pull/1935))
- ライブ開始とサムネイルからの視聴導線を整理 ([#1933](https://github.com/kukuri-app/kukuri/pull/1933))
- 画像・動画のブックマーク、blob再利用と出典表示を追加 ([#1929](https://github.com/kukuri-app/kukuri/pull/1929))

### Fixes

- preserve consumed render order as capture advances ([#1960](https://github.com/kukuri-app/kukuri/pull/1960))
- allow Android image checks to finish display retries ([#1959](https://github.com/kukuri-app/kukuri/pull/1959))
- keep a render frame for late capture callbacks ([#1958](https://github.com/kukuri-app/kukuri/pull/1958))
- Android IME editorへの重複タップを削除する ([#1957](https://github.com/kukuri-app/kukuri/pull/1957))
- reject cancelled display responses ([#1956](https://github.com/kukuri-app/kukuri/pull/1956))
- fall back to Docker Hub when fixture mirror fails ([#1953](https://github.com/kukuri-app/kukuri/pull/1953))
- デバイス時刻でエコー参照と取り込みを合わせる ([#1948](https://github.com/kukuri-app/kukuri/pull/1948))
- follow後の作者情報を古いプロフィール応答で戻さない ([#1947](https://github.com/kukuri-app/kukuri/pull/1947))
- wait for the public post before reopening its author profile ([#1940](https://github.com/kukuri-app/kukuri/pull/1940))
- カラムのトピック表示と選択を対象別に揃える ([#1931](https://github.com/kukuri-app/kukuri/pull/1931))

### Other

- リリースの版を 0.5.0 にする ([#1961](https://github.com/kukuri-app/kukuri/pull/1961))
- 投稿の先頭画像・透過GIFをLiveに重ねる ([#1951](https://github.com/kukuri-app/kukuri/pull/1951))
- DM会話を開く操作・API・列の記録を追加する ([#1946](https://github.com/kukuri-app/kukuri/pull/1946))
- SA-4失敗時の音声と入出力遅延を保存する ([#1945](https://github.com/kukuri-app/kukuri/pull/1945))
- mic停止後の映像到達を待ってから無音を確認する ([#1942](https://github.com/kukuri-app/kukuri/pull/1942))
- 旧client終了後のshell cache回収を待つ ([#1939](https://github.com/kukuri-app/kukuri/pull/1939))
- 0.4.9 preview の公開と LP・本番更新記録 ([#1927](https://github.com/kukuri-app/kukuri/pull/1927))

## [v0.4.9-preview.1] - 2026-10-23

### Features

- topic の lease が 60 秒ごとに近隣∪台帳の次の相手へ紹介を求め、後から現れた保持者へ届く（#1910 AC-2） ([#1919](https://github.com/kukuri-app/kukuri/pull/1919))
- 公開 topic の読み先に、近隣の紹介で見つけた非近隣の通常 peer を足す（#1910 AC-1） ([#1918](https://github.com/kukuri-app/kukuri/pull/1918))
- 保持の期間と容量の内で公開 topic を保持し、前日より前の日も遡って取り込む（#1909 AC-3） ([#1917](https://github.com/kukuri-app/kukuri/pull/1917))
- 保持する topic の blob・リアクション・取り下げ・session・Dome も取り込み、取れない対象を取り直す（#1909 AC-2） ([#1916](https://github.com/kukuri-app/kukuri/pull/1916))
- CLI daemon が指定した公開 topic を画面の列なしに取り込む（#1909 AC-1） ([#1913](https://github.com/kukuri-app/kukuri/pull/1913))

### Fixes

- CIのAPT依存installを共通化し、ミラー切替と時間上限を設ける ([#1924](https://github.com/kukuri-app/kukuri/pull/1924))
- CLI の公開スレッドの出力 schema を共有 DTO の欄に合わせる（#1914） ([#1915](https://github.com/kukuri-app/kukuri/pull/1915))

### Other

- リリースの版を 0.4.9 にする ([#1926](https://github.com/kukuri-app/kukuri/pull/1926))
- Rust依存とiroh forkをv1.4.1へ更新する（#1921 AC-1） ([#1925](https://github.com/kukuri-app/kukuri/pull/1925))
- desktop／CLI の実 stack と Web の実ブラウザで、紹介で見つけた保持者からの取得と再提供を確かめる（#1910 AC-3） ([#1920](https://github.com/kukuri-app/kukuri/pull/1920))
- rust-cache の key から runner 同梱の toolchain を外し、Windows・nextest・Playwright の job を分ける（#1911） ([#1912](https://github.com/kukuri-app/kukuri/pull/1912))
- 外部 relay の結合試験で session を確認済みにしてから接続を反映する ([#1908](https://github.com/kukuri-app/kukuri/pull/1908))
- 0.4.8 preview の公開と LP・運用記録 ([#1907](https://github.com/kukuri-app/kukuri/pull/1907))

## [v0.4.8-preview.1] - 2026-10-21

### Features

- 公開スレッド一覧の行に最初の投稿のサムネイルを出す（#1900） ([#1903](https://github.com/kukuri-app/kukuri/pull/1903))

### Fixes

- 侵害された eMudhra の中間 CA を拒否する版へ証明書検証・Node・CN の CA を上げる（#1904 AC-1） ([#1905](https://github.com/kukuri-app/kukuri/pull/1905))
- Service Worker の install の取得で HTTP cache を通さない（#1901 AC-1） ([#1902](https://github.com/kukuri-app/kukuri/pull/1902))

### Other

- リリースの版を 0.4.8 にする ([#1906](https://github.com/kukuri-app/kukuri/pull/1906))
- 0.4.7 preview の公開と LP・運用記録 ([#1899](https://github.com/kukuri-app/kukuri/pull/1899))

## [v0.4.7-preview.1] - 2026-10-20

### Features

- 公開スレッドの作成者がタイトルを変え・削除し、削除済みを一覧・詳細・リンクで示す（#1888 AC-2） ([#1897](https://github.com/kukuri-app/kukuri/pull/1897))
- 公開スレッドの作成者がタイトルを変え、スレッドを削除できるようにする（#1888 AC-1） ([#1895](https://github.com/kukuri-app/kukuri/pull/1895))
- 公開スレッドの作成時に最初の投稿へ添付と成人向けラベルを付けられるようにする ([#1889](https://github.com/kukuri-app/kukuri/pull/1889), [#1893](https://github.com/kukuri-app/kukuri/pull/1893))
- 公開スレッドの詳細で返信をツリー表示し、列の中で返信する ([#1892](https://github.com/kukuri-app/kukuri/pull/1892))

### Fixes

- 公開スレッドを会話順に表示しタイムライン操作から分離 ([#1890](https://github.com/kukuri-app/kukuri/pull/1890))

### Other

- リリースの版を 0.4.7 にする ([#1898](https://github.com/kukuri-app/kukuri/pull/1898))
- failure_paths の 2 つの間欠失敗の原因を取り除く ([#1894](https://github.com/kukuri-app/kukuri/pull/1894), [#1896](https://github.com/kukuri-app/kukuri/pull/1896))
- 0.4.6 preview の公開と LP・運用記録 ([#1886](https://github.com/kukuri-app/kukuri/pull/1886))

## [v0.4.6-preview.1] - 2026-10-19

### Features

- Web でも Live を視聴でき、公開スレッドに配信中の印を出す（#1879 AC-2） ([#1882](https://github.com/kukuri-app/kukuri/pull/1882))
- Web でも開発者モードで Live を作成して配信できるようにする（#1879 AC-1） ([#1881](https://github.com/kukuri-app/kukuri/pull/1881))
- ライブの列の左側に Live を大きく出し、一覧のカードと投稿の中の Live から開けるようにする（#1841 AC-3） ([#1870](https://github.com/kukuri-app/kukuri/pull/1870))
- Live の映像に会話の投稿と開始 post のリアクションを重ねる部品を足す（#1841 AC-2） ([#1869](https://github.com/kukuri-app/kukuri/pull/1869))
- Live の重ねる表示の土台として、Live・会話の場所・開始 post を結び付け、リアクションの新着を知らせる（#1841 AC-1） ([#1868](https://github.com/kukuri-app/kukuri/pull/1868))
- 公開スレッドで Live を始め、詳細の上部と一覧の行で配信中を示す（#1840 AC-4） ([#1867](https://github.com/kukuri-app/kukuri/pull/1867))
- topic の公開スレッドの一覧の列と、作成・詳細・本文のリンクの表示を足す（#1840 AC-3） ([#1866](https://github.com/kukuri-app/kukuri/pull/1866))
- topic の公開スレッドの一覧を足し、別の端末のスレッドを 2 つの並び順で取得できるようにする（#1840 AC-2） ([#1865](https://github.com/kukuri-app/kukuri/pull/1865))
- 公開 channel（公開スレッド）を channel の基盤に足し、招待の無い端末が ID から読み書きできるようにする（#1840 AC-1） ([#1864](https://github.com/kukuri-app/kukuri/pull/1864))
- 同じ Live の表示で視聴を共有し、Live ごとの音量を端末に保存する（#1839 AC-4） ([#1863](https://github.com/kukuri-app/kukuri/pull/1863))
- Live の配信を shell の上部の操作に移し、列や画面を移っても続ける（#1839 AC-3） ([#1862](https://github.com/kukuri-app/kukuri/pull/1862))
- 投稿から Live を始め、別の topic・channel へ紹介し、紹介から同じ Live に参加できるようにする（#1839 AC-2） ([#1861](https://github.com/kukuri-app/kukuri/pull/1861))
- Live を開始 post と結び付け、別の場所の紹介 post から同じ Live を解決する（#1839 AC-1） ([#1860](https://github.com/kukuri-app/kukuri/pull/1860))
- malware の通報から添付の blob hash を制限・解除できるようにする（#1062 AC-2） ([#1857](https://github.com/kukuri-app/kukuri/pull/1857))
- malware の通報を運営者の確認から投稿の制限・解除へつなぐ（#1062 AC-1） ([#1854](https://github.com/kukuri-app/kukuri/pull/1854))

### Fixes

- 配布の WASM を大きさ優先で最適化し、25 MiB の上限の下に収める（#1884） ([#1885](https://github.com/kukuri-app/kukuri/pull/1885))
- private channel の返信の URL を起動直後に開いても、公開の scope の返信の列を開かない（#1877） ([#1878](https://github.com/kukuri-app/kukuri/pull/1878))
- private channel の返信の列を開いたまま再起動しても、公開の scope の返信の列を開かない（#1875） ([#1876](https://github.com/kukuri-app/kukuri/pull/1876))
- 公開スレッドの返信の列を開いたまま再起動しても、同じ投稿の返信の列をもう 1 本開かない（#1873） ([#1874](https://github.com/kukuri-app/kukuri/pull/1874))
- 公開スレッドの詳細の列から返信しても、詳細の列の位置と親を変えない（#1871） ([#1872](https://github.com/kukuri-app/kukuri/pull/1872))
- 配信の CSP の connect-src で blob: を許す（#1858 AC-1） ([#1859](https://github.com/kukuri-app/kukuri/pull/1859))
- 先頭のページが欠けて表示された後に届いた古い行を新着として保留せず、続きを読めるようにする（#1851 AC-2） ([#1856](https://github.com/kukuri-app/kukuri/pull/1856))
- Dome のアバターが texture を読めず白一色になるのを直す（#1141 AC-1） ([#1855](https://github.com/kukuri-app/kukuri/pull/1855))
- タイムラインの途中で上へ scroll しても、読み込んだ続きのページを捨てない（#1851 AC-1） ([#1853](https://github.com/kukuri-app/kukuri/pull/1853))
- Linux の kukuri:// の登録で、AppImage の library の環境を host の command へ渡さない（#1797 AC-1） ([#1852](https://github.com/kukuri-app/kukuri/pull/1852))
- 実アプリの試験の失敗時の診断と終了の評価に期限を付け、応答しない画面があっても止まらずに log を残す（#1842 AC-1） ([#1843](https://github.com/kukuri-app/kukuri/pull/1843))
- 起動直後の URL の正規化で、直後に移った URL を既定の topic に戻さない（#1837 AC-1） ([#1838](https://github.com/kukuri-app/kukuri/pull/1838))

### Other

- リリースの版を 0.4.6 にする ([#1883](https://github.com/kukuri-app/kukuri/pull/1883))
- 0.4.5 preview の公開と LP・運用記録 ([#1836](https://github.com/kukuri-app/kukuri/pull/1836))

## [v0.4.5-preview.2] - 2026-10-15

### Features

- KUKURI_INSTANCE を付けた起動は同じ PC の他の instance と同時に動かす（#1800 AC-1） ([#1801](https://github.com/kukuri-app/kukuri/pull/1801))
- 視聴者どうしの転送と配信者の署名で、直接の枠を超えた視聴者へ届ける（#1034 AC-2b） ([#1794](https://github.com/kukuri-app/kukuri/pull/1794))
- 配信者から直接届く視聴者へ映像と音声を配信する（#1034 AC-2a） ([#1793](https://github.com/kukuri-app/kukuri/pull/1793))
- 空間音声を moq-lite の専用 ALPN と Rust の音声処理へ移す（#1033 AC-1） ([#1791](https://github.com/kukuri-app/kukuri/pull/1791))

### Fixes

- WebKitGTK 2.50 で <video> から作った frame が encode できないので、canvas に描いてから作る（#1834 AC-1） ([#1835](https://github.com/kukuri-app/kukuri/pull/1835))
- 画面が変わらない間の送り直しを、1 秒ごとの tick で毎回送る形にする（#1828 AC-1） ([#1829](https://github.com/kukuri-app/kukuri/pull/1829))
- 他の author の Dome の preset は、一度読んで検証したものを参照ごとに使い回す（#1818 AC-1） ([#1821](https://github.com/kukuri-app/kukuri/pull/1821))
- session を読むとき、反映済みと同じ版を返す読み先で打ち切らず、残りの読み先から新しい版を探す（#1816 AC-1） ([#1820](https://github.com/kukuri-app/kukuri/pull/1820))
- 変更の知らせの時点で取り直せなかった live・game の一覧を、次の store の更新で取り直す（#1815 AC-1） ([#1819](https://github.com/kukuri-app/kukuri/pull/1819))
- hint の時点で読み先に無かった session を、間を置いて読み直して一覧に出す（#1813 AC-1） ([#1814](https://github.com/kukuri-app/kukuri/pull/1814))
- 空間音声の echo cancellation が再生の mix を受け取るよう、moq-audio を fork に固定する（#1805 AC-1） ([#1812](https://github.com/kukuri-app/kukuri/pull/1812))
- 配信をし直しても、見続けている Linux の視聴者に映像を描く（#1809 AC-1） ([#1810](https://github.com/kukuri-app/kukuri/pull/1810))
- Dome で mute した相手の声を、画面の作者の情報に無くても流さない（#1807 AC-1） ([#1808](https://github.com/kukuri-app/kukuri/pull/1808))
- 動いている Dome の host を一瞬 offline と読まない（#1803 AC-1） ([#1804](https://github.com/kukuri-app/kukuri/pull/1804))
- CSP で Tauri の IPC の custom protocol を許し、live の映像を raw のまま受け渡す（#1034 AC-2a） ([#1799](https://github.com/kukuri-app/kukuri/pull/1799))
- 隣の Dome を読み直しの間も確かめ済みの状態に保ち、空間音声の購読を切らない（#1033 AC-1） ([#1798](https://github.com/kukuri-app/kukuri/pull/1798))
- AppImage に GStreamer の plugin を同梱して動画の再生と WebCodecs を使えるようにする（#1795） ([#1796](https://github.com/kukuri-app/kukuri/pull/1796))
- baseline候補集合を現行scanner除外へ同期

### Other

- リリースの版を 0.4.5 にする ([#1833](https://github.com/kukuri-app/kukuri/pull/1833))
- 空間音声の試験で歩かせる前に、操作を再開できるまで window と Dome の列を置き直す（#1822 AC-1） ([#1832](https://github.com/kukuri-app/kukuri/pull/1832))
- update doc ([#1831](https://github.com/kukuri-app/kukuri/pull/1831))
- SA-4 で話者に聞き手の声が届く基準を、話者の声の 10% にする（#1822 AC-4） ([#1830](https://github.com/kukuri-app/kukuri/pull/1830))
- 実アプリの試験が失敗したとき、各 instance の選ばれている列と focus の位置を log に出す（#1822 AC-3） ([#1827](https://github.com/kukuri-app/kukuri/pull/1827))
- 実アプリの試験の topic を test にし、外へ出る通信を runbook に正しく書く（#1825 AC-1） ([#1826](https://github.com/kukuri-app/kukuri/pull/1826))
- live の映像・音声の確認を、OS の入出力の境界と視聴者の canvas までの自動の試験にする（#1800 AC-4） ([#1811](https://github.com/kukuri-app/kukuri/pull/1811))
- 実アプリの試験の PulseAudio を、desktop と同じく real-time の優先度で動かす（#1822 AC-2） ([#1824](https://github.com/kukuri-app/kukuri/pull/1824))
- 空間音声の試験で歩かせる前に、window の focus を待って Dome の列を選ぶ（#1822 AC-1） ([#1823](https://github.com/kukuri-app/kukuri/pull/1823))
- 空間音声の確認を、OS の入出力の境界までの自動の試験にする（#1800 AC-3） ([#1806](https://github.com/kukuri-app/kukuri/pull/1806))
- 配布と同じ build を同じ PC で複数動かす自動の試験の仕組みを足し、CI で動かす（#1800 AC-2） ([#1802](https://github.com/kukuri-app/kukuri/pull/1802))
- live の映像の方式・codec・取り込む対象・画質の上限を決める（#1034 AC-1） ([#1792](https://github.com/kukuri-app/kukuri/pull/1792))
- metaverse の空間音声と live 配信のメディア搬送を決める（#1031 AC-1） ([#1790](https://github.com/kukuri-app/kukuri/pull/1790))
- 旧大型登録の解消を既存状態として正確に記す
- リファクタリング成果と次回監査baselineを確定
- 解消済みDesktopShellPageの大型登録を除去
- index query の要求準備と結果処理を統合 ([#1787](https://github.com/kukuri-app/kukuri/pull/1787))
- 完了済みの通信移行とCN回収の現状を同期 ([#1788](https://github.com/kukuri-app/kukuri/pull/1788))
- declare content commands in one local table ([#1786](https://github.com/kukuri-app/kukuri/pull/1786))
- 著者タイムラインの正本を著者別mapへ統合 ([#1785](https://github.com/kukuri-app/kukuri/pull/1785))
- share one subscription generation counter ([#1784](https://github.com/kukuri-app/kukuri/pull/1784))
- 0.4.4 preview の公開と LP・運用記録 ([#1776](https://github.com/kukuri-app/kukuri/pull/1776))

## [v0.4.4-preview.1] - 2026-10-12

### Features

- bind original Blob inputs to account operations and read finite chunks
- expose explicit shell updates and preserve live draft persistence
- prepare static shell caching and browser retention contract
- 任意のホーム画面追加とmanifestを実装 (#1767 AC-ENTRY) ([#1769](https://github.com/kukuri-app/kukuri/pull/1769))
- topic内と横断の検索をTantivyのreaderで返す（#1724 AC-3） ([#1741](https://github.com/kukuri-app/kukuri/pull/1741))
- 既存投影を128件ずつ検索の索引へ写して再開する（#1724 AC-2） ([#1740](https://github.com/kukuri-app/kukuri/pull/1740))
- 検索の索引へ取込と有界な回収を反映する（#1724 AC-1） ([#1739](https://github.com/kukuri-app/kukuri/pull/1739))
- 信頼評価情報の根拠の続きを、助言パネルの「さらに表示」と CLI の cursor で読む（#1702 AC-2） ([#1727](https://github.com/kukuri-app/kukuri/pull/1727))
- フィードを相互フォロー・フォローのみに絞るフィルターを追加 ([#1689](https://github.com/kukuri-app/kukuri/pull/1689), [#1692](https://github.com/kukuri-app/kukuri/pull/1692))
- kukuri.app で NIP-05 の nostr.json を配る ([#1687](https://github.com/kukuri-app/kukuri/pull/1687))

### Fixes

- keep composer attachment controls within the mobile text reflow
- preserve settings handoffs and restore emulator accessibility state
- keep a single routed settings entry when handing off overlays
- close settings before the underlying composer on back
- reuse retired history entries across dialog handoff
- keep dialog handoff ahead of history cleanup
- keep layered back navigation inside the PWA
- preserve held private reads and reclaim concurrent shell caches
- avoid recreating a reclaimed cache during asset lookup
- keep settings actions readable at 320px (#1198 AC-3a) ([#1762](https://github.com/kukuri-app/kukuri/pull/1762))
- 前回と別の項目で設定を開くと履歴が 2 つ積まれ、戻ると前回の項目へ移る不具合を直す ([#1734](https://github.com/kukuri-app/kukuri/pull/1734))
- resolve OS taps outside the inbox page ([#1756](https://github.com/kukuri-app/kukuri/pull/1756))
- keep channel link confirmation tied to its current token ([#1753](https://github.com/kukuri-app/kukuri/pull/1753))
- WebViewの動画poster生成とtimeout回収を直す ([#1746](https://github.com/kukuri-app/kukuri/pull/1746))
- CN fixture imageを同一digestのcacheから取得する ([#1742](https://github.com/kukuri-app/kukuri/pull/1742), [#1743](https://github.com/kukuri-app/kukuri/pull/1743))
- 観測の期限削除を索引から128件ずつ進める (#1704 AC-3) ([#1738](https://github.com/kukuri-app/kukuri/pull/1738))
- 索引の回収が選んだ行だけを読むようにする（#1736 AC-1） ([#1737](https://github.com/kukuri-app/kukuri/pull/1737))
- 信頼値の集計の掃除が、選んだ行を ctid で消し、満杯の回に表の全行を読まないようにする（#1732 AC-1） ([#1735](https://github.com/kukuri-app/kukuri/pull/1735))
- 期限削除を上限つきの小分けにし、起動を待たせない（#1704 AC-2） ([#1731](https://github.com/kukuri-app/kukuri/pull/1731))
- 案件保持の期限を行に持たず、起算点と保持日数で判定する（#1704 AC-1） ([#1713](https://github.com/kukuri-app/kukuri/pull/1713))
- ArcadeDB を 26.10.1 に上げ、索引の圧縮の後も scope の回収が投影の文書をすべて消すようにする（#1726 AC-1） ([#1729](https://github.com/kukuri-app/kukuri/pull/1729))
- 信頼値の照会を対象ごとの集計と basis のページで返し、risk signal の全行の読取りをやめる（#1702 AC-1） ([#1720](https://github.com/kukuri-app/kukuri/pull/1720))
- scope を指定した発見の新着の列挙が、scope の全件を読まず返す件数だけ読むようにする（#1719 AC-1） ([#1722](https://github.com/kukuri-app/kukuri/pull/1722))
- readiness の索引の整合検査が索引と判定の表を全行読まないようにする（#1714 AC-1） ([#1718](https://github.com/kukuri-app/kukuri/pull/1718))
- 観測の revision と観測提供の取消の表を廃止する（#1699 AC-2） ([#1717](https://github.com/kukuri-app/kukuri/pull/1717))
- 起動時の機微データの処理を、暗号文 1 行の鍵の照合と migration の旧平文の確認にする（#1705 AC-1） ([#1710](https://github.com/kukuri-app/kukuri/pull/1710))
- legal hold を解除で行ごと消し、解除済みの行を残さない（#1706 AC-2） ([#1716](https://github.com/kukuri-app/kukuri/pull/1716))
- 反映の途中で止まった edge の item を、同じ item の再受信で反映し直す（#1707） ([#1712](https://github.com/kukuri-app/kukuri/pull/1712))
- 索引の真実源に投稿 id の hash の索引を足し、申出の受付と送信防止の適用が全行を読まないようにする（#1708 AC-1） ([#1711](https://github.com/kukuri-app/kukuri/pull/1711))
- 却下等の権利侵害申出の本体を、履歴の保持を待たずに区分の期限で物理削除する（#1706 AC-1） ([#1709](https://github.com/kukuri-app/kukuri/pull/1709))
- risk signal の著者の対応を、参照する risk signal の削除と同じ取引で消す（#1699 AC-1） ([#1703](https://github.com/kukuri-app/kukuri/pull/1703))
- 送信防止を適用した投稿の ArcadeDB の写しを、消し待ちから indexer の巡回で消す（#1698 AC-2） ([#1701](https://github.com/kukuri-app/kukuri/pull/1701))
- 自分が作ったリアクションを、同じアカウントの別の端末でセット・投稿から保存できるようにする ([#1688](https://github.com/kukuri-app/kukuri/pull/1688))
- 複数枚の画像の投稿で先頭 4 枚を並べて取得し、5 枚目以降を拡大表示で取得する（#1690） ([#1693](https://github.com/kukuri-app/kukuri/pull/1693))

### Other

- リリースの版を 0.4.4 にする ([#1774](https://github.com/kukuri-app/kukuri/pull/1774))
- record fixed PWA integration evidence and final code boundaries
- return rejected input as evidence without the WebDriver error key
- expose structured source preparation errors in emulator evidence
- finalize Android PWA input evidence and remove temporary build reuse
- reach the Chrome install action through the enlarged native menu
- use the emulator override size for launcher page swipes
- retain native input contracts and remove setup diagnostics
- remove temporary artifact reuse after native PWA input probes
- resolve reply and message fields before native input observation
- verify Enter commits an active native IME composition
- assert actual Japanese preedit confirmation independently of the IME dictionary
- wait for rendered UI evidence and select the localized layout
- select the intended Japanese layout and measure touch-exploration focus
- classify the focus inspector as emulator test code
- read actual TalkBack focus without suppressing the service
- exercise native Japanese conversion and photo-picker file browsing
- wait for native keyboard and reacquire dismissed dialog controls
- isolate native input, Japanese IME, and TalkBack probes
- select the Japanese language result rather than its query
- reuse identical product builds for the mobile input probe
- match the native language field and separate TalkBack readiness
- verify native file IO and confirmation guards in the installed PWA
- remove the unused moved settings setter
- verify installed PWA rotation and encrypted clipboard recovery
- operate observed Gboard toolbar and cancel native picker
- use SDK TalkBack and configure the viewport before Chrome
- inspect the SDK TalkBack package before activation
- exercise first consent at mobile width and enlarged text
- match TalkBack to the emulator ABI and capture native UI
- avoid closing the APK verification pipe early
- prepare Japanese Gboard and signed TalkBack emulator checks
- record verified installed PWA offline and update flow
- remove unused private read fixture import
- capture private DocRead rejection on offline restart
- diagnose remaining PWA delivery and cache failures
- foreground the installed app and display private scope
- read live textarea values and restore private demand
- type through Android input in installed windows
- use actual text entry and observe update navigation
- verify installed offline data and retained drafts
- supply the video poster and satisfy frontend checks
- report structured runtime errors at the browser boundary
- exercise offline receipts and explicit updates with real Web builds
- preserve offline instrumentation and verify draft failure boundaries
- fix Android Chrome scope and acceptance plan for #1767 ([#1768](https://github.com/kukuri-app/kukuri/pull/1768))
- 未ACKのDMをoptimistic表示で配送完了にしない ([#1750](https://github.com/kukuri-app/kukuri/pull/1750))
- 送信防止の状態を認証なしで返し、判断記録に著者を持たないと ADR 0032 を改訂する（#1698 AC-1） ([#1700](https://github.com/kukuri-app/kukuri/pull/1700))
- 0.4.3 preview の公開と LP・運用記録 ([#1686](https://github.com/kukuri-app/kukuri/pull/1686))

## [v0.4.3-preview.1] - 2026-10-09

### Features

- 閲覧した端末が NIP-05 の識別子を照会し、名前の後ろに @domain を出す（#1670 AC-2） ([#1684](https://github.com/kukuri-app/kukuri/pull/1684))
- カスタムリアクションのセットを作り、投稿・DM から取り込めるようにする（#1232 AC-4） ([#1682](https://github.com/kukuri-app/kukuri/pull/1682))
- プロフィールに NIP-05 の識別子を設定し、署名つきプロフィールで配る（#1670 AC-1） ([#1679](https://github.com/kukuri-app/kukuri/pull/1679))
- 自作・保存済みのカスタムリアクションの画像を公開 blob の発見の対象にする（#1232 AC-3） ([#1681](https://github.com/kukuri-app/kukuri/pull/1681))
- カスタムリアクションの ID を画像の hash と検索名から求める（#1232 AC-1） ([#1680](https://github.com/kukuri-app/kukuri/pull/1680))
- 主ボタンを OS に合わせ（Mac・スマートフォン・OS 不明はブラウザ版）、metadata を Web 公開に揃える（#1669） ([#1675](https://github.com/kukuri-app/kukuri/pull/1675))
- LP の画面と OGP を、ベランダ菜園の会話が読めるスマートフォン幅の画面へ替える（#1668） ([#1672](https://github.com/kukuri-app/kukuri/pull/1672))
- LP を 4 つの役割群へ絞り、Hero を「話題でつながる。運営は選べる。」にする（#1667） ([#1671](https://github.com/kukuri-app/kukuri/pull/1671))
- スマートフォンの導線をブラウザ版へ切り替え、Web 版の名前を kukuri web にする ([#1665](https://github.com/kukuri-app/kukuri/pull/1665))

### Fixes

- カスタム絵文字の保存メニューを「保存して自分も使う」にする（#1232 AC-2） ([#1678](https://github.com/kukuri-app/kukuri/pull/1678))
- 通知の列の文脈の行で、話題とチャンネルを ID ではなく名前で出す（#1676） ([#1677](https://github.com/kukuri-app/kukuri/pull/1677))
- 投稿カードの元のトピック・元トピックを英数字以外の話題名でも名前のチップにする（#1673） ([#1674](https://github.com/kukuri-app/kukuri/pull/1674))

### Other

- リリースの版を 0.4.3 にする ([#1685](https://github.com/kukuri-app/kukuri/pull/1685))
- カスタムリアクションとそのセットの公開を法務文書へ反映し、Legal bundle を版 10 にする（#1232 AC-5） ([#1683](https://github.com/kukuri-app/kukuri/pull/1683))
- 0.4.2 preview の公開と LP・運用記録を完了する（#1657 AC-4） ([#1664](https://github.com/kukuri-app/kukuri/pull/1664))
- 一覧鍵を含む build で検索 capability の起動を確認する（#1657 AC-1）

## [v0.4.2-preview.1] - 2026-10-08

### Features

- kukuri の補助 index と署名一覧を GCP VM に配置する（#1657 AC-2）
- 公開 blob 発見の一覧鍵を組み込み 0.4.2 の配布候補を準備する（#1657 AC-1）
- Web が Community Node の検索で公開 blob の保持端末を見つけて取得する（#1632 AC-6） ([#1656](https://github.com/kukuri-app/kukuri/pull/1656))
- Community Node の公開 blob の保持端末の検索と補助 index の process（#1632 AC-5） ([#1653](https://github.com/kukuri-app/kukuri/pull/1653))
- 公開 blob の DHT 発見を native の取得・告知へ接続する（#1632 AC-4） ([#1651](https://github.com/kukuri-app/kukuri/pull/1651))
- 本人の端末どうしでプロフィールを和集合に同期する ([#1650](https://github.com/kukuri-app/kukuri/pull/1650), [#1652](https://github.com/kukuri-app/kukuri/pull/1652))
- 新着を常に反映する Flow モードを追加 ([#1647](https://github.com/kukuri-app/kukuri/pull/1647), [#1649](https://github.com/kukuri-app/kukuri/pull/1649))

### Fixes

- COS host firewall で補助 index の直接 UDP を許可する
- 最適化しない build でも str0m の再帰が 2 MiB の thread を溢れないようにする（#1658） ([#1659](https://github.com/kukuri-app/kukuri/pull/1659))
- relay の path の検証前に WebRTC の経路を失っても転送を止めない（#1654） ([#1655](https://github.com/kukuri-app/kukuri/pull/1655))
- v0.4.0 より前の profile を移行・同期に載せる ([#1646](https://github.com/kukuri-app/kukuri/pull/1646), [#1648](https://github.com/kukuri-app/kukuri/pull/1648))
- 経路のみの切替を差分診断へ反映 ([#1644](https://github.com/kukuri-app/kukuri/pull/1644))
- avoid claiming a direct path on NeighborUp ([#1643](https://github.com/kukuri-app/kukuri/pull/1643))
- avoid long waits for missing author profiles ([#1639](https://github.com/kukuri-app/kukuri/pull/1639))

### Other

- 一覧鍵を含む build で検索 capability の起動を確認する（#1657 AC-1）
- 実ブラウザの E2E の自分の表示名の変更を、欄を押して全選択から打つ形にし、今の名前に打った名前がつながらないようにする (#1660 AC-1) ([#1661](https://github.com/kukuri-app/kukuri/pull/1661))
- Android Web E2E の不安定な判定と押下を修正 ([#1641](https://github.com/kukuri-app/kukuri/pull/1641))
- セッションの後片付けを skill 化する ([#1645](https://github.com/kukuri-app/kukuri/pull/1645))
- wait for account reload and node consent before settings ([#1610](https://github.com/kukuri-app/kukuri/pull/1610), [#1642](https://github.com/kukuri-app/kukuri/pull/1642))
- Safari lifecycle の表示待ちと確認先の列を修正 ([#1640](https://github.com/kukuri-app/kukuri/pull/1640))
- 0.4.1 の CHANGELOG・LP・公開記録を同期 ([#1636](https://github.com/kukuri-app/kukuri/pull/1636))

## [v0.4.1-preview.2] - 2026-10-06

### Features

- Web 版の「別の端末から移す」で移行元の QR をカメラで読み、リンクを入力欄へ入れる (#1628 AC-1) ([#1631](https://github.com/kukuri-app/kukuri/pull/1631))
- 設定の「アカウント」に「アカウントを別の端末へ移す」ボタンを置く (#1629 AC-1) ([#1630](https://github.com/kukuri-app/kukuri/pull/1630))
- operator-config の deploy から CORS の許可 origin を tfvars へ書き出す (#1615 AC-1) ([#1618](https://github.com/kukuri-app/kukuri/pull/1618))

### Fixes

- 起動直後の Timeline の先頭ページを provider の応答で待たせない ([#1624](https://github.com/kukuri-app/kukuri/pull/1624), [#1626](https://github.com/kukuri-app/kukuri/pull/1626))
- expire stale blob providers per hash (#1594 AC-1) ([#1625](https://github.com/kukuri-app/kukuri/pull/1625))
- 保存済みの署名付きプロフィールを個別に中継する ([#1619](https://github.com/kukuri-app/kukuri/pull/1619), [#1622](https://github.com/kukuri-app/kukuri/pull/1622))
- web版に favicon を付ける (#1620 AC-1) ([#1623](https://github.com/kukuri-app/kukuri/pull/1623))

### Other

- sync third-party notices for 0.4.1 release ([#1635](https://github.com/kukuri-app/kukuri/pull/1635))
- bump release version to 0.4.1 ([#1634](https://github.com/kukuri-app/kukuri/pull/1634))
- DHT発見導入の影響調査を記録 ([#1632](https://github.com/kukuri-app/kukuri/pull/1632), [#1633](https://github.com/kukuri-app/kukuri/pull/1633))
- bound public blob provider discovery PoC (#1596 AC-1) ([#1627](https://github.com/kukuri-app/kukuri/pull/1627))
- v0.4.0-preview.1 の公開の記録・CHANGELOG・LP（ブラウザ版への導線）(#1615 AC-7) ([#1621](https://github.com/kukuri-app/kukuri/pull/1621))

## [v0.4.0-preview.1] - 2026-10-06

### Features

- 本番の Community Node が Web クライアントの origin に CORS で応答できるようにする (#1615 AC-1) ([#1616](https://github.com/kukuri-app/kukuri/pull/1616))
- 移行の画面に移るもの・移らないものを示し、移行先は完了の画面から切り替える。設定の「アカウント」で別の端末で使う 3 つの方法を比べる (#1211 AC-5) ([#1574](https://github.com/kukuri-app/kukuri/pull/1574))
- 自分のフォロー・ブロックを移行の必須 bundle と本人の端末間の同期で運び、移行先で相互フォローの相手と DM を使えるようにする (#1211 AC-6) ([#1569](https://github.com/kukuri-app/kukuri/pull/1569))
- 表示の照合で、既に表示している投稿の reaction を provider から新しい側 8 件まで読み直す (#1567 AC-2) ([#1572](https://github.com/kukuri-app/kukuri/pull/1572))
- gossip hint の受付を lease ごとに窓あたり 32 件に抑え、溢れた窓の終わりに 1 回読み直す (#1567 AC-1) ([#1568](https://github.com/kukuri-app/kukuri/pull/1568))
- native から Web への移行の後の本人の端末間の同期を実ブラウザの E2E で確かめ、設定の「アカウント」に同期の状態を出す (#1220 AC-3b) ([#1561](https://github.com/kukuri-app/kukuri/pull/1561))
- 別の端末から、所有者の端末で稼働中の隣の Dome へ遷移できるようにする (#1527 AC-2) ([#1560](https://github.com/kukuri-app/kukuri/pull/1560))
- Web クライアントの配信の artifact を作る command と配信の header、手順書、Web の外部送信の説明を足す (#1220 AC-6) ([#1557](https://github.com/kukuri-app/kukuri/pull/1557))
- 所有者の端末で稼働中の Dome へ別の端末から入室・滞在・再同期できる P2P の session 経路を設ける (#1527 AC-1) ([#1540](https://github.com/kukuri-app/kukuri/pull/1540))
- 新しいアクセスの配布を別の端末が行うために保留した操作を、共通の 1 つのダイアログで示す (#1220 AC-3a) ([#1547](https://github.com/kukuri-app/kukuri/pull/1547))
- 参加者の記録と参加者との follow を本人の端末間で同期し、鍵の配布・遅れた grant・送るときの確認に使う (#1219 AC-5) ([#1541](https://github.com/kukuri-app/kukuri/pull/1541))
- relay の host で STUN（binding だけ）を relay とは別の process（cn-stun）で動かす (#1483 AC-1) ([#1537](https://github.com/kukuri-app/kukuri/pull/1537))
- 移行で選んだ範囲の投稿の履歴を確認済みの接続で page ごとに送り、止めても必須の移行は完了のままにする (#1211 AC-3) ([#1532](https://github.com/kukuri-app/kukuri/pull/1532))
- 共有リンクの作成と新しいアクセスの配布を行う端末を、明示の操作と backup からの復元で移す (#1219 AC-4) ([#1522](https://github.com/kukuri-app/kukuri/pull/1522))
- Web の保存の永続化の要求と、サイトデータが消えたときの戻し方の案内 (#1217 AC-5) ([#1519](https://github.com/kukuri-app/kukuri/pull/1519))
- Web で runtime を動かす tab を 1 つにする lock と引継ぎ (#1217 AC-4) ([#1516](https://github.com/kukuri-app/kukuri/pull/1516))
- 確認済みの接続で必須 bundle を送り、移行先の保存の確定の後だけ移行を完了にする (#1211 AC-2) ([#1515](https://github.com/kukuri-app/kukuri/pull/1515))
- リンクプレビューの record の読取りに上限を置き、読んだ参加者が中継する (#1220 AC-2f) ([#1513](https://github.com/kukuri-app/kukuri/pull/1513))
- Web の build mode・entry と、実ブラウザの主要導線の試験 (#1220 AC-2a) ([#1503](https://github.com/kukuri-app/kukuri/pull/1503))
- 担当でない端末の鍵更新を伴う共有・投稿を account 同期で担当へ依頼し、担当の記録を本人の端末へ届ける (#1219 AC-3) ([#1509](https://github.com/kukuri-app/kukuri/pull/1509))
- browser の lifecycle を共通の復帰の入口へつなぐ (#1217 AC-3) ([#1507](https://github.com/kukuri-app/kukuri/pull/1507))
- 投稿者が書くリンクプレビューの record を Web で読む (#1220 AC-2d) ([#1502](https://github.com/kukuri-app/kukuri/pull/1502))
- account 同期の同期・未同期の状態を通信状態に載せる (#1218 AC-6a) ([#1504](https://github.com/kukuri-app/kukuri/pull/1504))
- Web の鍵の export・import の argon2id の導出を Dedicated Worker で行う (#1220 AC-2c) ([#1501](https://github.com/kukuri-app/kukuri/pull/1501))
- cn-user-api に許可する origin の設定を足し、Web クライアントに CORS で応答する (#1220 AC-2e) ([#1500](https://github.com/kukuri-app/kukuri/pull/1500))
- command の dispatch 表・Web の runtime・JS API (#1214 AC-5) ([#1499](https://github.com/kukuri-app/kukuri/pull/1499))
- 本人の別の端末から account 同期の差分を取得し、送り直す (#1218 AC-5b) ([#1497](https://github.com/kukuri-app/kukuri/pull/1497))
- private channel の鍵更新を予約した世代へ束縛し、配布を 128 件の page で背景に進める (#1219 AC-2) ([#1495](https://github.com/kukuri-app/kukuri/pull/1495))
- 参加中の channel の一覧の続きを画面の「さらに表示」と CLI の cursor で読む (#1218 AC-4d) ([#1491](https://github.com/kukuri-app/kukuri/pull/1491))
- private channel の参加・世代の鍵・担当を本人の端末間で書き、merge する (#1218 AC-4c) ([#1489](https://github.com/kukuri-app/kukuri/pull/1489))
- Web の鍵・設定・projection・peer の接続候補を IndexedDB に保存し、reload の後も同じ EndpointId で再開する (#1217 AC-2) ([#1490](https://github.com/kukuri-app/kukuri/pull/1490))
- private channel の鍵を (channel, epoch) ごとの行へ移し、全件の読み書きと走査をやめる (#1218 AC-4b) ([#1488](https://github.com/kukuri-app/kukuri/pull/1488))
- QR・専用リンクから両端末の確認を経て移行の接続を確立する (#1211 AC-1) ([#1485](https://github.com/kukuri-app/kukuri/pull/1485))
- Web の本人の record の保存失敗・破損・失効した capability を偽の成功にしない (#1216 W3 AC-4) ([#1486](https://github.com/kukuri-app/kukuri/pull/1486))
- reload の後は保存した自分の record を有界に読み、閉じた replica を memory から外す (#1216 W3 AC-3) ([#1484](https://github.com/kukuri-app/kukuri/pull/1484))
- Web の docs の record を IndexedDB に保護・保持し、native と有界な reader で読み合う (#1216 W3 AC-2) ([#1480](https://github.com/kukuri-app/kukuri/pull/1480))
- private channel の鍵更新を担当端末だけに限り、同じ account の別端末は保留を返す (#1219 AC-1) ([#1475](https://github.com/kukuri-app/kukuri/pull/1475))
- WebRTC の接続交渉を需要の接続へつなぎ、停止・再開・診断・STUN を統合する (#1422 AC-2) ([#1479](https://github.com/kukuri-app/kukuri/pull/1479))
- 本人の端末間で profile と表示例外を決定的な勝者で merge する (#1218 AC-3) ([#1477](https://github.com/kukuri-app/kukuri/pull/1477))
- Web の blob の書込み失敗・破損を完成と扱わず、保護した内容を残して固定の窓で回収する (#1215 W2 AC-3) ([#1476](https://github.com/kukuri-app/kukuri/pull/1476))
- Web の blob の保護・cache を IndexedDB に置き、reload の後も native と送受信する (#1215 W2 AC-2) ([#1474](https://github.com/kukuri-app/kukuri/pull/1474))
- account 同期の種別を起動・hint・rendezvous・診断・読み出し・CN の索引で分類する (#1218 AC-2) ([#1473](https://github.com/kukuri-app/kukuri/pull/1473))
- native だけの処理を target で分け、desktop-runtime を wasm32 で build する (#1214 AC-4d) ([#1470](https://github.com/kukuri-app/kukuri/pull/1470))
- 鍵・設定・同意の保存を非同期の保存 trait 1 つへ切り出す (#1214 AC-4c) ([#1469](https://github.com/kukuri-app/kukuri/pull/1469))
- desktop-runtime の account の保存を AccountStore の trait object で持つ (#1214 AC-4b) ([#1468](https://github.com/kukuri-app/kukuri/pull/1468))
- 時刻と task を n0_future・web_time へ置き換え、app-api と metaverse-host を wasm32 で build する (#1214 AC-4a) ([#1467](https://github.com/kukuri-app/kukuri/pull/1467))
- host に runtime の世代を持たせ、event・command の受付・WebRTC の交渉を世代へ結ぶ (#1214 AC-3) ([#1466](https://github.com/kukuri-app/kukuri/pull/1466))
- 専用 ALPN で WebRTC の接続交渉を行い、共有 Endpoint へ custom path を足す (#1422 AC-1) ([#1465](https://github.com/kukuri-app/kukuri/pull/1465))
- 共用 crate を wasm32 で build し、ブラウザと native の有界な読み出しを通す (#1214 AC-2) ([#1464](https://github.com/kukuri-app/kukuri/pull/1464))
- 本人の端末間の account 同期の namespace と封を追加する (#1218 AC-1) ([#1462](https://github.com/kukuri-app/kukuri/pull/1462))
- QUIC over WebRTC DataChannel の transport crate を追加する (#1421 AC-2) ([#1457](https://github.com/kukuri-app/kukuri/pull/1457))

### Fixes

- Dome の読取りと session の入力は鍵更新の判定を通さない (#1552 AC-1) ([#1602](https://github.com/kukuri-app/kukuri/pull/1602))
- E2Eで添付の準備完了を待ってから投稿とDMを送る ([#1577](https://github.com/kukuri-app/kukuri/pull/1577), [#1592](https://github.com/kukuri-app/kukuri/pull/1592))
- 電話幅の列と固定操作の位置を揃える ([#1588](https://github.com/kukuri-app/kukuri/pull/1588), [#1589](https://github.com/kukuri-app/kukuri/pull/1589))
- WebRTC の経路だけを失った間も、担当でない端末の鍵の更新の依頼を 15 秒の待ちに収める (#1220 AC-3c2) ([#1582](https://github.com/kukuri-app/kukuri/pull/1582))
- 接続の直後に WebRTC の経路を失っても、同じ相手との通信を止めない (#1571 AC-1) ([#1581](https://github.com/kukuri-app/kukuri/pull/1581))
- native の DataChannel を寿命 1 ms の部分的な信頼性にし、SACK と FORWARD-TSN の往復を止める (#1575 AC-1) ([#1576](https://github.com/kukuri-app/kukuri/pull/1576))
- WebRTC の経路を失ったら、閉じた custom path を iroh がすぐ閉じて relay で続ける (#1482 AC-1) ([#1543](https://github.com/kukuri-app/kukuri/pull/1543))
- gossip が、同じ EndpointId で作り直した相手の Join に Neighbor を返すようにし、作り直しの後の受信の offer が届くようにする (#1549 AC-1) ([#1565](https://github.com/kukuri-app/kukuri/pull/1565))
- channel を選ぶ操作で、選んだ channel の列を公開の列を経由せずに active にする (#1528 AC-1) ([#1564](https://github.com/kukuri-app/kukuri/pull/1564))
- 受信した Dome host の heartbeat の台帳を、最後の heartbeat から 30 秒で消して context ごとに 100 件で抑える (#1553 AC-1) ([#1556](https://github.com/kukuri-app/kukuri/pull/1556))
- 移行の後、同意した移行先と移行元の間で設定・鍵の更新が自動で届くよう、account 同期の読み出しと merge を直す (#1211 AC-4) ([#1558](https://github.com/kukuri-app/kukuri/pull/1558))
- 設定の「Add Node」を欄の label で包まず、読み上げ名を button の文字にし、欄の文字の押下でノードを足さない (#1550 AC-1) ([#1551](https://github.com/kukuri-app/kukuri/pull/1551))
- 観測提供の状態を、設定の描画し直しのたびではなく、設定を開いたときにだけ照会する (#1546 AC-1) ([#1548](https://github.com/kukuri-app/kukuri/pull/1548))
- 参加・作成の直後に、その前に始まった topic の読込みが参加済み一覧を古い一覧で置き換えないようにする (#1544 AC-1) ([#1545](https://github.com/kukuri-app/kukuri/pull/1545))
- Dome の session runtime の参加者ごとの台帳を退出と拒否で消し、参加者の上限で抑える (#1538 AC-1) ([#1539](https://github.com/kukuri-app/kukuri/pull/1539))
- 相手の follow の edge を保存したら event を出し、開いている profile と会話の列を読み直す (#1521 AC-1b) ([#1542](https://github.com/kukuri-app/kukuri/pull/1542))
- Control Center「場所」の参加中チャンネルの行で、名前が長くても操作が枠内に収まるようにする (#1533 AC-1) ([#1534](https://github.com/kukuri-app/kukuri/pull/1534))
- Windows の bin の main thread stack reserve を 8 MiB にし、command の future の大きさに上限 test を置く (#1526 AC-1) ([#1529](https://github.com/kukuri-app/kukuri/pull/1529))
- 信頼評価の観測の状態を account DB の行へ移し、pass ごとの丸ごとの読み書きをやめる (#1510 AC-1) ([#1523](https://github.com/kukuri-app/kukuri/pull/1523))
- 送れなかった follow の offer を約 2 分送り直し、開いたままの profile も相互 follow にする (#1521 AC-1) ([#1525](https://github.com/kukuri-app/kukuri/pull/1525))
- private channel を作成・選択して dialog を閉じた後も channel の列を active のまま画面に残す (#1517 AC-1) ([#1524](https://github.com/kukuri-app/kukuri/pull/1524))
- 反応の hint に送った時刻を載せ、同じ投稿への続く反応を gossip が重複として落とさないようにする (#1505 AC-1) ([#1506](https://github.com/kukuri-app/kukuri/pull/1506))
- 参加中の channel の一覧で、幅狭の「設定と共有」を icon にして channel 名を読めるようにする (#1492 AC-1) ([#1496](https://github.com/kukuri-app/kukuri/pull/1496))
- Web の IndexedDB の鍵の行の store に、参加の行の無い世代の鍵を足す操作を実装する (#1218 AC-4c) ([#1494](https://github.com/kukuri-app/kukuri/pull/1494))
- 保護移行の writer 切替を 1 文にし、別の接続の書込み中は待つ (#1471 AC-1) ([#1472](https://github.com/kukuri-app/kukuri/pull/1472))
- 相手への private の docs の応答を、要求の replica に登録した capability で確かめる (#1459 AC-1) ([#1460](https://github.com/kukuri-app/kukuri/pull/1460))
- 退出 test で owner を止める前に参加 record の ACK の到着を待つ (#1447 AC-1) ([#1448](https://github.com/kukuri-app/kukuri/pull/1448))
- 受け手が同時に処理する offer の数を送り手の binding 応答の上限に合わせる (#1426 AC-1) ([#1446](https://github.com/kukuri-app/kukuri/pull/1446))
- 続きの読み込みと定期更新で Timeline の既存の行を描き直さない (#1425 AC-1) ([#1445](https://github.com/kukuri-app/kukuri/pull/1445))
- 作者に届かなくても、手元に持つ作者の公開投稿をプロフィールに出す (#1442 AC-1) ([#1443](https://github.com/kukuri-app/kukuri/pull/1443))
- 作者列の最初の取得が終わるまで「公開投稿がありません」と出さない (#1442 AC-2) ([#1444](https://github.com/kukuri-app/kukuri/pull/1444))

### Other

- 版を 0.4.0 にする (#1615 AC-2) ([#1617](https://github.com/kukuri-app/kukuri/pull/1617))
- Android の行の PASS の本数と、#1605 の前との比較の記載を直す (#1213 AC-7) ([#1614](https://github.com/kukuri-app/kukuri/pull/1614))
- main への統合の前の 4 つのブラウザの再確認を matrix に反映する (#1213 AC-7) ([#1613](https://github.com/kukuri-app/kukuri/pull/1613))
- Web の E2E の fixture で STUN に応答する ([#1590](https://github.com/kukuri-app/kukuri/pull/1590), [#1605](https://github.com/kukuri-app/kukuri/pull/1605))
- dome_session の整形 ([#1213](https://github.com/kukuri-app/kukuri/pull/1213))
- 固定 workload の実測と native の回帰の確認を記録する (#1220 AC-5d3) ([#1598](https://github.com/kukuri-app/kukuri/pull/1598))
- Safariの別タブ・windowのruntime排他を検証 ([#1586](https://github.com/kukuri-app/kukuri/pull/1586), [#1591](https://github.com/kukuri-app/kukuri/pull/1591))
- 別の機器との同じ LAN の直接経路の判定を記録する (#1220 AC-5d2) ([#1597](https://github.com/kukuri-app/kukuri/pull/1597))
- 4 つのブラウザの実ブラウザの E2E の実結果を matrix に記録する (#1220 AC-5d1) ([#1593](https://github.com/kukuri-app/kukuri/pull/1593))
- 実ブラウザの E2E を Android の emulator の Chrome でも回す (#1220 AC-5c) ([#1587](https://github.com/kukuri-app/kukuri/pull/1587))
- 実ブラウザの E2E を macOS の Safari でも回し、試験の script を fixture の配信から入れる (#1220 AC-5b) ([#1584](https://github.com/kukuri-app/kukuri/pull/1584))
- 実ブラウザの E2E の投稿を、欄を押して全選択から打つ形にし、復元した下書きが本文につながらないようにする (#1579 AC-1) ([#1580](https://github.com/kukuri-app/kukuri/pull/1580))
- 同じアカウントの Web どうしの担当の不在と引継ぎ、Web → Web の移行、移行の途中の WebRTC の経路の喪失を実ブラウザの E2E で確かめる (#1220 AC-3c1) ([#1578](https://github.com/kukuri-app/kukuri/pull/1578))
- 実ブラウザの E2E を Firefox で夜間と手動で回す (#1220 AC-5a) ([#1573](https://github.com/kukuri-app/kukuri/pull/1573))
- 設定の drawer が開き終わってから section を押し、滑り込みの途中の押下が外れないようにする (#1559 AC-2) ([#1570](https://github.com/kukuri-app/kukuri/pull/1570))
- 実ブラウザの E2E を独立した scenario に分け、CI は build の job と scenario ごとの並列の job で回す (#1559 AC-1) ([#1566](https://github.com/kukuri-app/kukuri/pull/1566))
- 作り直しの 1 page の試験の docs が account の replica の読取りだけを数え、参加した channel の購読の task の背景の読取りを数えないようにする (#1562 AC-1) ([#1563](https://github.com/kukuri-app/kukuri/pull/1563))
- reload・終了・凍結・回線全断・WebRTC の経路だけの喪失からの復帰と、旧 state の再送を実ブラウザの E2E で確かめる (#1220 AC-4) ([#1530](https://github.com/kukuri-app/kukuri/pull/1530))
- 相互 follow を profile を開いたまま待ち、閉じて開き直す回避策を E2E から外す (#1220 AC-2h) ([#1555](https://github.com/kukuri-app/kukuri/pull/1555))
- command の future の大きさの試験（#1529）に、統合 branch だけにある command を載せる
- 所有者の端末で稼働中の Dome へ別の端末から入る P2P の session 経路を定める (#1527 AC-0) ([#1535](https://github.com/kukuri-app/kukuri/pull/1535))
- 観測の提供の状態と送信待ちの同じ操作列を SQLite と IndexedDB の実装で確かめる (#1510 の取込み)
- リンクプレビューの card と画像・URL だけの表示・投稿者が居ないときの中継を実ブラウザの E2E で確かめる (#1220 AC-2g) ([#1520](https://github.com/kukuri-app/kukuri/pull/1520))
- DM・private channel・主要な設定を実ブラウザの E2E で確かめる (#1220 AC-2b) ([#1518](https://github.com/kukuri-app/kukuri/pull/1518))
- Dome の layout commit を join の前に確かめ、呼び出しの間隔で committed になる flake を直す (#1511 AC-1) ([#1512](https://github.com/kukuri-app/kukuri/pull/1512))
- DB を消して再起動する試験 4 件を戻し、手元の replica からの作り直しを確かめる (#1218 AC-5c) ([#1498](https://github.com/kukuri-app/kukuri/pull/1498))
- account 同期の差分の取得・送り直し・DB を失ったときの作り直しを ADR に固定する (#1218 AC-5a) ([#1493](https://github.com/kukuri-app/kukuri/pull/1493))
- 受入条件・利用者体験・後方互換性に関わる変更はユーザーの判断を確認してから進める規則を AGENTS.md に足す ([#1487](https://github.com/kukuri-app/kukuri/pull/1487))
- private channel の鍵を行ごとに保存する形と channel の item の merge の規則を ADR に固定する (#1218 AC-4a) ([#1481](https://github.com/kukuri-app/kukuri/pull/1481))
- Web の blob-service が取得 gate の前提を保つことを確かめ、上流 iroh-blobs の更新手順を書く (#1215 W2 AC-4) ([#1478](https://github.com/kukuri-app/kukuri/pull/1478))
- Web の entry・capability・配信条件と検証環境を ADR 0060 に固定する (#1220 AC-1) ([#1461](https://github.com/kukuri-app/kukuri/pull/1461))
- Web の鍵・設定・projection の保存と複数 tab・lifecycle を ADR 0059 に固定する (#1217 AC-1) ([#1458](https://github.com/kukuri-app/kukuri/pull/1458))
- Web の docs の自分の record の保存を ADR 0058 §7 に固定する (#1216 AC-1) ([#1456](https://github.com/kukuri-app/kukuri/pull/1456))
- Web の blob の保存を ADR 0058 に固定する (#1215 AC-1) ([#1455](https://github.com/kukuri-app/kukuri/pull/1455))
- QUIC over WebRTC DataChannel の transport を ADR 0057 に固定する (#1421 AC-1) ([#1454](https://github.com/kukuri-app/kukuri/pull/1454))
- Web クライアントの実行境界を ADR 0056 に固定する (#1214 AC-1) ([#1453](https://github.com/kukuri-app/kukuri/pull/1453))
- Rust の依存と toolchain を最新へ更新する (#1450 AC-1) ([#1452](https://github.com/kukuri-app/kukuri/pull/1452))
- TypeScript / npm 依存を最新へ更新する (#1449 AC-1) ([#1451](https://github.com/kukuri-app/kukuri/pull/1451))
- v0.3.3-preview.1 の公開を CHANGELOG・LP・作業記録へ反映する (#1439 AC-5) ([#1441](https://github.com/kukuri-app/kukuri/pull/1441))

## [v0.3.3-preview.1] - 2026-09-30

### Features

- 添付の表示取得で、同じ topic の参加者を取得候補に入れる (#1419 AC-2) ([#1438](https://github.com/kukuri-app/kukuri/pull/1438))
- 同じ topic の参加者が作者の profile を中継し、作者がオフラインでも表示名とアバターを出す (#1419 AC-1) ([#1437](https://github.com/kukuri-app/kukuri/pull/1437))
- 成人向け表示 ON の間に表示した添付を cache に置き、OFF に戻したら消す (#1419 AC-4) ([#1435](https://github.com/kukuri-app/kukuri/pull/1435))
- 同意済みのコミュニティノードの規約更新を検知したら再同意のモーダルを開く (#1420 AC-1) ([#1433](https://github.com/kukuri-app/kukuri/pull/1433))

### Fixes

- 起動時の CN 設定の読込を、CN 状態の反映の完了を待たずに始める (#1416 AC-1) ([#1436](https://github.com/kukuri-app/kukuri/pull/1436))
- 手元にある画像を他の取得待ちに並べず、画面外へ出た直近の画像を残す (#1419 AC-3) ([#1434](https://github.com/kukuri-app/kukuri/pull/1434))
- AppImage の runtime を上流の continuous から取らず、審査済みの版を固定して使う (#1431 AC-1) ([#1432](https://github.com/kukuri-app/kukuri/pull/1432))
- 世代の確認の間も読み取りを poll し、参加状態の lock の自己デッドロックを防ぐ (#1423 AC-1) ([#1424](https://github.com/kukuri-app/kukuri/pull/1424))

### Other

- v0.3.3-preview.1 の版へ同期する (#1439 AC-1) ([#1440](https://github.com/kukuri-app/kukuri/pull/1440))
- CN image を標準 runner と BuildKit の cache へ移し、依存の layer を分ける (#1413 AC-3) ([#1430](https://github.com/kukuri-app/kukuri/pull/1430))
- package・release を標準 runner へ移し、PR の古い run を取り消す (#1413 AC-2) ([#1429](https://github.com/kukuri-app/kukuri/pull/1429))
- 通常 CI を標準 runner と GitHub Actions cache へ移し、重複 job を削る (#1413 AC-1) ([#1418](https://github.com/kukuri-app/kukuri/pull/1418))
- 起動直後に advisory 照会の確定を待つ query に上限を渡す (#1414 AC-1) ([#1415](https://github.com/kukuri-app/kukuri/pull/1415))
- v0.3.2-preview.1 の公開を CHANGELOG・LP・作業記録へ反映する (#1410 AC-5) ([#1412](https://github.com/kukuri-app/kukuri/pull/1412))

## [v0.3.2-preview.1] - 2026-09-28

### Features

- 読取りで作られた空の namespace を、旧 store の退役の後に小分けに回収する (#1407 AC-2) ([#1409](https://github.com/kukuri-app/kukuri/pull/1409))
- 検証済みの live session・game room を保持し、同じ topic の参加者へ提供する (#1395 AC-7) ([#1402](https://github.com/kukuri-app/kukuri/pull/1402))
- 検証済みの reaction を保持し、同じ topic の参加者へ提供する (#1395 AC-6) ([#1401](https://github.com/kukuri-app/kukuri/pull/1401))
- 検証済みの取り下げを保持し、同じ topic の参加者へ提供する (#1395 AC-5) ([#1400](https://github.com/kukuri-app/kukuri/pull/1400))
- 本文・添付の取得で、見出しを返した peer を候補に入れる (#1395 AC-2) ([#1399](https://github.com/kukuri-app/kukuri/pull/1399))
- 本人がオフラインでも同じ topic の参加者から一覧を取得する (#1395 AC-1) ([#1398](https://github.com/kukuri-app/kukuri/pull/1398))
- remote cache の上限を 3 GiB にする (#1395 AC-4) ([#1397](https://github.com/kukuri-app/kukuri/pull/1397))

### Fixes

- docs の読取りで手元に無い namespace を作らない (#1407 AC-1) ([#1408](https://github.com/kukuri-app/kukuri/pull/1408))
- CN 検索結果で解決できなかった投稿の文言を「投稿を取得できませんでした。」にする (#1403 AC-2) ([#1405](https://github.com/kukuri-app/kukuri/pull/1405))
- CN 検索結果の解決で、手元の投稿を先に確かめ、remote 読取りを group 共通にして候補ごとに打ち切る (#1403 AC-1) ([#1404](https://github.com/kukuri-app/kukuri/pull/1404))

### Other

- v0.3.2-preview.1 の版へ同期する (#1410 AC-1) ([#1411](https://github.com/kukuri-app/kukuri/pull/1411))
- 同じ topic の参加者による中継を ADR 0054・0055 に記録する (#1395 AC-3) ([#1406](https://github.com/kukuri-app/kukuri/pull/1406))
- v0.3.1-preview.1 の公開を CHANGELOG・LP・作業記録へ反映する (#1390 AC-4) ([#1394](https://github.com/kukuri-app/kukuri/pull/1394))

## [v0.3.1-preview.1] - 2026-09-28

### Fixes

- 本文の取り直しを node の取得の予算で待ち、応答しない peer の後ろの peer から取る (#1390 AC-1) ([#1391](https://github.com/kukuri-app/kukuri/pull/1391))

### Other

- v0.3.1-preview.1 の版へ同期する (#1390 AC-2) ([#1392](https://github.com/kukuri-app/kukuri/pull/1392))
- Store 版 1.3.0.0 の審査通過を記録する ([#1384](https://github.com/kukuri-app/kukuri/pull/1384), [#1389](https://github.com/kukuri-app/kukuri/pull/1389))
- v0.3.0-preview.2 の公開を CHANGELOG・LP・作業記録へ反映する (#1384 AC-7) ([#1388](https://github.com/kukuri-app/kukuri/pull/1388))

## [v0.3.0-preview.2] - 2026-09-28

### Features

- 通信状態を件数と稼働 topic にし、変化点の差分を push、詳細は有限ページで読む (#1221 R2-D) ([#1380](https://github.com/kukuri-app/kukuri/pull/1380))
- CN session の期限処理と回復を node ごとの差分にする (#1221 R2-B) ([#1379](https://github.com/kukuri-app/kukuri/pull/1379))
- 旧 iroh store を新しい store と cache へ有界に移して退役させる (#1221 R5-I) ([#1378](https://github.com/kukuri-app/kukuri/pull/1378))
- 新形式の writer へ切り替え、旧 namespace 定常同期・暫定 selector・旧 queue を撤去する (#1221 R5-H) ([#1375](https://github.com/kukuri-app/kukuri/pull/1375))
- client の購読を 64 件の scope lease へ統合する (#1221 R2-C) ([#1374](https://github.com/kukuri-app/kukuri/pull/1374))
- 旧 iroh-data の本人データを保護所有先へ移し、backup を切り替える (#1221 R5-G) ([#1372](https://github.com/kukuri-app/kukuri/pull/1372))
- CN の保存物を受入下限・容量・保持期間で有界に回収する (#1221 R5-F) ([#1371](https://github.com/kukuri-app/kukuri/pull/1371))
- CN の手動全件取込を撤去し、関係解析を 2 者間のアクションの差分へ移す (#1221 R5-E) ([#1370](https://github.com/kukuri-app/kukuri/pull/1370))
- DM を account route だけにし、関係を edge から読むときに求める (R4-D) ([#1369](https://github.com/kukuri-app/kukuri/pull/1369))
- private の通知を account の受信経路へ接続する (R4-C) ([#1368](https://github.com/kukuri-app/kukuri/pull/1368))
- complete R5-B bounded client bucket readers
- deliver public notifications on account receive route
- 投稿カードのフォロー関係・公開範囲をアイコン表示にする ([#1345](https://github.com/kukuri-app/kukuri/pull/1345))
- consolidate outbox retries under bounded account owner
- deliver protected outbox and ACK over account route
- resolve account receive endpoints from bounded peer windows
- 時間bucketと同期停止の基盤を追加 ([#1297](https://github.com/kukuri-app/kukuri/pull/1297))
- 遡った範囲でまだ取得できていない投稿の数を示し、その先へ進めるようにする ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1285](https://github.com/kukuri-app/kukuri/pull/1285))
- replica の購読が、event の取りこぼしと同期の区切りを受け取れるようにする ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1266](https://github.com/kukuri-app/kukuri/pull/1266))
- 更新がある間はコントロールクラスタから更新導線を出す ([#1231](https://github.com/kukuri-app/kukuri/pull/1231))
- add Microsoft Store MSIX packaging ([#1191](https://github.com/kukuri-app/kukuri/pull/1191))

### Fixes

- bundle 前に runner の既存 package をまとめて更新し、notices を更新する ([#1384](https://github.com/kukuri-app/kukuri/pull/1384), [#1387](https://github.com/kukuri-app/kukuri/pull/1387))
- 復元した非 active の DM の列を、選ぶ前に読み込む (#1221 R6-C) ([#1382](https://github.com/kukuri-app/kukuri/pull/1382))
- 動作確認の所見を補修する（表示名の背景読取り・profile 列の読み直し・CLI の policy_kind） (#1221 R6-B) ([#1381](https://github.com/kukuri-app/kukuri/pull/1381))
- gossip の接続の切替後に topic の stream を開き直し、相手の idle 回収で切られないようにする ([#1376](https://github.com/kukuri-app/kukuri/pull/1376), [#1377](https://github.com/kukuri-app/kukuri/pull/1377))
- 深いスレッド返信のカード幅を保ち、カラム本文を横スクロールさせる ([#1373](https://github.com/kukuri-app/kukuri/pull/1373))
- preserve local session reads after shutdown
- keep legacy reply targets on local docs reader
- preserve bounded client reads across sources and gaps
- context action menu の再描画で矢印キーの focus を先頭へ戻さない ([#1365](https://github.com/kukuri-app/kukuri/pull/1365))
- use the latest intersection entry for media demand
- stop restarting live subscriptions for sessions without viewers
- close every pooled connection and take write locks first
- keep topic membership symmetric across warmup and restarts
- bound public offer queue and reject private source
- avoid remote cache write locks on fresh reads
- retain protected remote blobs across temporary pins
- sync due-index migration golden and version contracts
- bound legacy hint wait and sync migration contracts
- inline account offer references to avoid unbounded blobs
- fence receive lookup shutdown and invalidation races
- point Windows downloads to Microsoft Store
- verify session manifest hash before blob fetch ([#1261](https://github.com/kukuri-app/kukuri/pull/1261), [#1302](https://github.com/kukuri-app/kukuri/pull/1302))
- sessionの反映を到着イベントと表示要求に分離する ([#1262](https://github.com/kukuri-app/kukuri/pull/1262))
- prevent signed session state rollback
- Metaverse avatarの再取得を有限化する ([#1242](https://github.com/kukuri-app/kukuri/pull/1242))
- blobを投稿単位で再読み込みできるようにする ([#1284](https://github.com/kukuri-app/kukuri/pull/1284))
- live・game一覧をchannel別の上限付き取得にする ([#1292](https://github.com/kukuri-app/kukuri/pull/1292))
- 複数の channel をまたぐタイムライン(AllJoined scope)を CLI・API から閉じ、複数 channel のページ取得を無くす ([#1280](https://github.com/kukuri-app/kukuri/pull/1280), [#1291](https://github.com/kukuri-app/kukuri/pull/1291))
- iroh stack rebuild の shutdown 印を取消に強くする ([#1287](https://github.com/kukuri-app/kukuri/pull/1287))
- paginate profile timelines ([#1289](https://github.com/kukuri-app/kukuri/pull/1289))
- view の生成で返信先を docs から読まず、projection の読み書きと新着の受信も件数で数える (#1239, #1277) ([#1283](https://github.com/kukuri-app/kukuri/pull/1283))
- 利用者が必要としない同期・復旧(自分の replica の全件の読み出しと索引の補完)を外す ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1282](https://github.com/kukuri-app/kukuri/pull/1282))
- プロフィールのタイムラインを、author replica の索引からページの行だけを読む形にする ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1276](https://github.com/kukuri-app/kukuri/pull/1276))
- author 購読が replica を走査せず、docs author と key の組・上限つきの一覧で反映する ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1275](https://github.com/kukuri-app/kukuri/pull/1275))
- ページの取得を索引の範囲の読み出しにし、非表示の著者の読み飛ばしに上限を置く (#1239, #1274) ([#1270](https://github.com/kukuri-app/kukuri/pull/1270))
- カラム本文を一段内側で包むだけの枠を外す ([#1271](https://github.com/kukuri-app/kukuri/pull/1271), [#1273](https://github.com/kukuri-app/kukuri/pull/1273))
- live / game の一覧が replica を走査せず、全件走査の関数を削除する ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1268](https://github.com/kukuri-app/kukuri/pull/1268))
- 購読タスクが replica を走査せず、窓の追いつきで反映する ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1267](https://github.com/kukuri-app/kukuri/pull/1267))
- 照合が、上限で止まった索引の読み出しを読み継ぎ、thread もページの範囲だけを読む ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1265](https://github.com/kukuri-app/kukuri/pull/1265))
- アカウントの署名鍵から導出した docs author で、著者の投稿と取り下げを読む ([#1258](https://github.com/kukuri-app/kukuri/pull/1258), [#1264](https://github.com/kukuri-app/kukuri/pull/1264))
- タイムラインと thread の取得を、replica を走査しないページの範囲の照合へ置き換える (#1239, #1257) ([#1247](https://github.com/kukuri-app/kukuri/pull/1247))
- reaction・live session・game room の反映で署名と replica を確かめる ([#1252](https://github.com/kukuri-app/kukuri/pull/1252), [#1259](https://github.com/kukuri-app/kukuri/pull/1259))
- 取り下げの key に不正な record が先にあっても、著者の取り下げを反映する ([#1250](https://github.com/kukuri-app/kukuri/pull/1250), [#1256](https://github.com/kukuri-app/kukuri/pull/1256))
- UTF-8 でない key の entry で docs の読み出し全体を失敗させない ([#1253](https://github.com/kukuri-app/kukuri/pull/1253), [#1255](https://github.com/kukuri-app/kukuri/pull/1255))
- 投稿の反映で署名つき envelope と replica を確かめる ([#1248](https://github.com/kukuri-app/kukuri/pull/1248), [#1249](https://github.com/kukuri-app/kukuri/pull/1249))
- 利用者の操作と view の生成で replica を走査しない ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1246](https://github.com/kukuri-app/kukuri/pull/1246))
- docs の読み出しを key の索引で行い、上限つきの読み出しを追加する ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1245](https://github.com/kukuri-app/kukuri/pull/1245))
- コミュニティノード規約を文書の役割どおりの場所で提示する ([#1238](https://github.com/kukuri-app/kukuri/pull/1238))
- 欠損した本文や変化の無い replica で全件走査を繰り返さない ([#1225](https://github.com/kukuri-app/kukuri/pull/1225), [#1237](https://github.com/kukuri-app/kukuri/pull/1237))
- runtime image に対象 binary だけを含める ([#1138](https://github.com/kukuri-app/kukuri/pull/1138), [#1236](https://github.com/kukuri-app/kukuri/pull/1236))
- 同一ログ行の連発を集約し、relay 受信集中後の復旧を test で固定する ([#1206](https://github.com/kukuri-app/kukuri/pull/1206), [#1233](https://github.com/kukuri-app/kukuri/pull/1233))
- blob 取得の自動再試行を有限化し、失敗表示と再取得を追加する ([#1207](https://github.com/kukuri-app/kukuri/pull/1207), [#1230](https://github.com/kukuri-app/kukuri/pull/1230))
- topic 追加ボタンの tooltip を「トピックを追加」にする ([#1229](https://github.com/kukuri-app/kukuri/pull/1229))
- 新しい Linux で AppImage が真っ白になる不具合を修正する ([#1222](https://github.com/kukuri-app/kukuri/pull/1222), [#1228](https://github.com/kukuri-app/kukuri/pull/1228))
- overlay 配下で tooltip を手前に描画する ([#1226](https://github.com/kukuri-app/kukuri/pull/1226))
- accelerate community index recovery ([#1223](https://github.com/kukuri-app/kukuri/pull/1223))
- refresh indexer peers before event ingest ([#1209](https://github.com/kukuri-app/kukuri/pull/1209))
- 無操作時のCN維持とdocs接続の復旧を安定化する ([#1208](https://github.com/kukuri-app/kukuri/pull/1208))
- changelog の起点を公開済み Release の最も近い祖先にする ([#1186](https://github.com/kukuri-app/kukuri/pull/1186), [#1187](https://github.com/kukuri-app/kukuri/pull/1187))

### Other

- v0.3.0-preview.1 の版へ同期する (#1384 AC-1) ([#1386](https://github.com/kukuri-app/kukuri/pull/1386))
- 不足候補の card の test で observer の登録を待ってから交差させる ([#1385](https://github.com/kukuri-app/kukuri/pull/1385))
- 完成版の動作確認を記録する (#1221 R6-A) ([#1383](https://github.com/kukuri-app/kukuri/pull/1383))
- #1221 R5-C: author と private の制御参照を有界な reader で解決する ([#1367](https://github.com/kukuri-app/kukuri/pull/1367))
- update Tauri lockfile for client reader dependency
- scale_independence の fixture で購読タスクの起動完了を観測してから数える ([#1363](https://github.com/kukuri-app/kukuri/pull/1363))
- wait for the lost neighbor before B subscribes again
- remove nondeterminism from reaction, cn-indexer and browser tests
- Align store migration generation assertions with grant ledger
- Align CLI registry counts with indexing revoke command
- Bound CN candidate admission before eligibility checks
- Keep private CN indexing consent across epoch rotation
- Unify bounded CN bucket reads for public and registered private epochs
- Wait for visibility-owned media attempts in browser tests
- Classify native preview commands and preserve padded media payloads
- Release offscreen attachment previews and preserve large media display
- Bound desktop attachment previews with file-backed media
- Complete R5-A cache ownership and fail-closed label eviction
- WIP R5-A app-owned remote cache and guarded content paths
- Preserve ephemeral retry admission through desktop wrapper
- Retry peer candidate writes after pending SQLite rollback
- Reject cancelled bounded hydration before any projection write
- Limit content save lock to remotely fetched bodies
- Propagate optional media source through harness requests
- Fence content fetches by account and permission generation
- Keep session retry tests within file size guard
- Drive post retry notifications from Rust attempt deadlines
- Count display retries only after network admission
- Unify visible display retries across bodies replies and sessions
- Checkpoint shared display retry budgets
- Use connected and learned peers for account destinations ([#1355](https://github.com/kukuri-app/kukuri/pull/1355))
- Bound visible author and advisory lookups to display references ([#1354](https://github.com/kukuri-app/kukuri/pull/1354))
- Route bounded docs and blob demand through one network owner ([#1353](https://github.com/kukuri-app/kukuri/pull/1353))
- Bound account peer candidate history and selection ([#1352](https://github.com/kukuri-app/kukuri/pull/1352))
- bound R1-D notification inbox work ([#1351](https://github.com/kukuri-app/kukuri/pull/1351))
- 投稿カードの icon test を別ファイルへ分ける ([#1345](https://github.com/kukuri-app/kukuri/pull/1345))
- 視覚回帰 baseline を再生成 (Kukuri Visual Baseline 35997528208)
- bound R1-A display and bookmark windows ([#1349](https://github.com/kukuri-app/kukuri/pull/1349))
- 投稿カードの icon 表示に合わせて既存 test を更新する ([#1345](https://github.com/kukuri-app/kukuri/pull/1345))
- require one planned PR per acceptance condition
- Reconcile removed CN scopes through bounded index seeks
- Wait for CN event completion before reading metrics
- Reserve CN scope slots before partial opens
- Admit bounded legacy CN scopes by demand and fair cursor
- Update CN rollout checks for bounded scope review
- Bound private CN periodic and unknown-key ingestion
- Preserve known CN object events in mixed batches
- Bound unknown public CN events to the current index window
- Own bounded resolver requests across crate boundaries
- Forward client bucket readers and suppress unchecked timeout results
- Resolve public CN bucket locators through bounded peer reads
- Persist withdrawals found during CN safety scans
- Keep verified CN withdrawals across provider changes
- Connect bounded public bucket reads to CN indexer
- Make CN E2E publisher write timeline index like desktop
- Bound CN public periodic ingest to current index window
- query CN withdrawals by changed object
- avoid unchanged timeline snapshot updates
- remove unused account locator discovery path
- 計画・構造整理・UI設計を新原則へ整合
- 初回の重複集約migrationの通報影響確認を維持
- 作業原則と設計原則に沿ってrunbookを整理
- 作業の完了範囲・最終コード量・エッジケースの原則を定義
- Validate full locator page in fake resolver
- Probe signed receive locator pages over live QUIC
- Add signed account receive locator claim
- Define change-driven account locator contract
- Prioritize current design principles over older ADRs
- Use one monotonic revision for CN clear and invalidation
- Fence in-flight bindings on every CN source clear
- Bind CN queries to due sessions and cache provenance
- Fence CN candidates and page recipient discovery fairly
- Add bounded CN candidates for account receive destinations
- Document pre-PR boundary checks for network work
- Recover account receive route after stack replacement
- Prevent superseded account offer receivers from reclaiming route
- Fence account offer teardown and attachment reflection
- Keep account route teardown generation-safe
- Connect authenticated account offers to DM receive
- Keep CLI DM schemas in sync with bounded status
- Bound DM status outbox count and show lower bound
- Bound DM outbox retry cycles against new sends
- Page protected DM outbox retries by peer
- Guard gossip topic replacement by generation
- Own gossip topic warmup tasks through shutdown
- Bound gossip peer warmup to a moving window
- Move blob-service tests out of production module
- Reject retained expired offers before provider I/O
- Add provider-verified bounded receive offer payload fetch
- Fence account offer transport shutdown and drain holds
- Recreate stopped account offer receiver on resubscribe
- Own offer tasks before cancellation and close fake account streams
- Add bounded account receive offer transport route
- Expose production docs sync handle from shared engine
- Bound CN rendezvous Redis response timeout under load
- Make rendezvous bucket insertion and expiry atomic
- Bound CN rendezvous candidates with expiring topic windows
- Own notification event forwarding with account runtime
- Update store migration generation contract
- Format notification dispatch regression test
- Page OS notification dispatch by insertion sequence
- Update Tauri lockfile for node binding dependency
- Classify receive binding test lock and report Store dirty paths
- Register account receive binding on desktop node lifecycle
- Bound blob peer health and fetch candidate selection
- Keep fetch service generations unique through retirement
- Share bounded fetch work across node services
- Admit display fetches through bounded node-owned work
- Bind signed DM acknowledgements to their conversation
- Verify owned gossip I/O with public topic state
- Bound network work admission and cancellation by scope
- [codex][fix] 終了したgossip接続とpeer address台帳を回収する
- [codex][feat] アカウント宛の受信参照とprivate manifestを暗号化する
- [codex][feat] アカウント受信先を署名とQUIC identityで確認する
- [codex][docs] #1221の共通通信設計と受信方針を記録
- 公開bucketの検索結果を安全にローカル解決する ([#1303](https://github.com/kukuri-app/kukuri/pull/1303))
- separate replica close operation from task dispatch ([#1305](https://github.com/kukuri-app/kukuri/pull/1305))
- session表示command追加後のCLI件数を更新する
- Issue1262の差分監査と最終検証を記録する
- CNの公開topicで時間bucketの読取りと停止を準備する ([#1300](https://github.com/kukuri-app/kukuri/pull/1300))
- avatar取得選定をbackend hookへ寄せる
- Dome読出し件数の計測を同期lookupに限定する
- live・game一覧の再取得回数を固定する ([#1292](https://github.com/kukuri-app/kukuri/pull/1292))
- [codex][fix] Dome Instance の読み出しを上限つきにする ([#1263](https://github.com/kukuri-app/kukuri/pull/1263))
- タスクトレイへの格納を明示選択にする ([#1288](https://github.com/kukuri-app/kukuri/pull/1288))
- worktreeincludeとdocs追加 ([#1286](https://github.com/kukuri-app/kukuri/pull/1286))
- replica の件数を 10 万件まで増やしても読む量が増えないことを数で確かめ、文書を整理する ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1279](https://github.com/kukuri-app/kukuri/pull/1279))
- 実 Iroh の結合 test の添付 view 状態の期待を #1152 の仕様へ合わせる ([#1251](https://github.com/kukuri-app/kukuri/pull/1251), [#1254](https://github.com/kukuri-app/kukuri/pull/1254))
- タイムラインの反映と復旧を総件数に依存させない設計を ADR 0052 に定める ([#1239](https://github.com/kukuri-app/kukuri/pull/1239), [#1244](https://github.com/kukuri-app/kukuri/pull/1244))
- 件数に依存しない処理を設計原則として AGENTS.md に定め、#1225 の結論を訂正する ([#1240](https://github.com/kukuri-app/kukuri/pull/1240))
- src-tauri の lib 単体 test に xtask の入口 tauri-test を追加する ([#1234](https://github.com/kukuri-app/kukuri/pull/1234), [#1235](https://github.com/kukuri-app/kukuri/pull/1235))
- record community indexer OOM recovery ([#1227](https://github.com/kukuri-app/kukuri/pull/1227))
- 誤って追跡した __pycache__ を外し、ignore する ([#1188](https://github.com/kukuri-app/kukuri/pull/1188))
- CHANGELOG に v0.2.7 / v0.2.8 を追加し、LP の案内を v0.2.8-preview.2 に更新する ([#1185](https://github.com/kukuri-app/kukuri/pull/1185))
- v0.2.8-preview.2 の公開結果と release の所要時間を記録する ([#1180](https://github.com/kukuri-app/kukuri/pull/1180), [#1184](https://github.com/kukuri-app/kukuri/pull/1184))

## [v0.2.8-preview.2] - 2026-09-19

### Features

- URLパースとOGPプレビューを追加 ([#1179](https://github.com/kukuri-app/kukuri/pull/1179))
- paste clipboard images into composers ([#1177](https://github.com/kukuri-app/kukuri/pull/1177))
- 案内する release を v0.2.7-preview.1 に更新する ([#1043](https://github.com/kukuri-app/kukuri/pull/1043), [#1170](https://github.com/kukuri-app/kukuri/pull/1170))

### Fixes

- show custom reaction names in tooltips ([#1178](https://github.com/kukuri-app/kukuri/pull/1178))

### Other

- linux-verify を PR でも走る reusable workflow にし PowerShell を入れる ([#1180](https://github.com/kukuri-app/kukuri/pull/1180), [#1183](https://github.com/kukuri-app/kukuri/pull/1183))
- prepare v0.2.8-preview.1 release ([#1182](https://github.com/kukuri-app/kukuri/pull/1182))
- Kukuri Release を Namespace へ移し cache を使わない構成にする ([#1180](https://github.com/kukuri-app/kukuri/pull/1180), [#1181](https://github.com/kukuri-app/kukuri/pull/1181))
- [codex][fix] 画像viewerのクリッピングを修正 ([#1175](https://github.com/kukuri-app/kukuri/pull/1175))
- profile editor 保存後の待ちの負荷時 flake を直す ([#1167](https://github.com/kukuri-app/kukuri/pull/1167), [#1169](https://github.com/kukuri-app/kukuri/pull/1169))
- v0.2.7-preview.1 の公開結果を作業記録へ追記する ([#1168](https://github.com/kukuri-app/kukuri/pull/1168))
- bookmarks 空状態と profile 集約の test の負荷時 flake を直す ([#1165](https://github.com/kukuri-app/kukuri/pull/1165), [#1166](https://github.com/kukuri-app/kukuri/pull/1166))

## [v0.2.7-preview.1] - 2026-09-18

### Features

- LP・OGP・Product Hunt・note・X の静止画を preset から一括で作る ([#1041](https://github.com/kukuri-app/kukuri/pull/1041), [#1156](https://github.com/kukuri-app/kukuri/pull/1156))
- 案内する release を v0.2.6-preview.1 にし、release.json で一元管理する ([#1043](https://github.com/kukuri-app/kukuri/pull/1043), [#1150](https://github.com/kukuri-app/kukuri/pull/1150))
- Dome 予告を実機の静止画 1 枚にし、取り込み手順を足す ([#1040](https://github.com/kukuri-app/kukuri/pull/1040), [#1143](https://github.com/kukuri-app/kukuri/pull/1143))
- kukuri.app の日本語・英語 LP を作る ([#1043](https://github.com/kukuri-app/kukuri/pull/1043), [#1144](https://github.com/kukuri-app/kukuri/pull/1144))
- 3 場面のデモデータと撮影シナリオで UI 原素材を作る ([#1039](https://github.com/kukuri-app/kukuri/pull/1039), [#1137](https://github.com/kukuri-app/kukuri/pull/1137))

### Fixes

- DB 作成前後で keyring account がずれて Windows 初回起動に失敗する問題を直す ([#1163](https://github.com/kukuri-app/kukuri/pull/1163))
- 添付の状態確認で remote 取得せず、成人向け表示 OFF の取得ゲートを迂回しない ([#1152](https://github.com/kukuri-app/kukuri/pull/1152), [#1158](https://github.com/kukuri-app/kukuri/pull/1158))
- Metaverse blob cache GC の unpin_blob を内側の IrohBlobService へ転送する ([#1157](https://github.com/kukuri-app/kukuri/pull/1157), [#1159](https://github.com/kukuri-app/kukuri/pull/1159))
- CSS・JS を内容のハッシュ付き URL で参照し、配信キャッシュに古い版が残らないようにする ([#1043](https://github.com/kukuri-app/kukuri/pull/1043), [#1155](https://github.com/kukuri-app/kukuri/pull/1155))
- リンクと小見出しの色を通常文字のコントラスト基準へ上げる ([#1043](https://github.com/kukuri-app/kukuri/pull/1043), [#1147](https://github.com/kukuri-app/kukuri/pull/1147))
- Metaverse の Tab / Enter で開いた overlay へ同じ key event 内で focus を移す ([#1139](https://github.com/kukuri-app/kukuri/pull/1139), [#1142](https://github.com/kukuri-app/kukuri/pull/1142))

### Other

- prepare v0.2.7-preview.1 release ([#1164](https://github.com/kukuri-app/kukuri/pull/1164))
- 本番 Community Node でテスターフィードバック受付を有効化した記録を追加する ([#1162](https://github.com/kukuri-app/kukuri/pull/1162))
- Cache Volume の保存量を減らし、容量を出力する ([#1160](https://github.com/kukuri-app/kukuri/pull/1160), [#1161](https://github.com/kukuri-app/kukuri/pull/1161))
- 検証の run を Namespace の Cache Volume へ移し、xtask 経由の依存再 build をなくす ([#1148](https://github.com/kukuri-app/kukuri/pull/1148), [#1149](https://github.com/kukuri-app/kukuri/pull/1149))
- #1068 の AC-6 を v0.2.6 で再確認した結果を記録する ([#1153](https://github.com/kukuri-app/kukuri/pull/1153))
- brief の配布候補を v0.2.6-preview.1 に更新する ([#1036](https://github.com/kukuri-app/kukuri/pull/1036), [#1151](https://github.com/kukuri-app/kukuri/pull/1151))
- v0.2.6-preview.1 のリリースと CN 反映を記録する ([#1146](https://github.com/kukuri-app/kukuri/pull/1146))
- update CHANGELOG for v0.2.6-preview.1 ([#1145](https://github.com/kukuri-app/kukuri/pull/1145))

## [v0.2.6-preview.1] - 2026-09-18

### Features

- 採用したコミュニティノードの信頼値で投稿を折りたためるようにする (#1061 PR3) ([#1130](https://github.com/kukuri-app/kukuri/pull/1130))
- Remotion + Playwright の制作環境と最小キャプチャ→レンダリング経路を入れる ([#1038](https://github.com/kukuri-app/kukuri/pull/1038), [#1135](https://github.com/kukuri-app/kukuri/pull/1135))
- ブロック・ミュートの観測を同意した CN へ提供できるようにする (#1061 PR2) ([#1129](https://github.com/kukuri-app/kukuri/pull/1129))
- trust 絶対値と閲覧者別 relation 値を CN 側で合算し、ブロック・ミュート観測を受け付ける (#1061 PR1) ([#1125](https://github.com/kukuri-app/kukuri/pull/1125))
- 推定による代替表示を枠上の短い表示にし、詳細を dialog へ移す ([#1108](https://github.com/kukuri-app/kukuri/pull/1108), [#1124](https://github.com/kukuri-app/kukuri/pull/1124))
- コミュニティノード規約を Markdown で描画し文書ごとに折りたためるようにする ([#1106](https://github.com/kukuri-app/kukuri/pull/1106), [#1115](https://github.com/kukuri-app/kukuri/pull/1115))

### Fixes

- 再 scan で現在の判定から外れた advisory signal を失効させる ([#1109](https://github.com/kukuri-app/kukuri/pull/1109), [#1119](https://github.com/kukuri-app/kukuri/pull/1119))
- 成人向け表示の切替で共有 blob がちらつき、通常画像がスケルトンに残る問題を修正 ([#1107](https://github.com/kukuri-app/kukuri/pull/1107), [#1114](https://github.com/kukuri-app/kukuri/pull/1114))
- 開発ビルドの app data を配布版と分け、同意記録に build の種別を残す ([#1105](https://github.com/kukuri-app/kukuri/pull/1105), [#1113](https://github.com/kukuri-app/kukuri/pull/1113))
- 初回の関係解析の前に stale alert を出さない ([#1102](https://github.com/kukuri-app/kukuri/pull/1102), [#1111](https://github.com/kukuri-app/kukuri/pull/1111))
- relation analyze の間隔を readiness の許容時間内に収まる範囲に制限する ([#1101](https://github.com/kukuri-app/kukuri/pull/1101), [#1103](https://github.com/kukuri-app/kukuri/pull/1103))
- startup 再実行後も readiness timer の次回実行を決める ([#1097](https://github.com/kukuri-app/kukuri/pull/1097), [#1098](https://github.com/kukuri-app/kukuri/pull/1098))
- 動画抽出の作業領域lock待機と decoder 準備、readiness の失敗分類を追加する ([#1091](https://github.com/kukuri-app/kukuri/pull/1091), [#1096](https://github.com/kukuri-app/kukuri/pull/1096))

### Other

- prepare v0.2.6-preview.1 release ([#1136](https://github.com/kukuri-app/kukuri/pull/1136))
- CI へ push する前にローカルで検証する規則を追記する ([#1134](https://github.com/kukuri-app/kukuri/pull/1134))
- LP・告知素材の共通brief を追加する ([#1037](https://github.com/kukuri-app/kukuri/pull/1037), [#1133](https://github.com/kukuri-app/kukuri/pull/1133))
- cn-images の push を regctl で行う ([#1122](https://github.com/kukuri-app/kukuri/pull/1122), [#1132](https://github.com/kukuri-app/kukuri/pull/1132))
- cn-images の 4 image を 1 回の build session で作る ([#1122](https://github.com/kukuri-app/kukuri/pull/1122), [#1131](https://github.com/kukuri-app/kukuri/pull/1131))
- rust-tests・Vitest・Playwright の並列度を上げる ([#1121](https://github.com/kukuri-app/kukuri/pull/1121), [#1128](https://github.com/kukuri-app/kukuri/pull/1128))
- test の失敗率を測る Kukuri Flake Probe workflow を追加する ([#1121](https://github.com/kukuri-app/kukuri/pull/1121), [#1127](https://github.com/kukuri-app/kukuri/pull/1127))
- debuginfo 変更前の成果物を含む Cache Volume を cache-tag の入れ替えで捨てる ([#1120](https://github.com/kukuri-app/kukuri/pull/1120), [#1126](https://github.com/kukuri-app/kukuri/pull/1126))
- harness を使わない job の xtask を軽くし Cache Volume の容量を減らす ([#1120](https://github.com/kukuri-app/kukuri/pull/1120), [#1123](https://github.com/kukuri-app/kukuri/pull/1123))
- Namespace の Linux 同時実行枠に収まるよう profile と concurrency を見直す ([#1117](https://github.com/kukuri-app/kukuri/pull/1117), [#1118](https://github.com/kukuri-app/kukuri/pull/1118))
- CI runner を Namespace へ移し Rust cache を Cache Volume に置く ([#1073](https://github.com/kukuri-app/kukuri/pull/1073), [#1112](https://github.com/kukuri-app/kukuri/pull/1112))
- v0.2.5 リリースと #1068 の CN 反映・統合確認を記録 ([#1110](https://github.com/kukuri-app/kukuri/pull/1110))
- update CHANGELOG for v0.2.5-preview.3 ([#1104](https://github.com/kukuri-app/kukuri/pull/1104))
- relation analyze timer が startup 再実行後も継続することを固定する ([#1099](https://github.com/kukuri-app/kukuri/pull/1099), [#1100](https://github.com/kukuri-app/kukuri/pull/1100))

## [v0.2.5-preview.3] - 2026-09-17

### Features

- 内容判定の共有とOpenAI Moderationの配備を接続 ([#1080](https://github.com/kukuri-app/kukuri/pull/1080))
- 動画抽出とOpenAI Moderationの基礎を追加 ([#1060](https://github.com/kukuri-app/kukuri/pull/1060), [#1079](https://github.com/kukuri-app/kukuri/pull/1079))
- タイムライン向け content advisory 一括照会・採用ノード設定・利用規約 version 6 ([#1056](https://github.com/kukuri-app/kukuri/pull/1056), [#1072](https://github.com/kukuri-app/kukuri/pull/1072))
- 見つけるで content advisory を成人向けゲートへ合成し取得ゲートへ登録する ([#1055](https://github.com/kukuri-app/kukuri/pull/1055), [#1071](https://github.com/kukuri-app/kukuri/pull/1071))
- nsfw / objectionable を content advisory 付きで index し trust 寄与 0・general_action knob を実装 ([#1054](https://github.com/kukuri-app/kukuri/pull/1054), [#1067](https://github.com/kukuri-app/kukuri/pull/1067))

### Fixes

- Linux package の build 前に libsqlite3-0 を現 APT 候補へ更新する ([#1094](https://github.com/kukuri-app/kukuri/pull/1094), [#1095](https://github.com/kukuri-app/kukuri/pull/1095))
- 取り込みの一時的な失敗で索引済み投稿を de-index しない ([#1090](https://github.com/kukuri-app/kukuri/pull/1090), [#1092](https://github.com/kukuri-app/kukuri/pull/1092))
- linux-verify に ffmpeg を入れ Fast の CN 依存と揃える ([#1087](https://github.com/kukuri-app/kukuri/pull/1087), [#1088](https://github.com/kukuri-app/kukuri/pull/1088))
- 生成 tfvars の moderation 行を段落分けし terraform fmt と一致させる ([#1082](https://github.com/kukuri-app/kukuri/pull/1082), [#1084](https://github.com/kukuri-app/kukuri/pull/1084))
- 変更通知の key を共有 replica の種別表で分類し索引 key で全体見直しへ倒れないようにする ([#1065](https://github.com/kukuri-app/kukuri/pull/1065), [#1077](https://github.com/kukuri-app/kukuri/pull/1077))
- Column 内の操作で押した Column を active にし route を同期する ([#1053](https://github.com/kukuri-app/kukuri/pull/1053), [#1076](https://github.com/kukuri-app/kukuri/pull/1076))
- operator が確定した risk signal を再 scan の集約更新から保護する ([#1058](https://github.com/kukuri-app/kukuri/pull/1058), [#1075](https://github.com/kukuri-app/kukuri/pull/1075))
- 「見つける」で解決済み投稿の添付画像を表示する ([#1052](https://github.com/kukuri-app/kukuri/pull/1052), [#1070](https://github.com/kukuri-app/kukuri/pull/1070))
- 保存済み verdict の再利用と risk signal の集約、変更 key 単位の ingest ([#1050](https://github.com/kukuri-app/kukuri/pull/1050), [#1059](https://github.com/kukuri-app/kukuri/pull/1059))

### Other

- DM status refresh 失敗 test の polling 待ちを fake timer 化する ([#1086](https://github.com/kukuri-app/kukuri/pull/1086), [#1089](https://github.com/kukuri-app/kukuri/pull/1089))
- 移管後の旧 owner 参照を kukuri-app へ更新する ([#1085](https://github.com/kukuri-app/kukuri/pull/1085))
- prepare v0.2.5-preview.1 release ([#1081](https://github.com/kukuri-app/kukuri/pull/1081))
- #1063 の main 3 run の CI 計測を記録 ([#1078](https://github.com/kukuri-app/kukuri/pull/1078))
- windows-fast の Windows package timeout を 45 分にし cache 設定を見直す ([#1063](https://github.com/kukuri-app/kukuri/pull/1063), [#1074](https://github.com/kukuri-app/kukuri/pull/1074))
- #1054 の本番反映を C5（#1068）へ移管した判断を記録 ([#1069](https://github.com/kukuri-app/kukuri/pull/1069))
- #1050 の AC-4 本番計測と Close 判定を記録 ([#1066](https://github.com/kukuri-app/kukuri/pull/1066))
- #1050 の本番反映（T6）記録を追加 ([#1064](https://github.com/kukuri-app/kukuri/pull/1064))
- nsfw / objectionable を content advisory 付きで index する方針を ADR へ固定 (#1051 C1) ([#1057](https://github.com/kukuri-app/kukuri/pull/1057))
- metaberse uiux ([#1049](https://github.com/kukuri-app/kukuri/pull/1049))
- READMEを v0.2.4-preview.1 時点の実装状況へ更新 ([#1048](https://github.com/kukuri-app/kukuri/pull/1048))
- record v0.2.4-preview.1 release and VM rollout ([#1047](https://github.com/kukuri-app/kukuri/pull/1047))
- update CHANGELOG for v0.2.4-preview.1 ([#1046](https://github.com/kukuri-app/kukuri/pull/1046))

## [v0.2.4-preview.1] - 2026-09-15

### Features

- アバター追従カメラとポインタ固定を追加 ([#1027](https://github.com/KingYoSun/kukuri/pull/1027))

### Fixes

- 全画面の描画・高さ・文脈保持を修正 ([#1029](https://github.com/KingYoSun/kukuri/pull/1029))
- カラム実幅に応じた配置と幅変更後の表示位置を修正 ([#1028](https://github.com/KingYoSun/kukuri/pull/1028))
- 所有Domeの管理・再開・削除を可能にする ([#1026](https://github.com/KingYoSun/kukuri/pull/1026))
- monitor community index availability and retire legacy topics ([#1019](https://github.com/KingYoSun/kukuri/pull/1019))
- 返信・日時を読みやすくし、カラムと操作ボタンの配置を修正 ([#1017](https://github.com/KingYoSun/kukuri/pull/1017))

### Other

- prepare v0.2.4-preview.1 release ([#1045](https://github.com/KingYoSun/kukuri/pull/1045))
- Metaverseの方位操作と確認済み接続マップを追加 ([#1035](https://github.com/KingYoSun/kukuri/pull/1035))
- MetaverseのHUDをカテゴリメニューと詳細タブへ再設計 ([#1030](https://github.com/KingYoSun/kukuri/pull/1030))
- record v0.2.3-preview.2 release and VM rollout ([#1015](https://github.com/KingYoSun/kukuri/pull/1015))
- update CHANGELOG for v0.2.3-preview.2 ([#1014](https://github.com/KingYoSun/kukuri/pull/1014))
- record release hold and verified DM reconnect fix ([#1013](https://github.com/KingYoSun/kukuri/pull/1013))

## [v0.2.3-preview.2] - 2026-09-13

### Fixes

- warm up gossip when another protocol owns the active path ([#1012](https://github.com/KingYoSun/kukuri/pull/1012))

## [v0.2.2-preview.1] - 2026-09-12

### Features

- 開発者モードでアプリ内ログを閲覧・コピー・書き出しできるようにする ([#978](https://github.com/KingYoSun/kukuri/pull/978), [#986](https://github.com/KingYoSun/kukuri/pull/986))
- 見つける検索と申請 dialog が索引状況を確認できるようにする（GET /v1/indexing/status） ([#975](https://github.com/KingYoSun/kukuri/pull/975), [#985](https://github.com/KingYoSun/kukuri/pull/985))
- 設定に「バックアップと復元」section を追加し、Control Center と鍵移行画面から到達できる入口を明示 ([#967](https://github.com/KingYoSun/kukuri/pull/967), [#984](https://github.com/KingYoSun/kukuri/pull/984))
- 通常画面からプライベートチャンネルの作成・参加・共有へ到達できる入口と案内を追加 ([#966](https://github.com/KingYoSun/kukuri/pull/966), [#982](https://github.com/KingYoSun/kukuri/pull/982))
- 通知の受信設定を「通知」セクションへ移し、通知一覧と開発者設定に診断・ログの入口を明示 ([#962](https://github.com/KingYoSun/kukuri/pull/962), [#979](https://github.com/KingYoSun/kukuri/pull/979))
- ブロック導線を一覧・設定へ追加し、ブロック関係の投稿を双方向で非表示にする ([#961](https://github.com/KingYoSun/kukuri/pull/961), [#977](https://github.com/KingYoSun/kukuri/pull/977))

### Fixes

- 全丸ボタンを高密度化し、avatarを常に全丸にする ([#983](https://github.com/KingYoSun/kukuri/pull/983))
- 添付の対応形式を案内し、非対応ファイルの理由を表示 ([#965](https://github.com/KingYoSun/kukuri/pull/965), [#981](https://github.com/KingYoSun/kukuri/pull/981))
- 投稿作成を Esc で閉じ、focus ring を可視化し、キーボード操作の案内を追加 ([#964](https://github.com/KingYoSun/kukuri/pull/964), [#980](https://github.com/KingYoSun/kukuri/pull/980))
- 見つける検索の空状態に検索先・理由・次の行動を表示 ([#960](https://github.com/KingYoSun/kukuri/pull/960), [#976](https://github.com/KingYoSun/kukuri/pull/976))
- 接続診断の状態説明と回復導線を整理 ([#959](https://github.com/KingYoSun/kukuri/pull/959), [#974](https://github.com/KingYoSun/kukuri/pull/974))
- show developer mode status and diagnostic shortcuts ([#973](https://github.com/KingYoSun/kukuri/pull/973))
- explain feedback availability and settings recovery ([#972](https://github.com/KingYoSun/kukuri/pull/972))
- 更新確認の進捗と結果を通常モードに表示 ([#971](https://github.com/KingYoSun/kukuri/pull/971))
- preserve visible column context on viewport resize

### Other

- prepare v0.2.2-preview.1 release ([#987](https://github.com/KingYoSun/kukuri/pull/987))
- record independent audit for issue 918 reopen fix
- [codex][fix] 年齢未確認の同意ボタンを操作前に識別できるようにする ([#969](https://github.com/KingYoSun/kukuri/pull/969))
- [codex][fix] 日本語表示とLinuxファイル選択の英語残存を修正 ([#968](https://github.com/KingYoSun/kukuri/pull/968))
- record v0.2.1-preview.1 release and VM rollout ([#955](https://github.com/KingYoSun/kukuri/pull/955))
- update CHANGELOG for v0.2.1-preview.1 ([#954](https://github.com/KingYoSun/kukuri/pull/954))

## [v0.2.1-preview.1] - 2026-09-09

### Fixes

- keep ScoreGame projections aligned with canonical state ([#942](https://github.com/KingYoSun/kukuri/pull/942))
- 見つけるの見切れと本文の文字密度を修正 ([#918](https://github.com/KingYoSun/kukuri/pull/918))
- 初回言語と同意前の言語選択を改善 ([#917](https://github.com/KingYoSun/kukuri/pull/917))
- 初回同意の年齢確認案内と操作到達性を改善 ([#948](https://github.com/KingYoSun/kukuri/pull/948))
- report successful negative smoke exit to CI ([#911](https://github.com/KingYoSun/kukuri/pull/911))

### Other

- prepare v0.2.1-preview.1 release ([#953](https://github.com/KingYoSun/kukuri/pull/953))
- [codex][fix] Column再選択時に古いrouteへ戻らないようにする ([#952](https://github.com/KingYoSun/kukuri/pull/952))
- record ScoreGame freshness validation ([#942](https://github.com/KingYoSun/kukuri/pull/942))
- reproduce stale ScoreGame hydration overwrites ([#942](https://github.com/KingYoSun/kukuri/pull/942))
- 日本語のCI描画環境と監査記録を整える ([#917](https://github.com/KingYoSun/kukuri/pull/917))
- [codex][fix] 投稿添付UIを表示言語に合わせる ([#947](https://github.com/KingYoSun/kukuri/pull/947))
- [codex][fix] コミュニティノードの初回案内と同意後の検索復旧 ([#946](https://github.com/KingYoSun/kukuri/pull/946))
- [codex][fix] 公開投稿後に非activeプロフィールの一覧と件数を更新する ([#945](https://github.com/KingYoSun/kukuri/pull/945))
- [codex][feature] add refactoring audit trigger and guarded issue upsert ([#944](https://github.com/KingYoSun/kukuri/pull/944))
- [codex][docs] #872の親完了監査と初回baselineを確定する ([#941](https://github.com/KingYoSun/kukuri/pull/941))
- [codex][docs] #872 Phase 3の実行結果と実機比較を保存する ([#940](https://github.com/KingYoSun/kukuri/pull/940))
- isolate CN indexer read-only source resolution ([#926](https://github.com/KingYoSun/kukuri/pull/926), [#939](https://github.com/KingYoSun/kukuri/pull/939))
- give Dome transition attempts one owner ([#924](https://github.com/KingYoSun/kukuri/pull/924), [#937](https://github.com/KingYoSun/kukuri/pull/937))
- [codex][contract] CN indexerのsource解決と禁止I/Oを固定する ([#935](https://github.com/KingYoSun/kukuri/pull/935))
- share prepared CN Dome transfer activation ([#922](https://github.com/KingYoSun/kukuri/pull/922), [#936](https://github.com/KingYoSun/kukuri/pull/936))
- preserve issue 872 audit and scope records ([#938](https://github.com/KingYoSun/kukuri/pull/938))
- [codex][refactor:extract] 通知取得とstate反映を単一loaderへ集約する ([#933](https://github.com/KingYoSun/kukuri/pull/933))
- [codex][contract] Dome遷移の取消・ack喪失・cleanupを固定する ([#934](https://github.com/KingYoSun/kukuri/pull/934))
- [codex][contract] CN Dome transferの途中失敗とretryを固定する ([#932](https://github.com/KingYoSun/kukuri/pull/932))
- [codex][fix] Preset取得待ちでもroom一覧の読込みを継続する ([#931](https://github.com/KingYoSun/kukuri/pull/931))
- characterize notification loading side effects ([#919](https://github.com/KingYoSun/kukuri/pull/919), [#929](https://github.com/KingYoSun/kukuri/pull/929))
- align capability freeze references with promoted availability ([#928](https://github.com/KingYoSun/kukuri/pull/928), [#930](https://github.com/KingYoSun/kukuri/pull/930))
- record preview.2 publication and verified VM rollout ([#912](https://github.com/KingYoSun/kukuri/pull/912))
- update CHANGELOG for v0.2.0-preview.2 ([#910](https://github.com/KingYoSun/kukuri/pull/910))

## [v0.2.0-preview.2] - 2026-09-07

### Fixes

- refresh libgcrypt before native source collection ([#909](https://github.com/KingYoSun/kukuri/pull/909))

## [v0.1.8-preview.1] - 2026-08-31

### Features

- submit composer with Ctrl+Enter ([#841](https://github.com/KingYoSun/kukuri/pull/841))
- add Explore post actions
- render community index results as post cards
- add tooltips to all icon actions
- add dome recovery and return home ([#825](https://github.com/KingYoSun/kukuri/pull/825))
- add authoritative Dome entry and safe spawn ([#823](https://github.com/KingYoSun/kukuri/pull/823))
- inherit Dome access, block, presence, and spatial audio ([#822](https://github.com/KingYoSun/kukuri/pull/822))
- add seamless Dome transitions ([#821](https://github.com/KingYoSun/kukuri/pull/821))
- enforce resource budgets
- retain Dome prop layouts and metaverse assets
- change default subscribed topics to general / dev / test ([#805](https://github.com/KingYoSun/kukuri/pull/805))
- add tester feedback report collection ([#802](https://github.com/KingYoSun/kukuri/pull/802))
- add Dome hosting lease lifecycle
- implement Dome connection topology ([#792](https://github.com/KingYoSun/kukuri/pull/792))
- add spatial context dome moves ([#800](https://github.com/KingYoSun/kukuri/pull/800))
- 固定規格Domeと最小customizationを実装 ([#799](https://github.com/KingYoSun/kukuri/pull/799))

### Fixes

- clarify user-facing terminology ([#848](https://github.com/KingYoSun/kukuri/pull/848))
- 追い越されたpending route pushを破棄して現在URLを再投影 ([#847](https://github.com/KingYoSun/kukuri/pull/847))
- light themeのsemantic contrastをWCAG 2.2 AAへ調整 ([#844](https://github.com/KingYoSun/kukuri/pull/844))
- route起因のColumn scroll中のmobile settle誤activateを防止 ([#846](https://github.com/KingYoSun/kukuri/pull/846))
- 識別子コンテキスト操作の残課題を完了 ([#843](https://github.com/KingYoSun/kukuri/pull/843))
- complete Japanese UI localization ([#840](https://github.com/KingYoSun/kukuri/pull/840))
- settle background notification loading
- hide technical identifiers
- stop unavailable media loading
- prevent column transition flicker
- カラムをPointer Eventsで並び替えられるようにする ([#830](https://github.com/KingYoSun/kukuri/pull/830))
- discard blocked Dome connection proposals ([#827](https://github.com/KingYoSun/kukuri/pull/827))
- recover lost Dome transition commit acknowledgements ([#826](https://github.com/KingYoSun/kukuri/pull/826))
- register tester feedback tests in lock contract and fixed cluster CSS assertions ([#802](https://github.com/KingYoSun/kukuri/pull/802))

### Other

- refresh third-party notices for v0.1.8-preview.1 ([#851](https://github.com/KingYoSun/kukuri/pull/851))
- bump preview version to 0.1.8 ([#850](https://github.com/KingYoSun/kukuri/pull/850))
- [codex][fix] relay接続でデスクトップ起動を止めない ([#849](https://github.com/KingYoSun/kukuri/pull/849))
- Community Index投稿アクションを完了可能なものだけに限定 ([#842](https://github.com/KingYoSun/kukuri/pull/842))
- record issue 820 review
- wait for mobile page scroll settle
- split icon tooltip smoke coverage
- record issue 817 review
- update icon tooltip visual baseline
- TimelineとExploreに簡易切替を追加する ([#834](https://github.com/KingYoSun/kukuri/pull/834))
- update hidden identifier visual baseline
- [codex][docs] UI/UX品質基準を検証可能な契約へ整理 ([#829](https://github.com/KingYoSun/kukuri/pull/829))
- resolve Dome ADR numbering
- move tester feedback UI preview assets to docs/ui-reviews ([#802](https://github.com/KingYoSun/kukuri/pull/802))
- add tester feedback UI preview assets ([#802](https://github.com/KingYoSun/kukuri/pull/802))
- update CHANGELOG for v0.1.7-preview.1 ([#787](https://github.com/KingYoSun/kukuri/pull/787))

## [v0.1.7-preview.1] - 2026-08-26

### Features

- show key decks on fresh start ([#785](https://github.com/KingYoSun/kukuri/pull/785))

### Fixes

- harden multilingual UI layouts ([#783](https://github.com/KingYoSun/kukuri/pull/783))

### Other

- bump preview version to 0.1.7 ([#786](https://github.com/KingYoSun/kukuri/pull/786))
- make README a user and contributor entry point ([#784](https://github.com/KingYoSun/kukuri/pull/784))
- update CHANGELOG for v0.1.6-preview.1 ([#781](https://github.com/KingYoSun/kukuri/pull/781))

## [v0.1.6-preview.1] - 2026-08-25

### Features

- 配布素材の権利と再配布条件を管理する ([#778](https://github.com/KingYoSun/kukuri/pull/778))
- 投稿コンテンツの権利表明と限定的利用許諾を追加 ([#777](https://github.com/KingYoSun/kukuri/pull/777))
- add scoped case retention and legal holds ([#776](https://github.com/KingYoSun/kukuri/pull/776))
- add rights infringement request intake ([#760](https://github.com/KingYoSun/kukuri/pull/760), [#775](https://github.com/KingYoSun/kukuri/pull/775))
- add signed withdrawals and transmission prevention ([#774](https://github.com/KingYoSun/kukuri/pull/774))
- add staged column workspace features ([#772](https://github.com/KingYoSun/kukuri/pull/772))
- add Issue #748 Wave 6 mobile lifecycle ([#758](https://github.com/KingYoSun/kukuri/pull/758))
- add Issue #748 Wave 5 variable spans ([#757](https://github.com/KingYoSun/kukuri/pull/757))
- add Issue #748 Wave 4 Control Center ([#756](https://github.com/KingYoSun/kukuri/pull/756))
- 管理画面の確認・結果ページを dashboard と共通 shell にする ([#740](https://github.com/KingYoSun/kukuri/pull/740), [#747](https://github.com/KingYoSun/kukuri/pull/747))
- 管理画面の操作を標準で有効化 ([#742](https://github.com/KingYoSun/kukuri/pull/742))

### Fixes

- pin release XML assets to LF ([#780](https://github.com/KingYoSun/kukuri/pull/780))
- Issue #765 Column 振る舞いの残課題を解消 ([#771](https://github.com/KingYoSun/kukuri/pull/771))
- Issue #748 監査 blocker B1-B5 を修正 ([#769](https://github.com/KingYoSun/kukuri/pull/769))
- Community Node provider alertの誤検知を分離 ([#746](https://github.com/KingYoSun/kukuri/pull/746))
- BlobText投稿本文をCommunity Indexへ投影する ([#745](https://github.com/KingYoSun/kukuri/pull/745))
- preserve native select contrast on Linux ([#743](https://github.com/KingYoSun/kukuri/pull/743))
- 接続設定の横スクロールを防ぐ ([#744](https://github.com/KingYoSun/kukuri/pull/744))

### Other

- bump preview version to 0.1.6 ([#779](https://github.com/KingYoSun/kukuri/pull/779))
- remove legacy shell projections ([#773](https://github.com/KingYoSun/kukuri/pull/773))
- Issue #768 Validation マトリクスと review record を補完 ([#770](https://github.com/KingYoSun/kukuri/pull/770))
- [codex][refactor:delete] 旧ShellFrame経路を削除 ([#759](https://github.com/KingYoSun/kukuri/pull/759))
- Issue #748 Wave 3: scope Columns and composers ([#755](https://github.com/KingYoSun/kukuri/pull/755))
- Issue #748 Wave 2: migrate production surfaces to Columns ([#754](https://github.com/KingYoSun/kukuri/pull/754))
- extract existing surface renderers ([#753](https://github.com/KingYoSun/kukuri/pull/753))
- Issue #748 Wave 1: use icon tabs in the Column header ([#752](https://github.com/KingYoSun/kukuri/pull/752))
- align detail visual baselines with Linux CI ([#751](https://github.com/KingYoSun/kukuri/pull/751))
- Issue #748 Wave 1: Column Canvas foundation ([#750](https://github.com/KingYoSun/kukuri/pull/750))
- Issue #748 Wave 0: 可変span Column Canvasのreview prototype ([#749](https://github.com/KingYoSun/kukuri/pull/749))
- update CHANGELOG for v0.1.5-preview.1 ([#737](https://github.com/KingYoSun/kukuri/pull/737))

## [v0.1.5-preview.1] - 2026-08-21

### Features

- 安定エラーコードを通信境界で定数化し契約試験と画面判別を揃える ([#712](https://github.com/KingYoSun/kukuri/pull/712), [#730](https://github.com/KingYoSun/kukuri/pull/730))
- 動画添付を media 対象として個別に通報できるようにする ([#697](https://github.com/KingYoSun/kukuri/pull/697), [#724](https://github.com/KingYoSun/kukuri/pull/724))
- add Community Node index client ([#671](https://github.com/KingYoSun/kukuri/pull/671))
- add audited Community Node admin actions ([#660](https://github.com/KingYoSun/kukuri/pull/660))

### Fixes

- propagate CN distance policy to Terraform ([#736](https://github.com/KingYoSun/kukuri/pull/736))
- 訂正版再発行後に利用者が審査結果を確認できるようにする ([#710](https://github.com/KingYoSun/kukuri/pull/710), [#732](https://github.com/KingYoSun/kukuri/pull/732))
- 索引申請の受付で索引参照の構成と有効化を確認する ([#713](https://github.com/KingYoSun/kukuri/pull/713), [#731](https://github.com/KingYoSun/kukuri/pull/731))
- 非公開チャンネルの索引参照を所属者に限定する ([#711](https://github.com/KingYoSun/kukuri/pull/711), [#728](https://github.com/KingYoSun/kukuri/pull/728))
- 異議申し立て審査の確認画面に変更前後の値を表示する ([#701](https://github.com/KingYoSun/kukuri/pull/701), [#727](https://github.com/KingYoSun/kukuri/pull/727))
- 異議申し立て審査の入力値を保存前に検証する ([#700](https://github.com/KingYoSun/kukuri/pull/700), [#726](https://github.com/KingYoSun/kukuri/pull/726))
- 信頼・関係応答の対象識別子を要求対象へ照合する ([#723](https://github.com/KingYoSun/kukuri/pull/723))
- 通報送信時に受付先を構成済みノードと同一オリジンへ限定し転送を追跡しない ([#722](https://github.com/KingYoSun/kukuri/pull/722))
- 通報先候補で提供中能力と責任範囲を厳密に照合する ([#721](https://github.com/KingYoSun/kukuri/pull/721))
- 信頼・関係機能を同意済みで提供中のノードに限定する ([#705](https://github.com/KingYoSun/kukuri/pull/705), [#720](https://github.com/KingYoSun/kukuri/pull/720))
- 索引ノードの利用可否変更時に古い選択・結果・索引申請を失効させる ([#719](https://github.com/KingYoSun/kukuri/pull/719))
- 参加拒否後に保持したトークンで自己修復経路が再認証を繰り返す問題を止める ([#718](https://github.com/KingYoSun/kukuri/pull/718))
- 添付対象のリスク判定を画面から異議申し立てできるようにする ([#717](https://github.com/KingYoSun/kukuri/pull/717))
- 異議申し立ての発行元識別子を公開ノード情報の node_id と一致させる ([#716](https://github.com/KingYoSun/kukuri/pull/716))
- 通報画面で開いた時に取得した最新ノード情報だけを送信先候補にする ([#714](https://github.com/KingYoSun/kukuri/pull/714))
- 観測記録の90日保持を読み取り時にも強制 ([#695](https://github.com/KingYoSun/kukuri/pull/695))
- 信頼・関係画面の取得文脈を固定 ([#694](https://github.com/KingYoSun/kukuri/pull/694))
- 索引結果の通報文脈を固定 ([#693](https://github.com/KingYoSun/kukuri/pull/693))
- 異議申し立ての対象と解消済み状態を修正 ([#689](https://github.com/KingYoSun/kukuri/pull/689))
- 添付の観測元と通報経路を修正 ([#688](https://github.com/KingYoSun/kukuri/pull/688))
- 索引申請の秘密値確認を送信ごとに消費 ([#687](https://github.com/KingYoSun/kukuri/pull/687))

### Other

- bump preview version to 0.1.5 ([#735](https://github.com/KingYoSun/kukuri/pull/735))
- stabilize docs sync relay tests ([#734](https://github.com/KingYoSun/kukuri/pull/734))
- 運営者審査の有効化設定を compose と terraform に配線する ([#709](https://github.com/KingYoSun/kukuri/pull/709), [#729](https://github.com/KingYoSun/kukuri/pull/729))
- クライアント視点の結合試験に異議申し立ての一続きと距離利用停止の結線確認を加える ([#725](https://github.com/KingYoSun/kukuri/pull/725))
- 通報画面試験で送信ボタンの出現を待ち合わせる ([#715](https://github.com/KingYoSun/kukuri/pull/715))
- 招待制参加認証の運用手順を更新 ([#686](https://github.com/KingYoSun/kukuri/pull/686))
- Issue #680 の異議申し立て審査を追加 ([#682](https://github.com/KingYoSun/kukuri/pull/682))
- 課題 #669 リスク判定への異議申し立てを追加 ([#681](https://github.com/KingYoSun/kukuri/pull/681))
- 非公開チャンネルのランデブー鍵を世代秘密から派生 ([#679](https://github.com/KingYoSun/kukuri/pull/679))
- Community Node の招待認証にクライアントを対応させる ([#678](https://github.com/KingYoSun/kukuri/pull/678))
- 課題 #666 の観測元記録と分散通報を実装 ([#677](https://github.com/KingYoSun/kukuri/pull/677))
- Add trust relation desktop UI and harness ([#676](https://github.com/KingYoSun/kukuri/pull/676))
- Issue #665: trust/relation desktop clientを追加 ([#675](https://github.com/KingYoSun/kukuri/pull/675))
- Fix distance opt-out E2E contract ([#674](https://github.com/KingYoSun/kukuri/pull/674))
- Implement community node distance opt-out ([#673](https://github.com/KingYoSun/kukuri/pull/673))
- Implement Community Node indexing request flows ([#672](https://github.com/KingYoSun/kukuri/pull/672))
- run workflows with Rust 1.92 ([#662](https://github.com/KingYoSun/kukuri/pull/662))
- update Rust and TypeScript dependencies ([#661](https://github.com/KingYoSun/kukuri/pull/661))
- update CHANGELOG for v0.1.4-preview.1 ([#659](https://github.com/KingYoSun/kukuri/pull/659))

## [v0.1.4-preview.1] - 2026-08-11

### Features

- automate remaining preview operations ([#656](https://github.com/KingYoSun/kukuri/pull/656))

### Fixes

- do not treat clean classifier as detection ([#654](https://github.com/KingYoSun/kukuri/pull/654))
- share active peers with media fetcher ([#653](https://github.com/KingYoSun/kukuri/pull/653))
- report deployed indexer stack ([#652](https://github.com/KingYoSun/kukuri/pull/652))
- publish data retention disclosure ([#650](https://github.com/KingYoSun/kukuri/pull/650))
- honor readiness activation after startup ([#649](https://github.com/KingYoSun/kukuri/pull/649))
- mount readiness operator config ([#648](https://github.com/KingYoSun/kukuri/pull/648))
- sync live peers and serve disclosures ([#647](https://github.com/KingYoSun/kukuri/pull/647))
- emit updater manifest without BOM ([#646](https://github.com/KingYoSun/kukuri/pull/646))

### Other

- bump preview version to 0.1.4 ([#658](https://github.com/KingYoSun/kukuri/pull/658))
- update CHANGELOG for v0.1.3-preview.1 ([#657](https://github.com/KingYoSun/kukuri/pull/657))
- add community node production rollout runbook ([#655](https://github.com/KingYoSun/kukuri/pull/655))
- cover replica query failure end to end ([#651](https://github.com/KingYoSun/kukuri/pull/651))
- update CHANGELOG for v0.1.3-preview.1 ([#645](https://github.com/KingYoSun/kukuri/pull/645))

## [v0.1.3-preview.1] - 2026-08-07

### Features

- MediaFetcher の本番実装(blob の一時 fetch で media scan を実働化)
- media 参照ごとの safety scan + derived タグの index 相乗り
- appeal 経路 + operator レビュー
- openai-compatible-vlm の provider 解決 + operator config/readiness
- operator policy 注入 + suspected visibility override + derived_tags
- OpenAI-compatible VLM moderation provider crate
- suspected_threshold(70)/signal visibility policy + derived tags foundation
- report ConnectionPath::RelayFallback from connection diagnostics
- community node trust / relation foundation (CommunityLocalTrust read surface) ([#415](https://github.com/KingYoSun/kukuri/pull/415), [#427](https://github.com/KingYoSun/kukuri/pull/427))
- Project Arachnid Shield known-CSAM provider 統合 ([#391](https://github.com/KingYoSun/kukuri/pull/391), [#426](https://github.com/KingYoSun/kukuri/pull/426))
- fail-closed community indexing 本体と search/discovery/recommendation 除外 ([#404](https://github.com/KingYoSun/kukuri/pull/404), [#425](https://github.com/KingYoSun/kukuri/pull/425))
- add model C index ingestion ([#423](https://github.com/KingYoSun/kukuri/pull/423))
- persist signed moderation events ([#407](https://github.com/KingYoSun/kukuri/pull/407))
- add uuid event id generator ([#403](https://github.com/KingYoSun/kukuri/pull/403))
- add system scan clock ([#402](https://github.com/KingYoSun/kukuri/pull/402))
- add safety runtime adapter ([#400](https://github.com/KingYoSun/kukuri/pull/400))
- add safety readiness CLI ([#397](https://github.com/KingYoSun/kukuri/pull/397))
- add safety domain model ([#396](https://github.com/KingYoSun/kukuri/pull/396))
- generate low-cost tfvars from operator config ([#394](https://github.com/KingYoSun/kukuri/pull/394))
- add GCP community node Terraform ([#392](https://github.com/KingYoSun/kukuri/pull/392))
- add community node admission controls ([#390](https://github.com/KingYoSun/kukuri/pull/390))
- add community node consent review flow ([#389](https://github.com/KingYoSun/kukuri/pull/389))
- add app legal consent gate ([#388](https://github.com/KingYoSun/kukuri/pull/388))
- iroh 1.0 に更新 ([#385](https://github.com/KingYoSun/kukuri/pull/385))
- バックグラウンドOS通知とトレイ常駐を追加 ([#304](https://github.com/KingYoSun/kukuri/pull/304), [#378](https://github.com/KingYoSun/kukuri/pull/378))
- OS通知クリックで対象投稿を開く ([#377](https://github.com/KingYoSun/kukuri/pull/377))
- 更新DL/検証後に再起動確認プロンプトを追加 ([#319](https://github.com/KingYoSun/kukuri/pull/319), [#376](https://github.com/KingYoSun/kukuri/pull/376))
- capability 別リスクと推奨対応ガイドを生成 ([#359](https://github.com/KingYoSun/kukuri/pull/359), [#375](https://github.com/KingYoSun/kukuri/pull/375))
- コミュニティノード側の通報受信エンドポイントと運営者確認導線 ([#370](https://github.com/KingYoSun/kukuri/pull/370), [#371](https://github.com/KingYoSun/kukuri/pull/371))
- コミュニティノード宛ての分散通報ルーティング ([#310](https://github.com/KingYoSun/kukuri/pull/310), [#369](https://github.com/KingYoSun/kukuri/pull/369))
- community node 依存度 / capability scope 表示 ([#357](https://github.com/KingYoSun/kukuri/pull/357), [#368](https://github.com/KingYoSun/kukuri/pull/368))
- content provenance と responsible capability metadata を追加 ([#358](https://github.com/KingYoSun/kukuri/pull/358), [#367](https://github.com/KingYoSun/kukuri/pull/367))
- public manifest endpoint ([#356](https://github.com/KingYoSun/kukuri/pull/356), [#366](https://github.com/KingYoSun/kukuri/pull/366))
- manifest を型付き共有スキーマ化し authority scope/P2P境界を追加 ([#355](https://github.com/KingYoSun/kukuri/pull/355), [#365](https://github.com/KingYoSun/kukuri/pull/365))
- コミュニティノード運営者向け文書生成CLIを追加 ([#352](https://github.com/KingYoSun/kukuri/pull/352), [#364](https://github.com/KingYoSun/kukuri/pull/364))
- スレッドツリーの枝線・タイムラインのリプライ表示・ブックマークアイコン・メンションz-indexを改善 ([#351](https://github.com/KingYoSun/kukuri/pull/351))

### Fixes

- make private secret file persistence atomic via temp+fsync+rename
- refresh topic rendezvous presence independently of the bootstrap heartbeat
- decode NULL option columns as None instead of empty values
- log the discarded operation context on internal errors
- bind metaverse translations to shell locale ([#543](https://github.com/KingYoSun/kukuri/pull/543))
- localize metaverse room surfaces ([#542](https://github.com/KingYoSun/kukuri/pull/542))
- localize direct message errors ([#535](https://github.com/KingYoSun/kukuri/pull/535))
- harden GCP community node COS bootstrap ([#393](https://github.com/KingYoSun/kukuri/pull/393))

### Other

- bump preview version to 0.1.3 ([#644](https://github.com/KingYoSun/kukuri/pull/644))
- Complete community node operational hardening ([#643](https://github.com/KingYoSun/kukuri/pull/643))
- #617 の昇格・開示同期の記録を追加する
- 届出用構成図・役務説明・モデレーション方針を実態へ同期する (#617 T5+T6)
- 保存・保持の開示にデータ区分と保存先を追加する (#617 T4)
- 安全性走査プロバイダの外部送信を operator config から動的に開示する (#617 T3)
- 昇格した capability の説明を実装済みのデータフローへ更新する (#617 T2)
- community index / moderation / local trust を提供中へ昇格する (#617 T1)
- #616 の実機解禁記録と readiness 運用手順を追加する
- generate-tfvars が読み取り面の環境変数 gate を features から導出する
- 信頼・申し立て・関係・復旧の E2E を追加する (#616 T6)
- 全構成 E2E の土台と許可・不許可・障害経路を追加する (#616 T4+T5)
- readiness 全項目合格の記録を関門にして読み取り面を有効化する
- readinessの走査網羅と全構成の実行時判定を実測で確定させる（#616 T2）
- cn-cli readinessでプロバイダ疎通確認を実行しprovider_credential_validを確定させる（#616 T1）
- CIのlow-cost planへ実運用tfvarsをrepository variable経由で供給する
- ローカルinitで混入したlock file差分を戻す
- COSの/var noexecでbackup / cert-renew timerが実行できない問題を直す
- e2e smoke specのtopic selectorを表示名基準へ更新する
- topic IDの名前空間prefixをUIから隠す
- docs / example / test中の実運用由来のprivate subnetをRFC 5737のdocumentationレンジへ置換する
- xtaskのcn compose envへ#615で必須化した変数を追加しCIのcompose起動を直す
- GCP runbookへindex / moderation stackの配備・再構築・rollback手順を追記する（#615 T7）
- ArcadeDB CREATE PROPERTYのIF NOT EXISTS位置を文法どおりデータ型の前へ直す
- ローカルinitで混入したlock file差分を戻す
- GCP low-cost Terraformへcn-indexer・ArcadeDB・moderation secrets・relation timerを追加する（#615 T4-T6）
- ArcadeDB image既定値を26.8.1へpinする
- 標準composeへArcadeDB・cn-indexer・relation定期解析を追加する（#615 T2/T3）
- cn-operatorのdeploy configをindexer stack対応に拡張しtfvars生成へ配線する（#615 T1）
- cn-indexer imageをGHCR workflowに追加しpublish前のvalidate-config smokeを組み込む（#614 T3-T5）
- rustfmt差分を修正する
- cn-indexerのproduction featureをArachnid+VLMに分離しvalidate-configモードを追加する（#614 T1/T2）
- 実iroh 2台と実ArcadeDBの統合テストを追加し、#613の進捗を文書化する（#613 T4/T5）
- cn-indexerを常駐ワーカー化し観測状態を追加する（#613 T2/T3）
- cargo fmtの整形差分を修正する
- cn-indexerのingest経路を本番依存で結線する（#613 T1）
- Add UI review record and previews for developer mode
- Add developer mode toggle hiding WIP features and diagnostics
- rust-toolchainにrust-analyzerを追加
- cargo fmt(clippy --fix 後の let-chain 整形追随)
- ADR 0028 §7 実装追補 + runbook + progress doc + 実機 e2e テスト
- split connection path tests out of relay_connectivity.rs
- declare the new rendezvous scheduler test in the lock classification
- extract the shared hint roundtrip test helpers
- record the standing decisions for observer pull and fake transport scope
- gate unused shell CSS selectors with a vitest sweep
- extract the shared remote fetch loop into iroh-node
- share the CN response types instead of client mirrors
- fix references that rotted behind later refactors
- pin runtimeApi request literals to the generated DTO types
- generate the IPC request DTOs into types.generated.ts
- stop triggering kukuri-fast on docs-only changes
- ratchet baseline up for the published retry predicates
- publish the private channel import retry contract
- write placement conventions for refactored boundaries
- reconcile oversized baseline after B9 merges
- ratchet oversized baseline down to current line counts
- delete dead frontend exports and CSS selectors
- remove cn-core protocol shims and dead cn-protocol fns
- delete dead client-side pub APIs and unreachable branch
- extract remaining section view models
- apply rustfmt to lock_contract
- add lock classification contract for desktop-runtime
- unify hand-rolled stable polls onto poll_until
- make section loaders the single source of shell data fetching
- remove tauri-plugin-notification
- replace notification permission plugin invokes with app commands
- derive CN packages from cargo metadata
- split cn-cli command modules
- cover cn-cli timestamp helpers
- centralize CN tracing setup ([#567](https://github.com/KingYoSun/kukuri/pull/567))
- move safety composition to runtime ([#566](https://github.com/KingYoSun/kukuri/pull/566))
- pin safety persistence failures ([#565](https://github.com/KingYoSun/kukuri/pull/565))
- scenario wire orphaned scenarios into nightly
- refactor scope desktop test locks
- refactor add resource scoped test locks
- refactor centralize cn test env gates
- refactor split app api slow tests ([#560](https://github.com/KingYoSun/kukuri/pull/560))
- refactor shared test diagnostics ([#559](https://github.com/KingYoSun/kukuri/pull/559))
- refactor test support foundation ([#558](https://github.com/KingYoSun/kukuri/pull/558))
- type trust and relation errors ([#557](https://github.com/KingYoSun/kukuri/pull/557))
- type indexing errors ([#556](https://github.com/KingYoSun/kukuri/pull/556))
- type bootstrap and report errors ([#555](https://github.com/KingYoSun/kukuri/pull/555))
- type auth and consent errors ([#554](https://github.com/KingYoSun/kukuri/pull/554))
- freeze HTTP error contracts ([#553](https://github.com/KingYoSun/kukuri/pull/553))
- type private channel import errors ([#552](https://github.com/KingYoSun/kukuri/pull/552))
- characterize private channel import errors ([#551](https://github.com/KingYoSun/kukuri/pull/551))
- route helpers through service handles ([#550](https://github.com/KingYoSun/kukuri/pull/550))
- add service handles composition root ([#549](https://github.com/KingYoSun/kukuri/pull/549))
- make docs fetch policy explicit ([#548](https://github.com/KingYoSun/kukuri/pull/548))
- narrow internal core helpers ([#547](https://github.com/KingYoSun/kukuri/pull/547))
- make core exports explicit ([#546](https://github.com/KingYoSun/kukuri/pull/546))
- complete epoch handoff grant rename ([#545](https://github.com/KingYoSun/kukuri/pull/545))
- freeze epoch handoff legacy contract ([#544](https://github.com/KingYoSun/kukuri/pull/544))
- add metaverse shell actions ([#541](https://github.com/KingYoSun/kukuri/pull/541))
- extract metaverse room session ([#540](https://github.com/KingYoSun/kukuri/pull/540))
- extract metaverse room view ([#539](https://github.com/KingYoSun/kukuri/pull/539))
- extract metaverse room controls ([#538](https://github.com/KingYoSun/kukuri/pull/538))
- extract metaverse room discovery ([#537](https://github.com/KingYoSun/kukuri/pull/537))
- characterize metaverse room boundaries ([#536](https://github.com/KingYoSun/kukuri/pull/536))
- split shell presentation selectors ([#534](https://github.com/KingYoSun/kukuri/pull/534))
- centralize shell route state ([#533](https://github.com/KingYoSun/kukuri/pull/533))
- extract timeline view models ([#532](https://github.com/KingYoSun/kukuri/pull/532))
- split shell section loaders ([#531](https://github.com/KingYoSun/kukuri/pull/531))
- extract shell dialog controller ([#530](https://github.com/KingYoSun/kukuri/pull/530))
- unify shell focus scrolling ([#529](https://github.com/KingYoSun/kukuri/pull/529))
- extract shell share preview hook ([#528](https://github.com/KingYoSun/kukuri/pull/528))
- [fix] 同期ステータスを event push で即時反映 (WP-Q2b) ([#527](https://github.com/KingYoSun/kukuri/pull/527))
- [refactor] 通知ステータスを event push で即時反映 (WP-Q2 PR5) ([#526](https://github.com/KingYoSun/kukuri/pull/526))
- [refactor] CN セッション 7 マップを単一エントリに統合 (WP-Q2 PR4) ([#525](https://github.com/KingYoSun/kukuri/pull/525))
- [refactor] 起動エラー分類を文字列 contains から typed downcast へ (WP-Q2 PR3) ([#524](https://github.com/KingYoSun/kukuri/pull/524))
- [fix] get_community_node_statuses を読み取り専用化 (WP-Q2 PR2) ([#523](https://github.com/KingYoSun/kukuri/pull/523))
- [refactor] Reloadable* 3 ラッパーを declarative macro で生成 (WP-Q2 PR1) ([#522](https://github.com/KingYoSun/kukuri/pull/522))
- [refactor] main.tsx の browser mock seed を mocks/ へ移動 (WP-Q1 PR7) ([#521](https://github.com/KingYoSun/kukuri/pull/521))
- [refactor] 一回性の review Storybook 2 本を削除 (WP-Q1 PR6) ([#520](https://github.com/KingYoSun/kukuri/pull/520))
- [refactor] vestigial な endpoint_publish_task / dht_options 配管を除去 (WP-Q1 PR5b) ([#519](https://github.com/KingYoSun/kukuri/pull/519))
- [refactor] Rust dead: key_kind() と app-api の test 専用糖衣ラッパー 2 件を削除 (WP-Q1 PR5) ([#518](https://github.com/KingYoSun/kukuri/pull/518))
- [refactor] TS OS 通知 dead 5 関数と buildGameLink を削除 (WP-Q1 PR4) ([#517](https://github.com/KingYoSun/kukuri/pull/517))
- [refactor] dead な ShellTopBar 一式を削除 (WP-Q1 PR3) ([#516](https://github.com/KingYoSun/kukuri/pull/516))
- [refactor] Cargo 未使用依存 3 件の宣言削除 (WP-Q1 PR2) ([#515](https://github.com/KingYoSun/kukuri/pull/515))
- [refactor] .gitmodules 削除と .gitignore 旧レイアウト残骸の掃除 (WP-Q1 PR1) ([#514](https://github.com/KingYoSun/kukuri/pull/514))
- [refactor] shell-phase1.css を連続4分割 + スタイリング層ルール文書化 (WP-H8 PR4) ([#513](https://github.com/KingYoSun/kukuri/pull/513))
- [refactor] 重複 CSS 宣言を統合(冗長 99 規則削除)(WP-H8 PR3) ([#512](https://github.com/KingYoSun/kukuri/pull/512))
- [refactor] shell-phase1-legacy.css を shell-scoped-overrides.css へ改名 (WP-H8 PR2) ([#511](https://github.com/KingYoSun/kukuri/pull/511))
- [refactor] 未使用 CSS セレクタ 34 クラスを削除 (WP-H8 PR1) ([#510](https://github.com/KingYoSun/kukuri/pull/510))
- [refactor] community-node 型を ts-rs で生成物化・生成器を desktop-runtime へ移設 (WP-H7 PR3 / Stage 3b) ([#509](https://github.com/KingYoSun/kukuri/pull/509))
- [refactor] core/transport の残り IPC 型を ts-rs で生成物化 (WP-H7 PR3 / Stage 3a) ([#508](https://github.com/KingYoSun/kukuri/pull/508))
- [refactor] metaverse/game/live 型を ts-rs で生成物化 (WP-H7 PR3 / Stage 2) ([#507](https://github.com/KingYoSun/kukuri/pull/507))
- [refactor] IPC view 型を ts-rs で生成物化 (WP-H7 PR3 / Stage 1) ([#506](https://github.com/KingYoSun/kukuri/pull/506))
- [refactor] desktopApiMock をドメイン別ファイルへ分割 (WP-H7 PR2) ([#505](https://github.com/KingYoSun/kukuri/pull/505))
- [refactor] runtimeApi の mock 分岐をディスパッチヘルパへ集約 (WP-H7 PR1) ([#504](https://github.com/KingYoSun/kukuri/pull/504))
- [refactor] page/ コンポーネントの store 由来 props を子側購読へ (WP-H6 PR4) ([#503](https://github.com/KingYoSun/kukuri/pull/503))
- [refactor] DesktopShellState をドメインスライス合成へ分割 (WP-H6 PR3) ([#502](https://github.com/KingYoSun/kukuri/pull/502))
- [refactor] shell の全ストア購読を selector 購読へ移行 (WP-H6 PR2) ([#501](https://github.com/KingYoSun/kukuri/pull/501))
- [refactor] shell の Record 更新 / AsyncPanelState ヘルパー導入 (WP-H6 PR1) ([#500](https://github.com/KingYoSun/kukuri/pull/500))
- [refactor] 購読タスク管理を SubscriptionRegistry へ集約 (WP-H5 PR5) ([#499](https://github.com/KingYoSun/kukuri/pull/499))
- [refactor] rotate_private_channel をフェーズ分割 (WP-H5 PR4) ([#498](https://github.com/KingYoSun/kukuri/pull/498))
- [refactor] private channel import 3 系統をテンプレート統合 (WP-H5 PR3) ([#497](https://github.com/KingYoSun/kukuri/pull/497))
- [refactor] AuthorViewParts 抽出と timeline_runtime_support の二分割 (WP-H5 PR2) ([#496](https://github.com/KingYoSun/kukuri/pull/496))
- [refactor] service/mod.rs の glob 再輸出を明示 import へ (WP-H5 PR1) ([#495](https://github.com/KingYoSun/kukuri/pull/495))
- [refactor] cn-user-api lib.rs / contract.rs をドメイン別に分割 (WP-H4) ([#494](https://github.com/KingYoSun/kukuri/pull/494))
- [refactor:boundary] HTTP パス定数と request 型を cn-protocol で共有 (WP-H3 PR2) ([#493](https://github.com/KingYoSun/kukuri/pull/493))
- [refactor:boundary] cn-protocol 共有 wire crate を抽出 (WP-H3 PR1) ([#492](https://github.com/KingYoSun/kukuri/pull/492))
- [refactor:boundary] IrohDocsNode を kukuri-iroh-node crate へ移動 (WP-H2 PR2) ([#491](https://github.com/KingYoSun/kukuri/pull/491))
- [refactor] docs-sync / blob-service のピア管理を kukuri-transport へ共通化 (WP-H2 PR1) ([#490](https://github.com/KingYoSun/kukuri/pull/490))
- [refactor] ProjectionStore デフォルト実装のヘルパ関数化 (WP-H1 PR2) ([#489](https://github.com/KingYoSun/kukuri/pull/489))
- [refactor:boundary] ProjectionStore をドメイン別 sub-trait に分割 (WP-H1 PR1) ([#488](https://github.com/KingYoSun/kukuri/pull/488))
- [docs] 互換パス3本の撤去条件を明文化 (WP-C8) ([#487](https://github.com/KingYoSun/kukuri/pull/487))
- [fix] 判定根拠(basis)の導出を verdict 文脈対応にし ADR 0027 §2.2 の既知ギャップを解消 (WP-C7) ([#486](https://github.com/KingYoSun/kukuri/pull/486))
- [fix] CRLF checksum 自己修復を撤去し fail-loud 化 (WP-C6) ([#485](https://github.com/KingYoSun/kukuri/pull/485))
- [fix] endpoint secret を version + hex の自前形式で永続化 (WP-C5) ([#484](https://github.com/KingYoSun/kukuri/pull/484))
- [fix] GossipHint parse 失敗を warn ログとカウンタで可観測化 (WP-C4) ([#483](https://github.com/KingYoSun/kukuri/pull/483))
- [fix] IPC エラーを { code, message } 構造化封筒へ変更し文言非依存判定にする (WP-C3) ([#482](https://github.com/KingYoSun/kukuri/pull/482))
- [docs] WP-C2 完了報告を docs/progress に追加 ([#481](https://github.com/KingYoSun/kukuri/pull/481))
- [fix] #479 マージ時に復活した export ラッパーの persist 呼び出し残骸を除去 ([#480](https://github.com/KingYoSun/kukuri/pull/480))
- [refactor:boundary] capability 永続化を AppService の write-through callback へ集約 (WP-C2 T5) ([#479](https://github.com/KingYoSun/kukuri/pull/479))
- [contract] capability registry 永続 JSON の形状 fixture を追加 (WP-C2 T4) ([#478](https://github.com/KingYoSun/kukuri/pull/478))
- [fix] export 系ラッパーの capability persist 漏れを修正 (WP-C2 T1-T2) ([#476](https://github.com/KingYoSun/kukuri/pull/476))
- [fix] keyring set 失敗時に stale entry を削除して file fallback を有効化 (WP-C2 T3) ([#477](https://github.com/KingYoSun/kukuri/pull/477))
- [docs] WP-C1 完了報告を docs/progress に追加 (WP-C1 T5 記録) ([#475](https://github.com/KingYoSun/kukuri/pull/475))
- [fix] get_sync_status を読み取り専用化し CN セッション駆動をスケジューラへ一本化 (WP-C1 T4) ([#474](https://github.com/KingYoSun/kukuri/pull/474))
- [fix] CN セッション維持を desktop-runtime 内スケジューラで駆動 (WP-C1 T1-T3) ([#473](https://github.com/KingYoSun/kukuri/pull/473))
- [docs] 参照チェーン整合(README/AGENTS 順序 + progress 分離規約 + preview checklist)(WP-S8 T3) ([#472](https://github.com/KingYoSun/kukuri/pull/472))
- [docs] REFACTORING.md に地雷リストと互換パス sunset 条件を追記 (WP-S8 T2) ([#471](https://github.com/KingYoSun/kukuri/pull/471))
- [docs] 検証マトリクスの e2e-smoke 誤要求を実態へ修正 (WP-S8 T1) ([#470](https://github.com/KingYoSun/kukuri/pull/470))
- [docs] 視覚回帰の運用手順を dev.md に追加し検証マトリクスを更新 (WP-S7 T3) ([#469](https://github.com/KingYoSun/kukuri/pull/469))
- [scenario] 視覚回帰 baseline 14 枚を追加し CI で強制化 (WP-S7 T2) ([#467](https://github.com/KingYoSun/kukuri/pull/467))
- [scenario] 視覚回帰 spec + Playwright/xtask/CI 基盤配線(baseline なし)(WP-S7 T1) ([#466](https://github.com/KingYoSun/kukuri/pull/466))
- [contract] sqlite/memory backend parity ハーネス (WP-S6 T8) ([#465](https://github.com/KingYoSun/kukuri/pull/465))
- [fix] MemoryStore の sqlite との挙動乖離 2 件を修正 (WP-S6 T7) ([#464](https://github.com/KingYoSun/kukuri/pull/464))
- [contract] pagination の keyset 純関数と複数ページ走査を characterization (WP-S6 T6) ([#463](https://github.com/KingYoSun/kukuri/pull/463))
- [contract] row_mapping のエッジ/legacy 値を生 SQL で characterization (WP-S6 T5) ([#462](https://github.com/KingYoSun/kukuri/pull/462))
- [contract] row_mapping の put→get 全列 round-trip(16 写像)(WP-S6 T4) ([#461](https://github.com/KingYoSun/kukuri/pull/461))
- [contract] row_mapping の enum⇔文字列写像 14 関数を characterization (WP-S6 T3) ([#460](https://github.com/KingYoSun/kukuri/pull/460))
- [contract] migration の世代別 round-trip + 全適用後スキーマ golden (WP-S6 T2) ([#459](https://github.com/KingYoSun/kukuri/pull/459))
- [fix] migration down の可逆性を回復(20260329 補完 + 非可逆 down 3 件修正)(WP-S6 T1) ([#458](https://github.com/KingYoSun/kukuri/pull/458))
- Refactor/s5 t3 viewmodels harness ([#457](https://github.com/KingYoSun/kukuri/pull/457))
- [contract] renderHook 共通ハーネス + useDesktopShellViewModels の characterization (WP-S5 T3) ([#452](https://github.com/KingYoSun/kukuri/pull/452))
- [contract] selectors の未カバー主要 26 関数を characterization (WP-S5 T2) ([#451](https://github.com/KingYoSun/kukuri/pull/451))
- [contract] timelineMerge / routes 純関数の characterization テスト (WP-S5 T1) ([#450](https://github.com/KingYoSun/kukuri/pull/450))
- [fix] types.ts の過剰緩和 3 フィールドを実態へ修正 (WP-S4 T1) ([#444](https://github.com/KingYoSun/kukuri/pull/444))
- [fix] zh-CN の欠落 37 キーを補完 (WP-S4 T4) ([#445](https://github.com/KingYoSun/kukuri/pull/445))
- [refactor:extract] wire prefix の重複リテラルを core::wire へ集約(値不変) ([#447](https://github.com/KingYoSun/kukuri/pull/447))
- [docs] IPC codegen spike(ts-rs 12)の評価メモと WP-H7 判断 ([#449](https://github.com/KingYoSun/kukuri/pull/449))
- [docs] REFACTORING.md に「凍結境界」章を追加 ([#443](https://github.com/KingYoSun/kukuri/pull/443))
- [contract] CommunityNodeManifest の round-trip / wire golden (WP-S3 T6) ([#441](https://github.com/KingYoSun/kukuri/pull/441))
- [contract] moderation event の digest / issuer / 署名済み fixture golden ([#440](https://github.com/KingYoSun/kukuri/pull/440))
- [contract] rendezvous / replica / gossip id 派生の golden テスト ([#439](https://github.com/KingYoSun/kukuri/pull/439))
- [contract] GossipHint / posts の wire serde snapshot (WP-S3 T2) ([#438](https://github.com/KingYoSun/kukuri/pull/438))
- [contract] 署名 canonical 3 系統の golden テスト(envelope / DM frame / DM ack) ([#437](https://github.com/KingYoSun/kukuri/pull/437))
- [refactor:move] xtask: main.rs を機能別7モジュールへ分割 (WP-S2 T1) ([#431](https://github.com/KingYoSun/kukuri/pull/431))
- [fix] desktop-lint に no-console ルールを追加(warn/info のみ許可) ([#432](https://github.com/KingYoSun/kukuri/pull/432))
- [refactor:move] apps/desktop: DesktopShellPage.test.tsx をテーマ別15ファイルへ分割 (WP-S1 T1) ([#428](https://github.com/KingYoSun/kukuri/pull/428))
- [refactor:move] app-api: tests/mod.rs のヘルパを tests/support/ へ分割 ([#430](https://github.com/KingYoSun/kukuri/pull/430))
- [refactor:move] desktop-runtime: tests/mod.rs のヘルパを tests/support/ へ分割 ([#429](https://github.com/KingYoSun/kukuri/pull/429))
- @ ([#424](https://github.com/KingYoSun/kukuri/pull/424))
- dedupe Validation sections against Feature Data Classification ([#422](https://github.com/KingYoSun/kukuri/pull/422))
- decide ADR 0026 §6 open items (trust/relation, #416) ([#421](https://github.com/KingYoSun/kukuri/pull/421))
- non-deterministic (VLM) moderation ADR ([#411](https://github.com/KingYoSun/kukuri/pull/411), [#419](https://github.com/KingYoSun/kukuri/pull/419))
- deterministic moderation (CSAM / known-hash critical safety) ADR ([#410](https://github.com/KingYoSun/kukuri/pull/410), [#418](https://github.com/KingYoSun/kukuri/pull/418))
- community node trust / relation foundation ADR ([#409](https://github.com/KingYoSun/kukuri/pull/409), [#414](https://github.com/KingYoSun/kukuri/pull/414))
- community node indexing foundation ADR ([#412](https://github.com/KingYoSun/kukuri/pull/412))
- Add PLANS.md ([#401](https://github.com/KingYoSun/kukuri/pull/401))
- operator-config.yamlをgitignore ([#395](https://github.com/KingYoSun/kukuri/pull/395))
- critical safety docから不要な記述を削除 ([#387](https://github.com/KingYoSun/kukuri/pull/387))
- community node safety architecture を整備 ([#386](https://github.com/KingYoSun/kukuri/pull/386))
- README を最新の実装状況に更新 ([#379](https://github.com/KingYoSun/kukuri/pull/379))
- default community node 依存低減ロードマップを文書化 ([#360](https://github.com/KingYoSun/kukuri/pull/360), [#374](https://github.com/KingYoSun/kukuri/pull/374))
- community node shutdown と user continuity protocol を文書化 ([#361](https://github.com/KingYoSun/kukuri/pull/361), [#373](https://github.com/KingYoSun/kukuri/pull/373))
- moderation event / safety advisory を optional trust input として文書化 ([#362](https://github.com/KingYoSun/kukuri/pull/362), [#372](https://github.com/KingYoSun/kukuri/pull/372))
- P2P-first community node の責任境界を文書化 ([#354](https://github.com/KingYoSun/kukuri/pull/354), [#363](https://github.com/KingYoSun/kukuri/pull/363))
- 監査ベースの community-node ハードニングとユーザー入力検証 ([#350](https://github.com/KingYoSun/kukuri/pull/350))
- update CHANGELOG for v0.1.2-preview.1 ([#349](https://github.com/KingYoSun/kukuri/pull/349))

## [v0.1.2-preview.1] - 2026-06-15

### Features

- リリースごとのCHANGELOG自動生成・運用を追加 ([#342](https://github.com/KingYoSun/kukuri/pull/342), [#344](https://github.com/KingYoSun/kukuri/pull/344))
- topic一覧にsearch/filter/sort機能を追加 ([#340](https://github.com/KingYoSun/kukuri/pull/340), [#343](https://github.com/KingYoSun/kukuri/pull/343))
- topic/channelごとのGossip接続トグルを追加 ([#305](https://github.com/KingYoSun/kukuri/pull/305), [#341](https://github.com/KingYoSun/kukuri/pull/341))
- リポスト・リプライ・スレッドのUIを改善 ([#307](https://github.com/KingYoSun/kukuri/pull/307), [#337](https://github.com/KingYoSun/kukuri/pull/337))
- アプデ通知を改善（バナー廃止・更新時のみDLボタン表示・リリース設定を整理） ([#333](https://github.com/KingYoSun/kukuri/pull/333))
- add Japanese font fallback and a monospace token ([#328](https://github.com/KingYoSun/kukuri/pull/328))

### Fixes

- changelog ジョブを main 直push からPR作成方式へ変更 ([#348](https://github.com/KingYoSun/kukuri/pull/348))
- third-party notices のソートをオーディナル化しCI差異を解消 ([#347](https://github.com/KingYoSun/kukuri/pull/347))
- 接続エラー復帰後に community-node エラー表示が消えない問題を修正 ([#312](https://github.com/KingYoSun/kukuri/pull/312), [#335](https://github.com/KingYoSun/kukuri/pull/335))
- OS通知をRustバックエンド経由に変更しWindowsで発火するように修正 ([#313](https://github.com/KingYoSun/kukuri/pull/313), [#334](https://github.com/KingYoSun/kukuri/pull/334))
- unify shell breakpoints to the 759/899/900/1099/1100 system ([#331](https://github.com/KingYoSun/kukuri/pull/331))
- resolve undefined CSS custom-property references in shell styles ([#327](https://github.com/KingYoSun/kukuri/pull/327))

### Other

- regenerate third-party notices for 0.1.2 ([#346](https://github.com/KingYoSun/kukuri/pull/346))
- bump preview version to 0.1.2 ([#345](https://github.com/KingYoSun/kukuri/pull/345))
- AGENTS.local.mdを追加 ([#339](https://github.com/KingYoSun/kukuri/pull/339))
- @ ([#338](https://github.com/KingYoSun/kukuri/pull/338))
- codegraph導入 ([#336](https://github.com/KingYoSun/kukuri/pull/336))
- tokenize elevation, blur, and the metaverse canvas color ([#332](https://github.com/KingYoSun/kukuri/pull/332))
- tokenize spacing and radius into --space-* / --radius-* scales ([#330](https://github.com/KingYoSun/kukuri/pull/330))
- consolidate font-sizes into a --text-* type scale ([#329](https://github.com/KingYoSun/kukuri/pull/329))
- Rework DESIGN.md into a concrete visual design spec ([#326](https://github.com/KingYoSun/kukuri/pull/326))
- Update release readiness manual items ([#324](https://github.com/KingYoSun/kukuri/pull/324))
- Add startup database error screen ([#323](https://github.com/KingYoSun/kukuri/pull/323))
- Add store migration fixture ([#322](https://github.com/KingYoSun/kukuri/pull/322))
- Generate third-party notices ([#321](https://github.com/KingYoSun/kukuri/pull/321))
- Add updater error guidance ([#320](https://github.com/KingYoSun/kukuri/pull/320))
- Fix community node settings notice link ([#317](https://github.com/KingYoSun/kukuri/pull/317))

