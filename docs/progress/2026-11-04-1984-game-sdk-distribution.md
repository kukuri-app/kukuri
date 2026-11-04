# Game SDK の配布候補（#1984 AC-2）

2026-11-04 に、CI で Rust の SDK の試験と配布候補の生成・照合を通し、CI が作った artifact を手元の Unity で確かめた。手順は [Game SDK の配布候補](../runbooks/game-sdk-distribution.md)。公開はしていない。

## 範囲の判断

2026-11-04 のユーザー判断で、Unity の Editor の試験と Player の smoke は CI につながず、作者の手元の Unity で配布物から確かめる。Unity の license の認証情報を CI に置かない。Unity を CI で動かさないので、Player の実行物は配布候補に含めず、手元の smoke で build する。

## 確かめた候補

| 項目 | 値 |
| --- | --- |
| source | `6bc07c0f4`（PR #2027 の head `c2f8673a6` を main `e75ef2357` へ merge した、CI の commit） |
| CI の run | [41767443628](https://github.com/kukuri-app/kukuri/actions/runs/41767443628) |
| 版 | 0.1.0 |
| Rust | 1.98.1 |
| libvpx | 1.17.0（tag の commit を固定） |
| 手元の Unity | 6000.3.27f2（Windows 11、正規の license） |

ADR 0070 §8.1 の 6000.3.27f1 ではなく、手元に入っている同じ 6000.3 系の 6000.3.27f2 で確かめた。

## CI の結果

- Rust の SDK の試験 52 件がすべて通った。Live の 2 件は、実アプリの試験と同じ仮想の音声の機器（VB-CABLE）を入れて動かす。poll の p99 ≤ 1 ms は指定の PC の値（§8.4）なので、CI の共有の runner では判定しない（手元と SLO の計測で判定する）。
- 配布候補の照合が通った。artifact を手元へ取得して、もう一度照合しても通った。
- DLL と Sidecar が読み込むのは Windows の DLL だけである。これまで依存していた `VCRUNTIME140.dll` と `api-ms-win-crt-*` は、CRT を静的に link して無くした（§8.1）。

## 手元の Unity の結果

`sdk/unity/smoke-dist.ps1` は全段が通った。

| 段 | 結果 |
| --- | --- |
| sample を `.tgz` の隣に展開し、sample の manifest のまま開いて compile する | 通過 |
| 空の project に `.tgz` を入れる: Mono Editor の EditMode の試験 | 全 21 件のうち 19 件が通過、失敗 0、省略 2（描画が要る Live の試験。`-nographics` では動かせない） |
| 同じ project: Mono の Player | 通過（FFI で起動の状態を読む、開始の画面と再開、Steam Cloud の file からの再開を模した確認） |
| 同じ project: IL2CPP の Player | 通過（同じ確認） |
| sample と package の Arena: Mono の Player | EditMode 11 件と Player の全項目が通過 |
| sample と package の Arena: IL2CPP の Player | Player の全項目が通過 |

Arena の Player の項目は、掲示板、Live の配信と観戦、Prefab ごとの最小の scene、対戦、スキンの発行と更新、退出である。desktop・Web を付けた相互運用は AC-1 で確かめたので、ここでは付けていない。

## 途中で見つけて直したもの

- DLL が VC++ の再頒布 package に依存していた（上記）。
- `crates/game-sdk/tests/live.rs` の競合: release の build では視聴の状態の知らせが応答待ちの間に届き、`call` が捨てていた。
- `sdk/unity/test.ps1` の profile の path が、project の dir の名前に関わらず `kgs-test` に固定されていた。

## 残したこと

- 音声の出力の機器が無い PC では、音声の track がある Live の映像も届かない（視聴が再生の機器を開くところで失敗する。runner で確かめた）。AC-2 の範囲外なので、利用者の体験の判断として別に扱う（[#2029](https://github.com/kukuri-app/kukuri/issues/2029) で直した。CI の VB-CABLE も外した）。
- GitHub の Linux の runner で、ディスク不足（`No space left on device`）の job の失敗がこの 2 日で 3 回出た。差分とは関係なく、再実行で通った。
