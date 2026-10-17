# ADR 0031: 可変 span の Column Canvas と下部 Control Center

## Status
Accepted

## Date
2026-08-22

## Base Branch
`main`

## Related
- `docs/adr/0014-uiux-dev-flow.md`
- `docs/adr/0018-channel-first-sidebar-and-unified-epoch-lifecycle.md`
- `docs/adr/0020-pairwise-dm-v1.md`
- `docs/adr/0023-local-notification-inbox-v1.md`
- `docs/adr/0025-community-node-indexing-foundation.md`
- `DESIGN.md`

## Context

現行 desktop shell は、常設 left rail、単一 primary workspace、必要時に右へ積む thread / author detail pane を基本構造とする。中央 workspace も primary section、Timeline / Bookmark、Community Index、Timeline を同程度の強さで並べるため、初期表示が狭く複雑に見える。

state も単一の active topic / channel と active primary section を全画面で共有し、Timeline / Notifications / Profile と Thread / Author を別階層に扱う。複数の topic / channel / thread / profile を並べて扱う場合、この区分は利用者の作業単位と一致しない。

一方、ADR 0018 が定めた「channel は topic 配下の scope」「Join / Share の統一」「epoch lifecycle と auto distribution / auto apply」は domain と transport の正本として維持する必要がある。置き換えるのは desktop shell 上の配置と、全画面で単一 scope を共有する UI contract である。

## Decision

### 1. Column Canvas を画面本体にする

アプリ本体には閲覧・対話対象の Column を横方向に並べる。Column kind は少なくとも次を扱う。

- `timeline`
- `notifications`
- `thread`
- `profile`
- `explore`
- `messages`
- `conversation`
- `stream`
- `game`
- `metaverse`

Thread、Profile、Conversation を Timeline より視覚的に下位の pane として扱わない。ただし `parentColumnId` により、どの Column から開いたかという意味上の親子関係を保持する。

### 2. 初期体験は主要5 Columnを提示する

保存済み layout がない利用者には、主要機能とプロフィール設定の入口を最初から理解できるよう、次のpin済み1 span Columnを同じstarter topicのpublic scopeで表示する。

1. Timeline
2. 自分のProfile（`entityId`なし）
3. Explore
4. Notifications
5. Messages

初期active ColumnはTimelineとし、topic-firstの閲覧・投稿導線とcanonical routeを維持する。Profileはoverview内のEditから設定できる。Stream / Metaverseはdeveloper向けまたは対象session / roomを選んで開くsurface、Thread / Conversationは投稿・相手から開く文脈Column、private channelはTimelineのscopeであるため、初期配置へ含めない。

この既定値を使うのは、`kukuri:workspace-layout:v1`に妥当な保存データがないfresh installまたは安全側fallback時だけとする。既存の自動保存layoutと名前付きlayoutは補完・移行・上書きせず、そのColumn構成とactive targetを優先する。

常設 Sidebar、global workspace tab header、Timeline 上の常設 Community Index、detail pane の予約領域は表示しない。画面下部のControl Center triggerと各Columnのprimary actionを維持し、追加Column、pin変更、並べ替え、span変更、layout保存は利用者が明示操作する。

### 3. 各 Column が独立 scope を持つ

Timeline、Stream、Game、Metaverse 等は、それぞれ optional な topic / channel scope を持てる。全画面で共有する active topic / selected channel を Column scope の正本にしない。

header には topic、public / channel、対象 entity を必要な範囲で表示する。同一画面に異なる topic / channel の Column を並べても、表示と primary action の対象が混ざらないようにする。

channel が topic 配下の scope であるという ADR 0018 の domain contract は維持する。channel を独立 domain workspace へ戻すものではない。

### 4. Column header と下部 action の責務を固定する

Column header は title、scope、active / pinned / transient 状態、drag grip、Column menu を持つ。close、pin、span、左へ移動、右へ移動は Column menu から到達可能にする。

Column 下部には、その Column に対する primary action を置く。Timeline の投稿、Thread の返信、Conversation のメッセージ、Stream / Metaverse の参加・退出等を global compose action へ混ぜない。

drag grip は focus 可能にし、Column 本体、投稿、video、3D viewport、text selection を drag handle にしない。並べ替えは drag だけを唯一の手段にせず、Column menu に keyboard 操作を用意する。

### 5. Draft は Column / action / scope 単位で分離する

