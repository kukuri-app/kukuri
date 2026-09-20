# #1138 CN runtime image の build 成果物混入修正

## 現在判定

- Issue: https://github.com/kukuri-app/kukuri/issues/1138
- 状態: ローカル AC-1 / AC-2 / INVAR-1 検証 PASS。PR CI / merge 待ち。
- Scope revision: 2026-09-18。リスク区分 B、fix。
- 基準 commit: `c29f073b03b924815e4b766b6a6aed8d720bed5b`。
- AC-1: 対象 binary だけを `/usr/local/bin/app` へ直接 COPY し、`/tmp/release` を持たない。
- AC-2: 4 image の gzip 圧縮 layer 合計が ac9f93ce 以下、relay binary layer は数十 MB 以下、各 entrypoint と既存 indexer smoke が成功する。
- INVAR-1: 共通 builder の package ごとの `cargo build --release -p "$package"` を変更しない。
- 対象外: base image、build 方法、binary の機能、本番 rollout、VM 清掃・容量変更。

## 修正前の再現

現行 Dockerfile は release directory 全体を COPY してから app を cp していた。修正前のローカル `cn-indexer-smoke:local`（config ID `sha256:96d77458c7b360f8e51a4054c3fdce0f154cba81522cd3b2d10f9f54b8a6b52a`）に対して以下を実行した。

```sh
docker run --rm --entrypoint sh cn-indexer-smoke:local -ec 'test ! -e /tmp/release; test -x /usr/local/bin/app'
```

exit 1。`/tmp/release` は 2.5G あり、cn-user-api / cn-iroh-relay / cn-cli / cn-indexer と各 `.d` が存在した。同じ filesystem assertion を修正後の4 image にも適用する。
公開 v0.2.6-preview.1 の manifest も取得し、4 image に同じ巨大 layer があることと、下表の圧縮 byte 数を確認した。

## 変更と inventory

Dockerfile の directory COPY と後続 cp を対象 binary の直接 COPY に置換した。base image、runtime package（ffmpeg 等）、ENTRYPOINT、builder と bake の package list は不変。

`scripts/ci/cn_image_check.py` は本番 OCI の各 blob digest / size、platform、全 layer、ENTRYPOINT を検査し、同じ config / diff_ids で Docker に読み込む。圧縮サイズは attestation を除外した linux/amd64 manifest の layer descriptor 合計。検証失敗なら publish 前に停止する。実行用の一時 image tag と専用 internal network / container / 匿名 volume のみを終了時に除去する。

既存 indexer smoke は `scripts/ci/cn_indexer_smoke.sh` に移し、本番 OCI から読み込んだ image を渡す。旧 workflow 本文との機械比較で、image 引数化以外の全 case / assertion が不変であることを確認した。

| ID | 入口 | helper / sink | 契約・検証 |
| --- | --- | --- | --- |
| INV-1 | bake user-api / iroh-relay / cli / indexer | 共通 builder → TARGET_BIN → runtime OCI | AC-1 / AC-2 / INVAR-1。全 layer、最終 filesystem、圧縮サイズ、4 entrypoint |
| INV-2 | bake indexer-smoke | 同じ stage / args → archive | 既存 target は維持。正負 smoke はより直接的に本番 OCI の image で実行 |
| INV-3 | Compose cn-migrate / cn-user-api / cn-iroh-relay / cn-indexer / cn-relation-analyze | 共通 Dockerfile、package 単位の args | 同じ4 binary に対応。cn-migrate / cn-relation-analyze は cn-cli。build 設定・製品ロジックは不変 |
| INV-4 | workflow PR / push / workflow_dispatch | bake → check/smoke → GHCR | script の失敗を伝播し、成功時のみ既存 publish 条件へ進む |

参照元は `docker/cn/Dockerfile` の repository 内逆引きと bake/Compose の TARGET_BIN / TARGET_PACKAGES 列挙で確認。4 group、適合 4 / 不適合 0 / 未分類 0。スクリプトと bake の変更も cn-images workflow の path filter に含めた。

| transition | 期待結果 | evidence |
| --- | --- | --- |
| TR-1: 修正前 → 修正後 image の構成検査 | 前者で失敗、後者で不要成果物なし | 上記 exit 1 → 全 layer + filesystem 検査 |
| TR-2: 4対象の COPY → 起動 | 各 app が実行可能、cli/indexer の decoder 依存を維持 | cli --help / prepare、user-api /healthz、relay /generate_204、indexer validate-config、ffmpeg/ffprobe/build-id |
| TR-3: 検証 → publish | 正例成功・負例拒否・credential 非漏出、失敗時 publish 不可 | 既存 indexer 全 case、check=True、workflow の step 成功条件 |

## サイズ比較

基準は `ac9f93ceef95d3fec8b89b33bf00e96d11d84c53` の `sha-ac9f93ceef95` tag。gzip layer の byte 合計を同じ方法で比較する。

| image | ac9f93ce | v0.2.6-preview.1 | 修正後 | binary layer |
| --- | ---: | ---: | ---: | ---: |
| user-api | 648220591 | 971539326 | 46583128 | 12607698 |
| iroh-relay | 239963153 | 963261203 | 39089488 | 5113995 |
| cli | 733553685 | 1142944617 | 212307721 | 7637707 |
| indexer | 748691783 | 1155519810 | 224698795 | 20029098 |

