# P2P Room Metaverse MVP

## 目的
この MVP は、kukuri が中央集権的なゲームサーバーを前提にせず、トピック単位の固定規格Domeを扱えることを検証するためのものです。VRChat クローン、MMO、音声、ワールドエディタ、任意map、大規模リアルタイムシミュレーションは対象外です。

固定geometryと変更可能範囲の正本は[ADR 0035](../adr/0035-fixed-dome-data-classification.md)、Spatial Context、Preset / Instance分離、引っ越しlifecycleの正本は[ADR 0036](../adr/0036-spatial-context-dome-instance-move.md)、Dome間Connectionとcomponent-local topologyの正本は[ADR 0037](../adr/0037-dome-connection-topology.md)とする。Metaverseは実験機能のため、旧`world_version = 1` / `2` roomとの後方互換やmigrationは提供せず、既存roomは再作成する。

## アーキテクチャ選択
`docs/progress/2026-05-27-metaverse-mvp-plan.md` の Option A として、既存の game room model を拡張しています。

理由:

- game room は docs の状態ポインタと blob manifest という永続化パターンをすでに持っている。
- `game_room_cache` により、トピック単位の room discovery と再起動後の復元を使える。
- 既存の list/create/update 経路、private channel scope、`SessionChanged` hint を再利用できる。
- `room_kind` の既定値を `score_game` にしているため、既存の score/status game room と互換性を保てる。

## 実装済み範囲
- `GameRoomManifestBlobV1.room_kind`: `score_game` または `metaverse_room`。
- `DomePresetManifestV1`: owner assetとして固定Dome customization、persistent prop初期定義、asset refsを保持。
- `DomeInstanceManifestV1`: topicまたはchannelのSpatial Context上でowner、Preset ref、generation、status、session境界を保持。
- `GameRoomManifestBlobV1.metaverse`: PresetとInstanceを解決してdesktopへ返すread model。
- `GameRoomView.manifest_blob_hash`: UI から確認できる永続化/debug 用のシグナル。
- `create_metaverse_room` / `update_metaverse_room` Tauri command。
- Desktop の game section で通常の score room と metaverse room を分離表示。
- Metaverse discovery panel で、現在のtopic / channel Contextにあるactive Dome一覧、owner slotが空の場合の作成、owner Domeの別Contextへの移動を提供。
- Room viewでthree.js scene、camera、固定半球、4方向endpoint、connection zone、local avatar、remote avatar、persistent propを描画。
- `apps/desktop/public/blumochichi.vrm` を local avatar としてロードし、失敗時は primitive fallback を使用。
- 任意の VRM file または sample VRM を blob storage に import し、`MetaverseAssetRef` として扱う。
- avatar presence は `avatar_asset_ref` を signed room event に載せ、raw bytes を room event に埋め込まない。
- avatar transform、chat、object update は署名済み `metaverse-room-event` envelope として hint transport で送受信する。
- WASD / arrow key movement により avatar transform event を約 10 Hz で送出。
- `update_metaverse_room`はownerだけがcustomizationとpersistent prop初期定義を更新できる。実行中interactionはmanifestを更新しない。
- 同一Spatial Contextのowner間で、4方向slotごとのConnection proposal、receiver accept、proposer withdraw、双方ownerからのrevokeを行える。
- proposal / selection / agreement / lifecycle envelopeをContext docsへ保存し、active Connection集合からcomponent root、5,700cm単位の相対座標、topology digestを決定論的に導出する。
- DesktopのConnection panelで4方向slot、待機proposal、active / draining状態、導出topologyを確認する。隣接sceneの描画と境界通過はIssue #790の範囲であり、まだ有効化しない。

## Production MVP 完了条件
この goal は、以下を満たす状態を production MVP として扱う。

- metaverse room discovery は既存 game room list/projectionをread modelとして使い、ownerごとにContext内最大1つのactive Domeだけを表示する。staging / tombstoned Instanceは表示しない。
- room metadata、owner customization、persistent prop初期定義はdocs pointer + manifest blobで永続化され、restart後に復元できる。実行中transformはTransient eventでありmanifestへ保存しない。
- avatar transform、chat、object update は署名済み `metaverse-room-event` envelope として P2P hint transport で送受信できる。
- avatar transform は high-frequency な ephemeral event として扱い、docs/blobs に 10 Hz で直接書き込まない。
- VRM asset bytes は blob storage に入り、presence / room event には `MetaverseAssetRef` だけを載せる。
- `cargo xtask check` と `cargo xtask test` が最終状態で完走する。

## Kukuri Primitive との対応
- Hints: `SessionChanged` は room metadata の通知と同期トリガー。`MetaverseRoomEvent` は署名済み room event envelope の軽量 transport。
- Docs: author replicaにPreset pointerとmove record、topic/private-channel replicaにowner slotとInstance pointerを保存。
- Blobs: manifest JSON、VRM/GLB などの asset bytes を保存。
- SQLite projection: `game_room_cache` に room discovery fields、room kind、manifest hash、metaverse JSON を保存。
- Dome Connection projection: proposal / selection / agreement / lifecycleの正本はContext docs。`dome_connection_projection_cache`は削除・再構築可能なlocal read modelであり、component entityやglobal座標を正本として保存しない。
- Connectivity/community node: optional facilitator として扱い、canonical authority にはしない。
- Avatar transforms: high-frequency な ephemeral event として扱い、docs/blobs に直接 10 Hz で書き込まない。

