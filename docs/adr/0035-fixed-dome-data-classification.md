# ADR 0035: 固定規格Domeのデータ分類

## Status

Accepted

## Context

Metaverse roomを任意mapやWorld Editorへ拡張せず、全ユーザー共通の固定空間「Dome」を基礎単位にする。Domeの形状、接続口、基本collision、physics契約はクライアント実装が所有し、ownerが署名付きmanifestで変更できる範囲をsurface、environment、gravity強度、persistent propの初期定義に限定する。

Metaverseは実験機能であるため、既存の`world_version = 1` / `2`形式とのdecode、migration、表示互換は提供しない。既存roomは再作成を前提とする。Spatial Context、owner-owned Preset、Context-owned Instance、引っ越しの正本は[ADR 0036](0036-spatial-context-dome-instance-move.md)で定義する。

## Feature Data Classification

- Feature 名: fixed Dome room
- Durable / Transient:
  - Durable: owner customization、persistent prop初期定義、room metadata、current manifest pointer
  - Transient: avatar transform、interaction入力、実行中prop transform、seat state、physics simulation state
- Canonical Source: owner author replica上のPreset current pointer、対象Context replica上のInstance owner slot、それぞれが指すowner署名manifest blob
- Replicated?: Yes
- Rebuildable From: `docs + blobs`
- Public Replica / Private Replica / Local Only: Presetはauthor replica、Instanceはroomのchannel scopeに従うreplica、SQLiteはlocal projection
- Gossip Hint 必要有無: `SessionChanged`は同期開始のhintとして使用し、canonical sourceにはしない
- Blob 必要有無: Yes。manifest、surface texture、VRM / GLB prop assetを保存する
- SQLite projection 必要有無: Yes。`game_room_cache`はdocsとblobから再構築可能とする
- 必須 contract:
  - 現行Dome wire round-trip
  - fixed geometry derivation
  - 規格外geometry、未知spec、任意script、任意world mesh、physics無効化の拒否
  - owner-only customization update
  - docs＋blob restart restore
- 必須 scenario: `desktop_smoke_metaverse_dome_persist`

## Fixed Geometry Contract

- 座標系はY-up、floor中心を原点とする。
- Domeは内半径20m、内径40m、floorから頂点まで20mの真半球とする。
- 壁厚は外向き2mで、外半径は22m、壁厚中央面は中心から21mとする。
- North / East / South / Westの4方向に同一形状のendpointを置く。
- 開口部は幅5m、全高10mとし、下部は高さ7.5mの矩形、上部は半径2.5mの半円とする。
- connection zoneは開口部と同じ断面で奥行き15mとする。両端を各Domeの壁厚中央面へ合わせるため、各shellへ1mずつ重なる。
- connection zoneの長手方向中央を境界面およびtransition中心線とする。
- 隣接Domeの中心間距離は`21m + 15m + 21m = 57m`とする。
- gravity方向は`-Y`、physicsは常時有効とし、manifestやIPCから変更できない。

## Customization Contract

- ownerが変更できるのは内壁・floorのmaterial presetまたはtexture asset ref、key light、ambient light、fog、gravity強度、persistent prop初期定義だけとする。
- 数値environmentは整数の固定単位で保存し、決定論的な線形補間が可能な表現にする。
- persistent propが表現できるinteractionは`grab`、`throw`、`push`、`sit`だけとする。visual-only propはinteractionを持たない。
- avatar / prop colliderはasset loaderから得た明示colliderを優先する。明示colliderが無い場合は正規化後bounding box全体を包含するY軸capsuleを決定論的に生成する。
- 任意world mesh、任意script、Dome寸法、gravity方向、physics enabledはmanifest fieldとして持たない。

## Authority

- owner署名identityだけがDurable customizationを持つPresetとInstance refを更新できる。peer idや一時session idは認可に使用しない。
- interaction入力はmanifestを更新しない。authoritative physicsとpeer同期はIssue #788で定義する。
- Connection recordとtopologyはIssue #792、guest propとlayout commitはIssue #793の責務とし、本ADRのmanifestへ先取りしない。

## Consequences

- fixed geometryは`fixed_dome_v1` resolverから再構築し、manifestへmeshや寸法を複製しない。
- 既存の実験用Metaverse roomは読み込み対象外となり、再作成が必要になる。
- room一覧projectionは既存のgame room read modelを継続利用するが、Preset / Instance authorityとowner slot一意性はADR 0036の専用stateで判定する。
- #1909（2026-10-22）: 公開topicのDomeのInstance・Preset・hostingの記録は、ownerの署名を検証した同じtopicの参加者（公開topicを保持する
  端末など）も保持して返す。読む側は、owner本人の宛先の次にtopicの参加者を読み先に入れ、manifestを返したpeerを資材の取得候補に入れる
  （ADR 0054 §4）。ownerがオフラインでも、Domeの一覧の行と資材を読める。privateのcontextのDomeは保持しない。
