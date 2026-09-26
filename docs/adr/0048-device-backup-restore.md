# ADR 0048: 端末内データの暗号化バックアップと復元

## Status

Accepted

## Context

- Issue #855（Parent #853、Depends on #859）。#859 は本人性を表すアカウント鍵だけを移行できるが、SQLite、未同期データ、下書き、添付、設定、非公開チャネル能力は移行しない。
- #859 以後は複数アカウントを扱うため、初版の完全バックアップは「現在選択中の1アカウント」を単位とする。複数アカウントはそれぞれバックアップする。
- バックアップには秘密鍵、DM、非公開チャネル能力、招待情報、未公開下書きが含まれ得る。平文の一時アーカイブを作らず、利用者が指定したパスフレーズで暗号化する必要がある。

## Decision

### 1. 1ファイル、1アカウント

- `kukuri-device-backup.v1` は、1アカウントの管理情報と全entryをまとめた単一ファイルとする。
- ファイルは固定header、暗号化manifest、連番の暗号化chunkからなる。大容量Blobをfrontend IPCやメモリへ一括展開しない。
- 作成先と同じdirectoryの一時ファイルへ書き、完了時だけrenameする。失敗またはcancel時は一時ファイルを削除する。

### 2. 暗号化と検証

- 鍵導出は#859と同じArgon2id、暗号化はXChaCha20-Poly1305を使う。headerの形式版、KDF parameter、salt、nonce prefixを各chunkのAADへ束縛する。
- manifestは暗号化し、公開鍵、作成アプリ版、entry名、長さ、BLAKE3 hash、含有範囲を保持する。frontend state自体は暗号化entryに置く。
- chunk番号をnonceとAADへ含め、欠落、並べ替え、重複、切詰め、余分なchunkを拒否する。未知形式版、KDF上限超過、entry数・entry長・総容量の上限超過は復元前に拒否する。
- パスフレーズ、秘密鍵、復号内容はログ、診断、`Debug`、進捗eventへ出さない。

### 3. 対象と除外

- 必須: account key、整合したSQLite（保護所有先の台帳を含む）、保護された`kukuri.remote-blobs/`のfile、discovery/Community Node接続設定、private channel能力、gossip購読状態、Community Node招待情報、下書き、workspace layout、theme、locale。
- 旧`iroh-data`（remote内容が混在するSDKのDocs/Blob）は含めない（§7、#1221 R5-G）。
- 移行不可: iroh endpoint secret、Community Node bearer token、実行中session、通知cursor、OS通知権限。
- 再同意: app-level同意、Community Node同意、18歳以上の自己申告。これらの記録はバックアップへ含めない。成人向け表示設定も含めず、復元後はOFFとする。
- バックアップはCommunity Node、他端末、Direct P2P参加者が保持するcopyを削除または巻き戻さない。

### 4. 整合スナップショット

- active runtimeを停止してSQLite pool、Docs、Blob、background syncを閉じた後にファイルを列挙する。main DBを正とし、close timeoutなどで残ったWAL/shmも失わないよう存在時は同じarchiveへ含める。
- identityとoptional secretは保存backendに依存せず論理値として読み出す。keyring名やfile fallback名はarchive formatへ露出させない。
- runtimeを止める前に、旧`iroh-data`から保護所有先への移行（§7）の残りを128件ずつ終端まで写す。停止後、同じSQLiteを1本の接続で読み、全kindが終端へ達していることを確かめてから、保護された`kukuri.remote-blobs/`のfileを列挙する。達していなければbackupを作らない。

### 5. 復元と競合

- 復元は専用staging directoryで全entryの認証、長さ、hash、path、公開鍵を検証し、runtime、Iroh endpoint、remote取得taskを構築しないvalidation-only pathでSQLite migrationと永続設定を検証してから行う。
- 公開鍵が未登録なら新規アカウントとして追加する。同じ公開鍵が登録済みなら対象を表示し、明示的な置換確認がある場合だけアカウントdirectory全体を置換する。内容単位のmergeは行わない。
- 置換前directoryはrollback用に退避し、`Installing`、`Installed`、`Committed`、`AwaitingConsent`、`Activated`のjournalでprocess停止をまたいで追跡する。起動時は同意判定や通常runtime構築より先にjournalを回収し、未commitなら旧directory、identity、optional secret、registryへrollbackする。
- registry更新後はapp-level同意と18歳以上の自己申告をresetし、`AwaitingConsent`として停止する。復元対象の通常runtime、remote取得、scheduler、background通知は、利用者が明示的に再同意し、runtime構築が成功するまで開始しない。
- 再同意後のruntime構築成功時に`Activated`へ進め、rollback用directoryとjournalを削除する。cleanup中に停止した場合は次回起動で完遂し、runtime構築または`Activated`の永続化に失敗した場合は旧状態へrollbackする。
- 復元成功時は対象アカウントをactiveにする。frontend stateはjournalでactivationまで保持し、`Activated`後にdurable markerへ移してから、明示したkeyだけをlocalStorageへ適用する。適用またはmarker確認応答がerrorを返した場合は今回のlocalStorage変更を元へ戻し、markerを残す。process停止ではmarkerを残したまま、次回起動時に同じ復元値を再適用する。cancelはdurableなinstall開始前まで受け付け、`Installing`以後はcancel操作を表示しない。

