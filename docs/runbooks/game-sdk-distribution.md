# Game SDK の配布候補

Unity の Game SDK（ADR 0070）の Windows/UPM の配布候補を作り、照合し、手元の Unity で確かめる手順。公開は別途の明示の指示で行う。既存の SNS の release の成果物と更新の仕組みとは別物で、それらを置き換えない。

## 配布物

CI の workflow `Kukuri Game SDK` が、同じ commit から次を作り、artifact `kukuri-game-sdk-<commit>` に置く。

| file | 内容 |
| --- | --- |
| `com.kukuri.gamesdk-<版>.tgz` | UPM package。commit の `sdk/unity/com.kukuri.gamesdk` に、DLL・Sidecar と、`Documentation~` の license・第三者 notice を足したもの |
| `cyber-disc-arena-unity-<版>.zip` | sample project（commit の `examples/cyber-disc-arena-unity` から開発用の試験を除いたもの）。隣の `.tgz` を読む |
| `kukuri_game_sdk.dll`・`kukuri-sdk-sidecar.exe` | package に入れたものと同じ binary |
| `build-info.json` | 版、source の commit、Rust・libvpx・Unity の版、package の依存、binary が読み込む Windows の DLL、各 file の SHA-256 |
| `SHA256SUMS` | 上の全 file の SHA-256 |

版は `sdk/unity/com.kukuri.gamesdk/package.json` の `version` に揃える。DLL と Sidecar には、版と source の commit を印として埋め込む（`crates/game-sdk/build.rs`）。

## CI が確かめること

`scripts/release/game_sdk_dist.py` の `verify` は、次のどれかが成り立たなければ失敗する。成果物が欠けた候補を成功として扱わない。

- artifact の file の一覧が過不足なく揃い、`SHA256SUMS` と `build-info.json` が中身と一致する。
- DLL と Sidecar の印が、指定した commit と版に一致する。
- DLL と Sidecar が読み込む DLL が Windows の DLL だけである（VC++ の再頒布 package を要らなくする。ADR 0070 §8.1）。
- package と sample の中身が、commit から組み立て直した内容と 1 byte まで一致する。手元の未追跡の file と profile は入らない。
- 秘密や個人の profile に当たる file 名（`kukuri.identity*`・`*.db`・`.env*` など）と、秘密鍵や token の文字列が無い。

CI は Rust の SDK の試験（`cargo test -p kukuri-game-sdk --features vp8`）も動かす。PR では SDK・sample・配布の script の変更で動き、SDK が依存する crate の変更は毎晩の main で確かめる。

## 手元の Unity で確かめる

Unity は CI につながない（2026-11-04 のユーザー判断。Unity の license の認証情報を CI に置かない）。正規の license の Unity 6000.3 が入った手元の Windows で、配布物から確かめる。Rust は要らない。

```powershell
git checkout <commit>
gh run download <run id> --repo kukuri-app/kukuri --name kukuri-game-sdk-<commit> --dir C:\kgs-dist
powershell -ExecutionPolicy Bypass -File sdk/unity/smoke-dist.ps1 -Dist C:\kgs-dist -Source <commit> -Unity "C:\Program Files\Unity\Hub\Editor\<版>\Editor\Unity.exe"
```

`smoke-dist.ps1` は次を順に行い、結果を `%TEMP%\kgsd\smoke-result.txt` に書く。1 つでも失敗すれば止まる。

1. 配布物を commit と照合する。
2. sample を `.tgz` の隣に展開し、sample の manifest のまま開いて compile する。
3. 空の project に `.tgz` を入れ、Mono Editor の EditMode の試験と、Mono・IL2CPP の Player を動かす（`sdk/unity/test.ps1`）。
4. sample と展開した package で、Arena の試験を Mono・IL2CPP の Player で動かす（`examples/cyber-disc-arena-unity/test.ps1`。試験の script だけは repository の同じ commit のものを使う）。

作業の dir は 1 回で十数 GB を使う。終わったら `%TEMP%\kgsd` を消す。