## 起動
```powershell
cargo xtask check
cd apps/desktop
npx pnpm@10.16.1 dev
```

desktop app を開き、`Game` に切り替えて `Metaverse Rooms` panel を使う。

## 1 クライアント確認
1. `Game` を開く。
2. metaverse room を作成する。
3. room card を開く。
4. 3D viewport が表示され、local avatar に `blumochichi.vrm` が読み込まれることを確認する。
5. `Sample VRM` または `VRM file` で avatar asset を blob に import し、`Avatar asset` が `blob VRM loaded` になることを確認する。
6. WASD または arrow key で local avatar を移動する。
7. room chat message を送信する。
8. object controls で共有オブジェクトを移動する。
9. owner customizationを保存し、refreshまたは再起動後にmaterial、environment、persistent prop初期定義がmanifestから復元されることを確認する。
10. 同じContextに別ownerのDomeがある場合、空いている方向slotからproposalを送り、receiver側でacceptする。双方の画面にactive Connectionと同じtopology digestが表示されることを確認する。
11. どちらかのownerからConnectionをrevokeし、残ったedgeだけでcomponentが再構築されることを確認する。

## 2 クライアント確認
1. 通常の kukuri peer connectivity で desktop instance を 2 つ起動する。
2. 既存 game-room smoke flow と同じ topic と peer connectivity path を使う。
3. client A で metaverse room を作成する。
4. client B が room を discover できることを確認する。
5. 両 client で room を開く。
6. client A の avatar movement が client B の remote avatar に反映されることを確認する。
7. chat message が相互に送受信されることを確認する。
8. persistent propの実行中interactionがroom eventで反映され、ownerによる初期定義の保存だけがmanifestを更新することを確認する。

自動検証として、`metaverse_room_events_replicate_between_iroh_peers` は 2 つの Iroh peer 間で signed room event が配送されることを確認する。

## VRM / Asset Flow 確認
- local avatar は public sample または blob-backed VRM URL を `GLTFLoader` と `VRMLoaderPlugin` で読み込む。
- 読み込み成功時、debug panel の `Avatar asset` は `sample VRM loaded` または `blob VRM loaded` になる。
- 読み込み失敗時、debug panel は fallback 状態を表示し、primitive avatar を維持する。
- asset bytes は blob storage に入り、room event / presence には `MetaverseAssetRef` だけを載せる。

## Persistent prop同期確認
persistent propの初期定義はownerだけが`update_metaverse_room`でmanifestへ保存する。`grab` / `throw` / `push` / `sit`や実行中transformは`object_update` / avatar transform eventとして送信し、interactionだけではDurable manifestを更新しない。authoritative physicsとlayout commitはIssue #788/#793の範囲とする。

自動検証として、`metaverse_room_manifest_restores_after_restart_from_docs_and_blobs`と`desktop_smoke_metaverse_dome_persist`はrestart後にroom list、customization、persistent prop初期定義がdocs + blobから復元されることを確認する。

`desktop_smoke_metaverse_dome_connections`は3 ownerでA–B / B–Cを接続し、cycleとなるA–Cを拒否し、restart後の同一topologyとrevoke後のcomponent分割を確認する。

## 既知の制約
- 大規模同時接続、voice、WebRTC/SFU、ワールドエディタ、UGC scripting、asset safety scanning は goal の non-goal。voice のうち空間音声は後に ADR 0043（#795）で追加され、その搬送は [ADR 0067](../adr/0067-media-transport-moq.md) が正本となる。
- Connection proposalは1 Domeあたりopen outbound 32件、同一peer slot 4件、receiver slot queue 32件、local createは10分間に8件を上限とする。
- owner blockをConnection失効へ結線する入力元はIssue #795、隣接Dome sceneのprefetchとtransitionはIssue #790で実装する。
- chat は room-scoped signed event として扱う。topic timeline post として永続表示しないため、長期履歴 UI はこの MVP の対象外。

## Fallback / Mock と MVP 対象外
- desktop/Tauri 実行時の canonical path は `metaverse-room-event` envelope を使う signed P2P hint transport。
- browser-only dev shell では Tauri backend がないため、同一ブラウザ内確認用に `BroadcastChannel` fallback を残している。
- VRM が読み込めない場合でも room view を壊さず、primitive avatar を表示する。
- remote peer が avatar asset blob を解決できない場合も fallback avatar で表示を継続する。
- 長期 chat history、moderation UI、asset safety scanning、asset pinning/backfill の完全自動化はこの MVP の未実装範囲。

## 次の実装候補
- room event buffer を既存 message/history primitive と統合し、chat の長期履歴 UI を追加する。
- VRM/GLB asset の pinning/backfill 状態を debug panel から操作・確認できるようにする。
- remote avatar の補間と stale/leave 表示を polish する。
- 2 desktop instance の手動確認を e2e harness 化し、3D viewport のスクリーンショット検証も自動化する。