### 6. 設定画面のフロー

- 対象利用者は端末故障への備え、または別端末への移行を行うdesktop利用者とする。単一目的は、鍵だけの移行と端末全体の移行を混同せず、安全に1アカウントを持ち出すことである。
- 設定では「バックアップと復元」を「アカウント」（鍵だけのexport／import、切り替え）と別のsectionに置き、両sectionの冒頭で対象の違いを説明して相手のsectionへ移動できるようにする。Control Centerの「システム」からも「バックアップと復元」へ直接入る。入口の表示・移動はsectionとURLだけを変え、backup作成、復元、鍵export、file選択を開始しない（#967）。
- 設定navは、Tauri既定window（1280×840）でも全sectionがnavの可視域に収まる密度にする。navの末尾がnav内scrollでしか現れない配置は、入口の未発見として扱う（#967）。
- 作成は説明・秘密情報警告・確認checkbox・パスフレーズと確認入力・native保存先選択・進捗・cancel・成功／失敗を持つ。復元はnativeファイル選択・パスフレーズ・内容preview・任意設定の適用・既存アカウント置換確認・進捗・cancel・失敗回復を持つ。
- 狭幅では1columnを維持し、長いpathと公開鍵は折り返す。pointerとkeyboardの同じcontrolを使い、native dialog以外の操作にdragやhoverを必須としない。
- offlineでもローカルファイルの作成・preview・復元は可能とする。runtime停止中のネットワーク同期は復元後の明示的な再同意とruntime activation後に再開し、remote copyを削除したような表示はしない。
- 非目標は複数アカウント一括選択、バックアップ内容の個別編集、クラウド同期、旧端末の遠隔削除である。

### 7. 保護データの移行と旧領域（#1221 R5-G、2026-09-26）

- 旧`iroh-data`にしか無い本人のデータ（本人投稿の本文・添付・state・envelope・media manifest・プロフィールの行、bookmark、private参加状態の現epochの記録、未送信outboxのframeと暗号化添付、DM履歴の平文添付、custom reaction bookmarkのasset、本人のprofile avatar、自作のcustom reaction asset、自作Domeのpin済みasset、自分が署名したlive/gameのmanifest）を、R5-Aのremote cacheと保護参照（大きいblobは`kukuri.remote-blobs/`のfile）へ移す。新しいstoreは作らない。ownerの参加者名簿はR5-Hの参加record配送で扱う。
- 移行はkindごとの索引（`envelopes`・bookmark・DMの行、live/gameの反映時刻、capabilityの一覧、SDKのpin tag）を1回128件以内のcursorで歩き、BLAKE3（blob）とcontent hash（record）で照合してから写し、`protected_migration`へ位置と終端へ達した時刻を保存する。途中で止まれば保存した位置から同じ結果でやり直す。旧領域に無いものは写さずに進み、旧領域から何も読めなかった参照（復元後・旧領域の回収後）は置き換えず、既にある保護を減らさない。
- 保護参照はindex行の寿命に従う。bookmarkとcustom reaction bookmarkの解除、DMのACKと手元の削除で、同じtransactionの中で外す。privateの記録は旧領域が無くても、capabilityを持つ間はkey指定の手元の読み出しで読める。
- 旧領域を削除できる前提: R5-Hのwriter切替（本人の新しい書込みを旧領域へ入れない）を永続化した後に、全kindの`caught_up_at`がその時刻より後になっていること（`SqliteStore::protected_migration_caught_up_at`）。削除そのもの（有限単位の回収）はR5-Iで行う。
- 旧案の失効: `iroh-data`全体を毎回backupへ含める案（component version 1）は失効した。component version 2だけを作成・復元し、1の復元は既存状態を変更せず拒否する。

## Consequences

- データ分類は`docs/legal/device-backup-data-classification.md`を正とする。
- 含める内容は外側のbackup formatとは独立したcomponent versionを持つ。現行は2（保護所有先、§7）で、非対応component版（旧`iroh-data`入りの1を含む）は既存状態を変更せず拒否する。
- 初版は複数アカウント一括archive、クラウド保管、定期実行、内容mergeを提供しない。