Composer は Column 下部から上へ展開し、投稿先 topic / channel / thread / peer を明示する。Draft は少なくとも Column、action、scope、thread / peer 単位で分離する。

scope 変更時に未送信 Draft を別 scope へ黙って移さない。元 scope で保持するか、明示確認を要求する。

### 6. Control Center を画面下部から開く

常設 Sidebar、global navigation、Community Node 管理、設定入口を Control Center に整理する。desktop の既定 trigger は左下とし、次の空間的役割を持たせる。

- 左側 / 左下: アプリ全体の移動・制御
- 各 Column 下部: その Column に対する操作

Control Center は画面下から開く bottom drawer とし、通常の移動では background interaction を完全に遮断する強い modal にしない。設定編集、認証、同意、破壊的確認だけを modal Sheet / Dialog へ遷移させる。

Control Center は次を扱う。

- Column: 追加、一覧、focus、pin、close、保存済み layout
- 場所: topic / channel 検索、topic 追加、channel 作成、Join / Share
- アクティビティ: Notifications、Messages、unread count
- システム: 接続、update、Community Node、Settings、About / Legal、developer diagnostics

### 7. Community Node の利用目的と運用情報を分離する

検索、発見、推薦、trust / relation signal、moderation 由来表示は Explore / Search Column または対象 Column の結果として扱う。node URL、auth / consent、manifest、capability、authority scope、health、token 管理は Control Center または Settings に置く。

健康な状態では node selector を primary UI に常設しない。障害時は Control Center trigger の状態表示と、影響を受ける Column の inline Notice の両方で伝える。Community Node を identity、profile、social graph、投稿本文の network-wide truth source として見せない。

### 8. Desktop は可変 span の横方向 workspace にする

通常 Column 1本を1 span とし、表示幅は次で求める。

```text
width = span * columnUnit + (span - 1) * gap
```

kind ごとの desktop 既定値は次とする。具体的な unit、gap、breakpoint は `DESIGN.md` を正本とする。

| Column kind | Desktop default | Mobile |
|---|---:|---:|
| Timeline / Notifications / Profile / Thread / Public threads | 1 span | 1 viewport |
| Messages / Conversation | 1〜2 span | 1 viewport |
| Stream | 2 span | 1 viewport |
| Metaverse | 3 span | 1 viewport |
| Metaverse focused | 最大4 span | 1 viewport |

複数 span Column は複数の独立 Column ではなく、並べ替え時にも分割しない atomic surface とする。Window が狭い場合は actual width を縮めてよいが、利用者が保存した preferred desktop span は失わない。内部 layout は container query 等で切り替える。

### 9. Stream と Metaverse は wide surface とする

Stream と Metaverse は別画面への強制遷移ではなく、Column Canvas 内の wide surface とする。

Stream は desktop 既定2 span とする。2 span 以上では映像と chat / reactions / session information を並列配置し、1 span では補助情報を映像下または overlay へ移す。

Metaverse は desktop 既定3 span、最大4 span とする。viewport を主役にし、HUD、chat、participant、interaction control は幅に応じて side pane または overlay へ切り替える。

Fullscreen は一時的な表示状態とし、退出後に元の Column 順、span、session state へ戻る。既存の LiveSession domain / API、game room / `room_kind` contract の rename は行わない。

### 10. Mobile は 1 Column = 1 viewport とする

mobile では desktop と同じ Column 順を横方向の page として扱い、全 Column を1 viewport 幅へ正規化する。

- horizontal scroll snap を使う
- swipe 完了時に active Column を更新する
- child Column は parent の右へ追加して移動する
- system back で parent Column へ戻る
- Control Center から任意 Column へ直接移動できる
- 現在位置と総数を表示する
- safe area と Column 下部 action / Composer を両立する

Stream の seek、media viewer、Metaverse の camera / virtual stick / object drag 内では Column swipe を奪わない。interactive surface 内は scene / media 操作を優先し、header、edge swipe、Column indicator、Control Center を切替導線にする。

### 11. active / visible / audio focus / lifecycle を分離する

可変幅 Canvas では複数 Column が同時に visible になり得るため、次を別 state として扱う。

- `visible`: viewport 内に見えている
- `active`: 最後に明示操作した Column
- `audioFocused`: 音声を出す Stream / Metaverse
- `suspended`: render / media resource を縮退している

Column 内の明示操作は、本文・header のボタンや選択 control を含めてその Column を active にし、URL をその Column の canonical target へ同期する。Metaverse の camera 等の gesture 領域は例外とする。pointer 押下で active になった Column の scroll は押下の終了後に行い、押した操作を別の要素へずらさない。

