---
name: session-cleanup
description: このセッションで使ったローカルのビルドキャッシュ・temp/tmp・Docker のボリューム/イメージ/ビルドキャッシュと、使用済みのブランチを削除する。「後片付け」「セッションの片付け」「cleanup」を頼まれたときに使う。
---

# セッションの後片付け

このセッションで使用したものだけを対象に、次を削除する。該当が無い項目は飛ばす。

1. ローカルのビルドキャッシュ（`target/`、`node_modules/` 配下の cache、`apps/*/dist` など、このセッションの build・test で作られたもの）
2. このセッションで作った temp / tmp のファイル・ディレクトリ（scratchpad、`tmp/`、`*.tmp`、`*.temp` を含む）
3. このセッションで作成・起動した Docker のコンテナ・ボリューム・イメージ・ビルドキャッシュ（`docker compose ... down -v`、`docker image rm`、`docker builder prune` など）
4. 使用済みのブランチ（PR が merge・close 済みのもの、または作業を終えたもの。local と、merge・close 済みなら remote も）

## 守ること

- 対象はこのセッションで使ったものに限る。他のセッション・worktree・利用者が使っている cache、Docker 資源、ブランチには触れない。`docker system prune -a` のような全体削除はしない。
- 削除前に対象を一覧し、作業中の変更が無いことを確認する。未 commit の変更、未 push の commit、open な PR が残るブランチは削除せず、理由を報告する。
- checkout 中のブランチは、default branch へ切り替えてから削除する。
- 最後に、削除したもの・残したもの（理由つき）・解放した容量を短く報告する。