### 比較対象の linux/amd64 manifest digest

| image | ac9f93ce | v0.2.6-preview.1 |
| --- | --- | --- |
| user-api | `sha256:3a77f67b335f9386c5a7528b294b2dc753be52d27caca7e8606f33b1a0b254c3` | `sha256:9b4b8307573d376ea66f883afbd59fa3e98a35bfa55f61d75a159a3323c941ac` |
| iroh-relay | `sha256:3d4f9a3ef34b599ddd255881d8548abb38bcd988b83b7343b3673179b8903e73` | `sha256:3f8285352c4288f0de5ec51962f412fb34151e75ec8f229e77c12b108805d654` |
| cli | `sha256:a37720ac25af3f9481e6a2e6582bb11489c1c9beed0f2ebf6e64072fc2534083` | `sha256:7925952866902293fbc256ba0403351dd387d4dfecb06c3fea6977ba739af0aa` |
| indexer | `sha256:fd041420d477b61930ff2e563c3a9b055fa13fdd0fc449b07db88cf756b33b76` | `sha256:6817bb66a0ef782c7435eeda0e7a24d7de1bba7a8326887482fb98156c9cae20` |

### 検証した修正後 artifact

| image | linux/amd64 manifest digest | app SHA-256 |
| --- | --- | --- |
| user-api | `sha256:ef5eac351013d3142dfb1897c66bf9703fea65c05e7b5382ee0ead0b8b32d1b8` | `01b8c21d37225b29c8c73f6a94e4f59e8b0074898d320197084dc1fe6e2409af` |
| iroh-relay | `sha256:421697a07ff7591f68b7de2e469bd05fc4c0aaf5ec578829b31f082f793b2935` | `51641b7547028fe4b64a584d08c3644b0b91224547576885e1682d82dcdda8cb` |
| cli | `sha256:75d1678f2feb33c497bee846308ce6b94a5cf611482d7d9c1e294e96c843eae0` | `f542a7143c448d9781577e55ced41b37e820f874250621762ce4fecc4203abe1` |
| indexer | `sha256:9c3d0c2c23c47ae4ac32d57ef2be045d11544300e6c547f40051270039843acb` | `b7ea428937b9303caeda28406ae8593f01f797a45f837707c55a6e9684473347` |

## Validation

- 環境: Windows / Docker Desktop 4.76.0、Linux Engine 29.5.2、buildx v0.34.0、Python 3.13、Git Bash。
- 修正前 filesystem assertion: 想定どおり失敗（exit 1）。
- `docker buildx bake --file docker/cn/docker-bake.hcl --set '*.platform=linux/amd64' --allow 'fs.write=C:/Users/kgm11/cn-1138-out' --progress plain`: PASS（4 package の release build と5 target の export）。OUT_DIR は同じ workspace 外 directory。
- `python scripts/ci/cn_image_check.py C:/Users/kgm11/cn-1138-out`: 最終実行 PASS（exit 0）。初回は Docker image ID と OCI config digest の比較で停止。containerd image store の ID 表現に依存しないよう、起動設定・全 diff_ids・app SHA-256 を実内容で比較する方式へ修正し、同じ OCI で再実行した。次に Windows の `bash` が PATH 上の Git Bash より System32/WSL を優先し、別 Docker context（default）を参照する問題を観測。`shutil.which` で Git Bash を明示解決し、desktop-linux context の検証対象をそのまま渡すよう修正した。製品 image の変更・再build は不要だった。
- `actionlint .github/workflows/kukuri-cn-images.yml`: PASS。
- Python AST parse / `bash -n scripts/ci/cn_indexer_smoke.sh` / bake `--print`: PASS。
- indexer smoke 本文の旧版との機械比較: PASS。
- `git diff --check`: PASS。
- Rust / frontend ロジックは変更しないため cn-test / rust-test / desktop suite は非該当。image build と実際の entrypoint smoke を実行する。
- PR CI / merge tree 一致確認: 未実行。結果を確認するまで Complete / Close としない。

独立監査は区分 B の本修正では必須でない（親 Issue、Reopen、shared guard の変更ではない）。追加の製品要件・境界変更は行っていない。

## 完了条件との対応

- AC-1: 全4本番 OCI の全 layer と最終 filesystem に `/tmp/release`・他 CN binary・Rust 中間成果物なし。app の実行権限・SHA-256・ENTRYPOINT を確認。
- AC-2: 全4 image の合計 byte が旧基準以下。relay binary layer は 5,113,995 byte（約5.1MB、数十MBより小さい）。CLI の --help / prepare、user-api の /healthz、relay の /generate_204、indexer の既存9 case と credential 非漏出が全て PASS。
- INVAR-1: Dockerfile の builder と bake args は差分なし。bake 変更は現行 smoke/publish 経路の説明コメントだけ。
- 残りは PR CI の確認と merge commit の対象一致確認、Issue の現在判定更新・Close。GHCR への転送自体はローカル未検証で、既存 publish 処理は変更していない。