Control Center と keyboard shortcut は active Column を対象とする。WASD、camera、gamepad 等は Metaverse Column を明示 focus した後だけ捕捉し、入力中、Composer 操作中、Dialog 表示中には奪わない。

画面外 Stream は video 停止、低品質化、または明示的 background audio へ縮退し、画面外 Metaverse は render 停止または低 FPS 化する。network session と render lifecycle を分離し、既定の audio focus は1つに限定する。

### 12. URL と local layout state を分離する

共有・deep link 用 URL は focus 中 Column の canonical target だけを表現する。Column 配列全体、幅、順序、scroll 位置は URL に入れない。

local persistence は Column kind、scope、target entity、order、preferred span、pinned state、active Column、scroll restoration key、Draft、保存済み layout を扱える。

deep link は、一致する Column があれば focus し、なければ transient Column を作る。mobile ではその Column へ移動する。無効な target は既存の安全側 normalize を維持する。

### 13. product state と runtime state を分離する

型名は実装時に調整できるが、責務は次の形に分離する。

```ts
type ColumnSpan = 1 | 2 | 3 | 4;

type WorkspaceState = {
  columns: ColumnState[];
  activeColumnId: string;
  controlCenterOpen: boolean;
  activeLayoutId: string | null;
};

type ColumnScope = {
  topicId: string;
  channelId: string | null;
};

type ColumnState = {
  id: string;
  kind: 'timeline' | 'notifications' | 'thread' | 'profile' | 'explore'
    | 'messages' | 'conversation' | 'stream' | 'game' | 'metaverse'
    // topic の公開スレッドの一覧（ADR 0068 §7）。詳細は scope の channel が公開スレッドの timeline。
    | 'publicThreads';
  scope?: ColumnScope;
  entityId?: string;
  parentColumnId?: string;
  pinned: boolean;
  preferredDesktopSpan: ColumnSpan;
};

type ColumnRuntimeState = {
  visible: boolean;
  active: boolean;
  audioFocused: boolean;
  suspended: boolean;
};
```

IntersectionObserver や focus から導出する runtime state を、persist する product state へ無条件に混ぜない。

### 14. accessibility と motion を仕様に含める

- Column、drag grip、primary action、Control Center、Column menu は keyboard で到達可能にする
- active、focused、pinned、transient、dragging、drop target を色だけで区別しない
- screen reader に Column title、position、span、active state を伝える
- mobile swipe 以外の Column 切替手段を持つ
- Control Center、Column 移動、edge auto-scroll は shared motion token を使う
- reduced motion では移動量と auto-scroll animation を抑制する
- touch target は最小44pxを維持する

### 15. 名前付き layout は local snapshot として扱う

名前付き layout は、最後の作業状態を自動復元する `kukuri:workspace-layout:v1` とは別の version 付き local snapshot とする。snapshot は Column の kind、scope、entity、親子関係、順序、pin、preferred span、Timeline view、active Column だけを持ち、Draft、Control Center、Fullscreen、audio focus、session runtime を含めない。

保存、更新、呼び出し、rename、delete は Control Center の Column section から行う。呼び出しは active Column の canonical URL だけを同期する。active layout に未保存差分がある状態で別 layout を呼び出す場合は破棄確認を要求し、active layout を削除しても現在の workspace 自体は維持する。

### 16. Community Index の node 固定は advanced preference とする

Community Index の問い合わせ先は `自動`を既定とし、健康な状態では primary Explore surface に selector を置かない。Settings の Community Node 詳細設定で、構成済み node を明示選択できる。

明示 node が一時的に認証・同意・health・manifest・capability の適格条件を失った場合、preference は保持するが query target を `null` にし、別 node へ黙って切り替えない。影響を受ける Column と設定面に理由と Settings 導線を表示する。明示 node が構成から削除された場合だけ `自動`へ戻す。

### 17. immersive lifecycle と mobile gesture の境界を固定する

Stream / Metaverse Column の fullscreen は Fullscreen API の `fullscreenchange` を正本にする一時的 runtime state とする。入退出で Column 順、span、scope、active layout、Draft、LiveSession / Metaverse room stateを変更せず、退出後は元 Column へ focus を戻す。保存 layout と URL へ fullscreen 状態を含めない。

