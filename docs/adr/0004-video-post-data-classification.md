# ADR 0004: Video Post Data Classification

## Status
Accepted

## Feature Data Classification
- Feature 名: video post
- Durable / Transient: Durable
- Canonical Source: `iroh-docs` for post header / asset refs, `iroh-blobs` for video payload and poster payload
- Replicated?: Yes
- Rebuildable From: `docs + blobs`
- Public Replica / Private Replica / Local Only: Public replica for `VideoManifest` / `VideoPoster` asset refs, local projection for blob status and preview cache
- Gossip Hint 必要有無: Yes, `TopicIndexUpdated` only
- Blob 必要有無: Yes
- SQLite projection 必要有無: Yes
- 必須 contract:
  - `video_post_visible_before_full_blob_download`
  - `remote_video_manifest_payload_available_after_sync`
  - `late_joiner_backfills_video_media_payload`
  - `restart_restores_video_media_payload`
- 必須 scenario:
  - single attach composer 導入後に `video post -> poster skeleton -> poster preview -> playable video` を回帰に追加する

## Decision
- 動画投稿は `VideoManifest` と `VideoPoster` の 2 種類の asset ref を header に載せる。
- 最小縦スライスでは `VideoManifest` は動画本体 blob を指す attachment として扱い、再生制御や adaptive manifest は後続フェーズに送る。
- `VideoPoster` は card renderer の preview source として扱い、blob 未取得時は poster skeleton を表示する。
- frontend の media 表示は `data URL` ではなく `Blob + object URL` を canonical にし、image/video とも同じ object URL cache を使う。
- composer の添付 UI は単一 `Attach` に統合し、video 選択時は browser 内で `VideoPoster` を自動生成する。
- `VideoPoster` の client-side 生成に失敗した video は publish を許可しない。
- 画像・動画の添付容量に製品上の上限を設けない（#1197、2026-10-10 ユーザー判断で現行の上限なしを維持）。
- poster 生成は `preload=auto` で読み込んだ frame を元の解像度・JPEG quality 0.85 で保存する。WebView の idle JPEG callback を待たず、`toDataURL` の一時的な結果を直ちに File の bytes へ戻す。表示用の canonical は引き続き object URL。生成成功・失敗・既存の5秒timeoutのいずれでも video と生成元URLを回収する。
- `gossip` は video bytes や poster bytes を運ばず、`TopicIndexUpdated` hint のみを publish する。
- `SQLite` は video post の timeline/thread projection、poster status、local preview cache に限定する。

## Consequences
- late joiner と restart 後の復元は `docs header + blobs fetch` だけで成立しなければならない。
- poster が未取得でも video post row 自体は timeline/thread に出なければならない。
- poster だけ先に取得できた場合は poster preview を出し、manifest payload 取得後は playable video へ昇格する。
- client が manifest payload を decode できない場合は poster-only のまま維持し、`unsupported on this client` として扱う。
- poster 生成 failure は publish blocker として扱う必要がある。
