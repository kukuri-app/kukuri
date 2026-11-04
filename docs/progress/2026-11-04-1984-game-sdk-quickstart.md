# Game SDK のクリーン導入と手順書（#1984 AC-3）

2026-11-04 に、CI が作った配布候補を、開発用の profile と設定を引き継がない新しい Unity の project へ入れて確かめた。手順書は [Game SDK の導入の手順](../runbooks/game-sdk-quickstart.md)、確認の script は `sdk/unity/smoke-dist.ps1` である。公開はしていない。

## 実行の条件

| 項目 | 値 |
| --- | --- |
| source | `748c552e6`（PR #2028 の head `db73a1851` を main `dc9aae51c` へ merge した、CI の commit） |
| CI の run | [41778076781](https://github.com/kukuri-app/kukuri/actions/runs/41778076781) |
| 版 | 0.1.0 |
| Unity | 6000.3.27f2（Windows 11、正規の license）。ADR 0070 §8.1 の 6000.3.27f1 と同じ 6000.3 系 |
| Rust | PATH から外し、`cargo` が見つかれば script が止まる |
| desktop | main `dc9aae51c` の release（`tauri/custom-protocol` 付き）を手元で build した。SDK の配布物ではなく、利用者が持っている kukuri app の役である |
| 外への通信 | 試験は `static_peer` で `127.0.0.1` につなぐ。投稿は試験の topic へ書く |

## 手動の操作と作業時間

作業の手動の操作は、Unity Hub のサインイン（license の認証。ユーザーが事前に実施）だけである。外部の担当者は要らない。

それ以外は script が行う。Unity の画面を人が操作した確認ではなく、Player は実際の build を起動し、`KukuriAccountScreen` の操作を試験の script から呼んでいる。desktop は画面の操作を CDP で動かす。

全体で 28 分だった。

| 段 | 時間 | 確かめたこと |
| --- | --- | --- |
| package-mono | 315 秒 | 空の project へ `.tgz` を入れ、EditMode の試験（21 件中 19 件が通過、失敗 0、描画が要る 2 件は省略）、Mono の Player、U1〜U5 |
| package-il2cpp | 139 秒 | IL2CPP の Player、U1・U2・U5 |
| netcode | 99 秒 | sample「Netcode Sync」。FFI と Sidecar の両方で 2 peer が同期した |
| graphics | 68 秒 | sample「Skin Loader」と Live。Live は desktop へ 32 frame、ゲームへ 30 frame が届いた |
| arena-mono | 391 秒 | sample の展開と Arena の全項目（掲示板、Live、機能ごとの最小の scene、対戦、スキン、退出、状態の保存） |
| arena-il2cpp | 486 秒 | 同じ項目を IL2CPP で |
| partial | 107 秒 | 部分導入（下記） |

## U1〜U5 の結果

| ID | 結果 | 確かめたこと |
| --- | --- | --- |
| U1 | 通過 | 空の profile で同意 →「はじめる」→ プレイ → 再起動で、同じ game player と公開鍵のまま再開した。別のサイトの登録、メール、パスワード、SNS 用のプロフィールは通らない。Sidecar の process は無い |
| U2 | 通過（模擬） | Steam Cloud への保存を模し、保存した profile の `game-account.json` を写した先で、同じ公開鍵と game player ID に戻った。**実際の Steam は未確認**（2026-11-04 のユーザー判断） |
| U3 | 通過 | desktop の「リンクをコピー」を Player の「kukuriアカウントを使う」→「貼り付け」で受け、両方で同じ確認コードを承認した。鍵の文字列は入力しない。次の起動でも同じ desktop の公開鍵と game player で再開した |
| U4（引継ぎ） | desktop は通過 | Player の「kukuriで続ける」のリンクを desktop で受け、desktop がゲームと同じ公開鍵に切り替わった。desktop の返信を Player が受け、同じ game player のまま続いた |
| U4（統合） | 通過 | 「既存のアカウントへまとめる」で両方の承認の後、game player を保ったまま desktop のアカウントへ結び付けが移り、再起動でも続いた。まとめた後の投稿は desktop のアカウントの投稿として timeline に出た |
| U5 | 通過（模擬） | 一度も開いていない profile へ `game-account.json` だけを写して再開した。元の端末の profile、token、鍵は使わない。**実際の Steam は未確認** |

U4 の Web と旧投稿については、担当 Issue の証拠を使った。

- Web へ移す経路: Player から Web へ移す組み合わせは動かしていない。同じ `create_account_transfer_invite` の招待を Web が受ける経路は、Web の E2E（`apps/desktop/tests/web-e2e/main-flow.mjs` の `transfer`）が確かめている。
- 統合後の旧投稿の ID・署名・著者: `crates/game-sdk/tests/account_transfer.rs` の `the_game_account_is_merged_into_an_existing_account_and_its_relations_follow` が確かめている。A の投稿は A の著者のまま読め、その thread へ B が返信できる。この試験は配布候補の CI の run に含まれる。

## 部分導入

NGO の Unity Transport で cube の位置を同期するゲームへ、掲示板だけを足した（`sdk/unity/test-partial.ps1`）。

| 項目 | 結果 |
| --- | --- |
| 足した package | `com.kukuri.gamesdk` の 1 行 |
| 足した scene の object | `kukuri`（`KukuriRunner`・`KukuriAccountScreen`）と `KukuriBoard` |
| ゲームの script | 変更なし（SHA-256 が一致） |
| ゲームの Transport | 両方の Player で `UnityTransport` のまま |
| 掲示板 | host の投稿が client に届き、その後もゲームの同期が続いた |

## 手順書との照合

| 項目 | 手順書の節 | 照合の内容 |
| --- | --- | --- |
| 導入 | 導入 | sample の相対の path と、空の project の絶対の path の両方で読み込めた |
| 制約・未対応の環境 | 対応環境 | Windows 11 x64、Direct3D 12、Unity 6000.3 だけを対応とし、確かめていない環境を含めていない |
| 費用 | 費用と運用 | seed、保持する端末、Community Node・relay、Steam の役割と負担者を書いた。性能や料金を無条件に保証しない（INVAR-2） |
| 原因別の診断 | 診断と停止 | code と画面の名前が実装と一致することを確かめた |

## 残したこと

- Player から Web へ移す組み合わせは、上記のとおり実行していない。
- 実際の Steam は確かめていない。Steamworks の AppID と登録料はゲームの作者が用意する。
