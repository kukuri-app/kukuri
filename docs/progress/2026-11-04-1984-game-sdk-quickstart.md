# Game SDK のクリーン導入と手順書（#1984 AC-3）

2026-11-04 に、CI が作った配布候補を、開発用の profile と設定を引き継がない新しい Unity の project へ入れて確かめた。手順書は [Game SDK の導入の手順](../runbooks/game-sdk-quickstart.md)、確認の script は `sdk/unity/smoke-dist.ps1` である。公開はしていない。

## 実行の条件

| 項目 | 値 |
| --- | --- |
| source | `ca4860abd`（PR #2028 の head `6fa53af75` を main `dc9aae51c` へ merge した、CI の commit） |
| CI の run | [41806366542](https://github.com/kukuri-app/kukuri/actions/runs/41806366542) |
| 版 | 0.1.0 |
| Unity | 6000.3.27f2（Windows 11、正規の license）。ADR 0070 §8.1 の固定をこの版へ替えた（2026-11-04 のユーザー判断） |
| Rust | PATH から外し、`cargo` が見つかれば script が止まる |
| desktop | main `dc9aae51c` の release（`tauri/custom-protocol` 付き）を手元で build した。SDK の配布物ではなく、利用者が持っている kukuri app の役である |
| keyring | smoke は `KUKURI_DISABLE_KEYRING=1` で動く。OS の keyring の経路は、下の「keyring を有効にした確認」で別に確かめた |
| 外への通信 | 端末どうしの通信は `static_peer` で `127.0.0.1` につなぐ。`static_peer` でも公開の Mainline DHT へは出る（#1825）。投稿は試験の topic へ書く。Arena の試験は画像つきの投稿を作るので、公開の添付の hash の告知と補助 index への登録も起きうる |

## 手動の操作と作業時間

作業の手動の操作は、Unity Hub のサインイン（license の認証。ユーザーが事前に実施）だけである。外部の担当者は要らない。

それ以外は script が行う。Unity の画面を人が操作した確認ではなく、Player は実際の build を起動し、`KukuriAccountScreen` の操作を試験の script から呼んでいる。desktop は画面の操作を CDP で動かす。時間は script の実行時間で、全体で 30 分だった。

| 段 | 時間 | 確かめたこと |
| --- | --- | --- |
| package-mono | 335 秒 | 空の project へ `.tgz` を入れ、EditMode の試験（22 件中 20 件が通過、失敗 0、描画が要る 2 件は省略）、Mono の Player、U1〜U4 の desktop の経路、U5 |
| package-il2cpp | 143 秒 | IL2CPP の Player、U1・U2・U5 |
| netcode | 99 秒 | sample「Netcode Sync」。FFI と Sidecar の両方で 2 peer が同期した |
| graphics | 72 秒 | sample「Skin Loader」と Live。省略した 2 件の Live の試験もここで動く。配信を Sidecar の profile が 32 frame 受け、ゲームに 30 frame 出た |
| arena-mono | 399 秒 | sample の展開と Arena の項目（掲示板、Live、機能ごとの最小の scene、対戦、スキン、退出、状態の保存）。desktop・Web を付けた段は除く（AC-1 で確かめた） |
| arena-il2cpp | 496 秒 | 同じ項目を IL2CPP で |
| partial | 120 秒 | 部分導入（下記） |

## U1〜U5 の結果

| ID | 結果 | 確かめたこと |
| --- | --- | --- |
| U1 | 通過 | 空の profile で同意 →「はじめる」→ プレイ → 再起動で、同じ game player と公開鍵のまま再開した。別のサイトの登録、メール、パスワード、SNS 用のプロフィールは通らない。Sidecar の process は無い |
| U2 | 通過（模擬） | Steam Cloud への保存を模し、保存した profile の `game-account.json` を写した先で、同じ公開鍵と game player ID に戻った。**実際の Steam は未確認**（2026-11-04 のユーザー判断） |
| U3 | 通過 | desktop の「リンクをコピー」を Player の「kukuriアカウントを使う」→「貼り付け」で受け、両方で同じ確認コードを承認した。鍵の文字列は入力しない。次の起動でも同じ desktop の公開鍵と game player で再開した |
| U4（引継ぎ） | desktop は通過 | Player の「kukuriで続ける」のリンクを desktop で受け、desktop がゲームと同じ公開鍵に切り替わった。ゲームの投稿を同じ ID と著者で読んで返信し、その返信を Player が受け、同じ game player のまま続いた |
| U4（統合） | 通過 | 「既存のアカウントへまとめる」で両方の承認の後、game player を保ったまま desktop のアカウントへ結び付けが移り、再起動でも続いた。まとめた後の投稿は desktop のアカウントの投稿として timeline に出た |
| U5 | 通過（模擬） | 一度も開いていない profile へ `game-account.json` だけを写して再開した。元の端末の profile、token、鍵は使わない。**実際の Steam は未確認** |

U4 の Web と統合後の旧投稿は、担当 Issue の証拠を使った。

- Web へ移す経路: Player から Web へ移す組み合わせは動かしていない（2026-11-04 のユーザー判断）。同じ `create_account_transfer_invite` の招待を Web が受ける経路は、Web の E2E（`apps/desktop/tests/web-e2e/main-flow.mjs` の `transfer`）が native の移行元から確かめている。
- 統合後の旧投稿: `crates/game-sdk/tests/account_transfer.rs` の `the_game_account_is_merged_into_an_existing_account_and_its_relations_follow` が、A の投稿が同じ ID と A の著者のまま読め、その thread へ B が返信できることを確かめている。この試験は配布候補の CI の run に含まれる。署名は、受信時の検証を通った投稿だけが読めることによる。この試験で署名そのものは比べていない。

## keyring を有効にした確認

smoke は keyring を無効にして動くので、利用者の既定の経路を別に 1 回通した（2026-11-04 のユーザー判断）。`test.ps1` の写しから `KUKURI_DISABLE_KEYRING` を外し、配布候補 `748c552e6` の Mono の Player で動かした。これ以降の SDK の変更は掲示板の読む位置だけで、鍵の経路は変わっていない。

| 確かめたこと | 結果 |
| --- | --- |
| U1 | 通過。開始、再開で同じ公開鍵と game player |
| 鍵の置き場所 | この project の項目は資格情報マネージャーに 2 件（前の段で消した profile の項目を含む）。profile にアカウントの鍵の file は無い（`game-account.json` の写しを除く。`endpoint-secret.json` は端末の通信用の鍵） |
| U2・U5 | 通過。keyring の項目と profile を消し、`game-account.json` だけから同じ公開鍵と game player に戻った |
| profile の dir を消した後 | keyring の項目を残したままでも、次の起動は新しいアカウントと game player で始まる。手順書の記述をこれに合わせた |

試験が作った項目は `cmdkey /delete` で消した。`db:` の項目の数は実行の前後で同じ 7 件である。

## 部分導入

NGO の Unity Transport で cube の位置を同期するゲームへ、手順書の「自分のゲームに掲示板だけを足す」のとおりに掲示板を足した（`sdk/unity/test-partial.ps1`）。host は設定 asset の既定の profile で開いた。同じ PC で同じ profile を 2 つの process が使えないので、client だけ試験の script が profile の dir を替えている。

| 項目 | 結果 |
| --- | --- |
| 足した package | `com.kukuri.gamesdk` の 1 行 |
| 足したもの | 設定 asset、`BoardOpener.cs`（手順書の script）、`kukuri`（`KukuriRunner`・`KukuriAccountScreen`・`BoardOpener`）、`KukuriBoard` |
| ゲームの script | 変更なし（SHA-256 が一致） |
| ゲームの Transport | 両方の Player で `UnityTransport` のまま |
| 掲示板 | host の投稿が client に届き、その後もゲームの同期が続いた |

## 手順書との照合

| 項目 | 手順書の節 | 照合の内容 |
| --- | --- | --- |
| 導入 | 導入 | sample の相対の path と、空の project の絶対の path の両方で読み込めた |
| 開始 | ゲームを始める | 設定 asset から開く構成を部分導入で動かした |
| 制約・未対応の環境 | 対応環境 | Windows 11 x64、Direct3D 12、Unity 6000.3.27f2 だけを対応とする |
| 費用 | 費用と運用 | seed、保持する端末、Community Node・relay、Steam の役割と負担者を書いた。性能や料金を無条件に保証しない（INVAR-2） |
| 原因別の診断 | 診断と停止 | code と画面の名前が実装と一致することを確かめた。診断の各症状を smoke で起こしてはいない |

## 途中で見つけて直したもの

- 手順書に設定 asset の作成と `KukuriRunner` への割当てが無く、手順どおりでは SDK が開かなかった（独立監査）。当時の試験は設定を code で渡していたので気づけなかった。手順書を直し、部分導入の試験を手順書と同じ構成にした。
- 掲示板の thread が、同じ秒に書かれて ID が小さい返信を読み飛ばしていた。投稿の時刻は秒単位で、最後の行より後だけを読んでいたためである。smoke の EditMode の試験が揺れて分かった。SDK の command だけの反復で、投稿の直後の読取りの約 200 回に 16 回で再現した。最後の行の秒の先頭から読むように直した。同じ秒の読んだ行だけで頁が埋まったときは、その先を「さらに表示」で読む。
- `BoardTests` の 2 か所が、同じ秒でも作った順に並ぶと決めつけていた。

smoke は、上の修正の途中で 3 回実行した。1 回目は全段が通り、2 回目は `package-mono` の EditMode の試験だけが掲示板の不具合で落ち、3 回目は全段が通った。ここに書いた結果は、すべてを直した後の 4 回目である。

## 残したこと

- Player から Web へ移す組み合わせは実行していない。
- 実際の Steam は確かめていない。Steamworks の AppID と登録料はゲームの作者が用意する。