Fullscreen要素は通常のColumn Canvasをrootにした交差判定の外に出るため、可視集合の公開時に実際の`document.fullscreenElement`を合成する。全画面の所有Column（内側のmediaが全画面の場合も含む）をvisibleとし、覆われたColumnは非表示として扱う。遅延したIntersectionObserver通知で所有Columnをsuspendせず、退出後は通常交差判定へ戻す。audio focusの取得やnetwork sessionの再開始を伴わない。Metaverseの高さと補助面配置は`DESIGN.md`の全画面契約に従う。

mobile の明示的な追加導線は viewport 左右端と Column indicator から始まる主方向 horizontal swipe とする。1回の gesture で隣接1 Column だけへ移動し、先頭・末尾では wrap しない。短い drag、縦優位、cancel、indicator tap は切替にしない。Metaverse scene、media control、Composer 等の中央操作は各 surface が所有し、edge または indicator 以外から Column gesture を開始しない。

現行 Stream は映像 source、transport、seek、background playback の product contract を持たないため、架空の player や設定は追加しない。player 導入時に、seek と Column swipe の非競合、および background playback を明示 opt-in とする試験を追加する。Metaverse の参加・退出・chat は viewport 内 HUD / discovery card を正本とし、Column footerへ複製しない。

## ADR 0018 との優先関係

次の ADR 0018 の判断は維持する。

- channel は topic 配下の scope である
- Join / Share を利用者向けに統一する
- epoch lifecycle と auto distribution / auto apply
- private channel の domain / transport contract

次の desktop shell contract は本 ADR が置き換える。

- channel switch を常設 left rail に置く
- 全画面で単一 active topic / channel を共有する
- Timeline / Live / Game / Profile を単一 primary workspace の切替として扱う
- Thread / Author を専用 detail pane stack として扱う

## Migration

既存 presentational component、topic / scope key の cache、API DTO、Tauri command、Community Node state、route normalize、Metaverse scene model は再利用する。旧 shell と新 shell を長期間二重運用せず、Column adapter から既存 component を呼ぶ。

移行は review prototype、Column Canvas foundation、既存 surface の Column 化、scope / Composer、Control Center、可変 span / reorder、mobile / immersive lifecycle の順に進める。protocol や domain 変更を UI 移行と同じ変更へ混ぜない。

## Consequences

- 初期表示から主要5 Columnを横に並べ、利用者がProfile設定と主要機能の存在を確認したうえで、必要に応じて別scopeやwide surfaceを追加できる。
- 常設 Sidebar と global section switch が primary shell の正本ではなくなる。
- URL は共有 target、layout は local state という責務が明確になる。
- active と visible を分離するため、focus、audio、media / render lifecycle の明示管理が必要になる。
- Stream / Metaverse の interactive gesture と mobile Column paging の競合を test で固定する必要がある。
- ADR 0014 の Storybook、PR preview、Shneiderman checklist、keyboard、resize、reduced-motion の review flow は引き続き適用する。
- immersive lifecycle の確定（2026-08-24 追記、Issue #766）: Stream / Metaverse Column の一時 fullscreen、mobile の edge / indicator swipe、対象 control の44px最小領域を実装した。Stream player と seek 非競合試験は media contract 導入時へ留保し、background playback は将来も明示 opt-in とする。Metaverse の参加 / 退出 / chat は viewport 内 HUD / discovery card を正本として維持し、Column footerへ複製しない。
- 対象外 / 見送りの明示（2026-08-24 追記、Issue #765）: (a) §12 の「必要な scroll restoration key」は現時点で対象外とする。Column は表示時に最新内容を提示する方針で、session 内の縦 scroll は React の DOM 保持（Column id を key にした mount 維持）で足りるため、persistence への scroll 位置保存は行わない。必要になった場合は schema version を上げて追加する。 (b) bookmarks 一覧の Column scope フィルタは見送る。bookmarks は topic / channel を横断する利用者個人の集約であり、Column scope（表示・投稿先の正本）とは責務が異なるため、Timeline Column の Bookmarks view は global 一覧のまま維持する。scope 別に絞る必要が生じた場合は view 側の filter として再検討する。

## Non-goals

- Community Node の HTTP API 変更
- P2P protocol 変更
- channel epoch lifecycle 変更
- Stream / Metaverse の network protocol 再設計
- game room / LiveSession の serialized contract rename
- Community Node を network-wide truth source にすること
- 初回起動時から多数 Column を自動配置すること
- layout 全体を共有 URL に埋め込むこと
- drag を mobile の必須操作にすること
