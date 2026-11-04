# Game SDK の導入の手順

Unity のゲームへ kukuri Game SDK を入れる手順。配布物だけで進められ、Rust は要らない。契約の正本は [ADR 0070](../adr/0070-unity-game-sdk.md)。配布物の作り方と確かめ方は [Game SDK の配布候補](game-sdk-distribution.md) にある。

## 対応環境

| 対象 | 版 |
| --- | --- |
| Unity | 6000.3.27f2（Unity 6.3 LTS）。Editor は Mono、Player は IL2CPP と Mono。他の版は確かめていないので対応に含めない |
| OS・graphics | Windows 11 x64、Direct3D 12。Windows 10 と他の OS・graphics は確かめていないので対応に含めない |
| 任意の package | URP 17.3.0。Netcode for GameObjects 2.13.3（NGO の Transport を使うとき）。glTFast 6.20.0（スキンを使うとき） |

Unity の project は短い path に置く。深い path は Windows の path の長さの上限に当たる。

## 導入

1. 配布物の `com.kukuri.gamesdk-<版>.tgz` を、ゲームの project の dir の隣に置く。`SHA256SUMS` と照合する。
2. `Packages/manifest.json` の `dependencies` に 1 行足す。path は `Packages` からの相対か絶対で書く。次の例は相対で、project の dir の隣を指す。

   ```json
   "com.kukuri.gamesdk": "file:../../com.kukuri.gamesdk-0.1.0.tgz"
   ```

3. Unity で開くと package が読み込まれる。DLL は Player の build に自動で入り、Sidecar は Editor の開発用として package に入っている。

## ゲームを始める

利用者は別のサイトで登録しない。メールやパスワードも、SNS 用のプロフィールも要らない。

1. Project の Create の `kukuri/Settings` で設定 asset を作る。既定のまま（mode は FFI、profile は `default`）でよい。
2. scene に空の GameObject を置き、`KukuriRunner` と `KukuriAccountScreen` を足す。`KukuriRunner` の `settings` に 1 の asset を、`KukuriAccountScreen` の `runner` に `KukuriRunner` を、`applicationId` にゲームの ID（例: `com.example.game`）を設定する。
3. 実行すると、同意の画面 →「はじめる」→ プレイの順に進む。同意でアカウントが、「はじめる」で game player が自動で作られ、次の起動からは同じものでそのまま再開する。
4. ゲームのセーブは `KukuriAccountScreen.GamePlayerId` で参照する。鍵やアカウントはゲームのコードから扱わない。

profile は `Application.persistentDataPath/kukuri/profiles/<profile>` に置かれる。アカウントの鍵は既定では OS の keyring（Windows の資格情報マネージャー）に置き、keyring を使えないときは profile の file に置く。「Steam Cloud に保存」を有効にしている間は、鍵の写しが profile の `game-account.json` に入る。profile をゲームのセーブと一緒に公開しない。

## 2 つの端末をつなぐ

同じ topic を開いた端末どうしが、次の順でつながる。

1. Direct P2P: 直接の接続。
2. Relay Supported P2P: Community Node や relay を発見と hole punching にだけ使う、P2P の接続。
3. Relay Fallback: 上の 2 つが成立しないときだけ、実データが relay を通る。

既定の発見は seeded DHT である。Community Node を使うときは、`set_community_node_config` で設定し、その node の文書に同意する。端末を直接つなぐときは、`get_local_peer_ticket` の ticket を相手が `import_peer_ticket` で取り込む。

## 4 つの機能

どれも単独で使える。使わないものは scene に置かない。Arena の `Assets/CyberDiscArena/Scenes/Minimal` に、機能ごとの最小の scene がある。

| 機能 | 置くもの | 設定 |
| --- | --- | --- |
| ロビーと対戦 | `Runtime/Lobby/KukuriLobby.prefab`。NGO を使うなら `NetworkManager` の Transport に `KukuriNetcodeTransport` | `runner`・`applicationId`・`topic`。掲示板と同じく、`KukuriAccountScreen.Screen` が `playing` になってから有効にする。組み方は package の sample「Netcode Sync」 |
| 掲示板 | `Runtime/Social/KukuriBoard.prefab` | `runner`・`topic`。GameObject を無効で置き、`KukuriAccountScreen.Screen` が `playing` になってから有効にする（下の script） |
| スキン | `KukuriSkinLoader`（glTFast が要る） | `runner`・`topic`・信頼する発行者の公開鍵・application ID・asset ID。組み方は package の sample「Skin Loader」 |
| Live | 配信は `KukuriLiveBroadcaster`、視聴は `KukuriLiveViewer` | どちらも code で `Client`（`runner.Client`）と `Topic` を設定する。配信は `SourceCamera` も設定し、`create_live_session` で作った session を `StartBroadcast` に渡す。視聴は `StartWatching` で、映像は `screen` の Renderer と `Output` の RenderTexture に出る。組み方は Arena の `Assets/CyberDiscArena/Scripts/ArenaBroadcast.cs` |

UI の prefab は、scene に `EventSystem` が無ければ Unity の menu の UI から足す。

### 自分のゲームに掲示板だけを足す

ゲームの通信はそのままでよい。NGO の Unity Transport で動いているゲームに足すときも、Transport を替えない。足すのは次の 4 つだけである（`sdk/unity/test-partial.ps1` で確かめている）。

1. `manifest.json` の 1 行（上の「導入」）。
2. 設定 asset と、`KukuriRunner`・`KukuriAccountScreen` を持つ GameObject（上の「ゲームを始める」）。
3. `KukuriBoard.prefab`。`runner` と `topic` を設定し、GameObject を無効にして置く。scene に `EventSystem` が無ければ、Unity の menu の UI から足す。
4. 掲示板を開く script。同じ GameObject に足し、`account` と `board` を設定する。開始の前は SNS を開かない。開始の画面は Canvas の上に描かれ、押すと両方が反応するので、掲示板を開く間は `Visible` を `false` にする。

```csharp
using Kukuri.GameSdk;
using Kukuri.GameSdk.Social;
using UnityEngine;

public sealed class BoardOpener : MonoBehaviour
{
    [SerializeField] KukuriAccountScreen account;
    [SerializeField] KukuriBoard board;

    void Update()
    {
        if (account.Screen != "playing" || board.gameObject.activeSelf) return;
        account.Visible = false;
        board.gameObject.SetActive(true);
    }
}
```

これは開始してすぐ開く最小の形で、開始の画面を隠したままにする。「kukuriで続ける」などのアカウントの操作を使うゲームは、掲示板を閉じたら `Visible` を `true` に戻す。キーで開閉する例は Arena の `Assets/CyberDiscArena/Scripts/ArenaBoard.cs` にある。

### スキンを発行する

発行は開発者の手元からだけ行う。秘密鍵はゲームと package に入らない。

1. 設定 asset（`kukuri/Settings`）の mode を Sidecar にして、Editor で Play する。
2. `publish_game_asset` で、ファイルを application ID と asset ID を付けて発行する。形式は `.glb`・PNG・JPEG・AssetBundle。
3. 新しい版は Player が自動で取得して差し替える。ゲームの再起動は要らない。

ゲームの Player（FFI）からは発行できない。署名は発行元と内容の一致を確かめるもので、アセットが無害であることの保証ではない。

## アカウントの任意の操作

どれも利用者が選んだときだけ使う。ゲームの開始の条件にしない。`KukuriAccountScreen` が画面を持っている。

| 操作 | 画面 | 結果 |
| --- | --- | --- |
| 既存の kukuri アカウントで始める | 開始の画面の「kukuriアカウントを使う」 | kukuri app の「別の端末へ移す」でコピーしたリンクを「貼り付け」、両方で確認コードを承認して「はじめる」を押す。鍵の文字列は入力しない。この鍵はゲームの profile に渡り、kukuri app からは取り消せない |
| desktop・Web でも使う | プレイ中の「kukuriで続ける」 | リンクを kukuri app の「別の端末から移す」で受ける。同じ鍵なので、投稿の ID と署名はそのまま続く。ゲームから desktop へ移す経路は確かめた。Web は同じ移行の仕組みを使うが、ゲームから Web へ移す組み合わせは確かめていない（2026-11-04 のユーザー判断）。Web は Community Node が要る |
| 既存のアカウントへまとめる | プレイ中の「既存のアカウントへまとめる」 | ゲームの結び付きを別のアカウントへ付け替える。古い投稿は元のアカウントのまま残る |
| 新しい PC で再開する（Steam） | プレイ中の「Steam Cloud に保存」（`steamCloud` の設定） | 下の「外部ログイン」 |

暗号化 export の取り込み（開始の画面の「復旧（上級）」）は、端末を失ったときの操作である。通常の開始には使わない。

### 外部ログインと復旧（Steam）

最初の外部ログインは Steam である。SDK は Steam を扱わず、ゲームで作ったアカウントの鍵と game player を profile の `game-account.json` に書く。同期は Steamworks の Auto-Cloud が行う。

1. Steamworks の App Admin の Cloud で、Auto-Cloud に `game-account.json` を足す。Root は `WinAppDataLocalLow`、Subdirectory は `<会社名>/<製品名>/kukuri/profiles/<profile>`、Pattern は `game-account.json`。利用者ごとの容量は file 1 件・数 KB で足りる。
2. 新しい PC では、Steam が起動の前に file を戻す。同意 →「はじめる」で、同じアカウントと game player で再開する。

利用者に示すこと:

- Steam アカウントを使える人は、このゲームの kukuri の鍵も使える。Steam（Valve）も同じである。
- 保存をやめても、すでに Steam に渡った写しは取り消せない。
- 「kukuriアカウントを使う」で受けたアカウントと、まとめた後のアカウントは保存できない（`not_game_account`）。

実際の Steam では確かめていない。Steam Cloud の同期を file の写しで模して確かめた（2026-11-04 のユーザー判断）。Steamworks の AppID と登録料は、ゲームの作者が用意する。

## 開発中の Sidecar

設定 asset（Project の Create の `kukuri/Settings`）の mode を Sidecar にすると、Play の開始時に Editor が `kukuri-sdk-sidecar.exe` を起動する。Play を止めても動き続けるので、接続と runtime を保ったまま Play をやり直せる。C# の API は同じである。

- 止める: Editor の終了か、menu の「kukuri/Sidecar を止める」。
- Player の build は Sidecar を起動しない。既定は FFI で、別の process は要らない。

## 費用と運用

kukuri はサーバーを必須にしないが、常に費用がかからないわけではない。必要に応じて、次を誰が用意するかをゲームの作者が決める。

| 役割 | 働き | 無いと | 費用の負担 |
| --- | --- | --- | --- |
| 初回の seed | 公開の DHT や既知の端末から、最初の相手を見つける | 相手を見つけるのが遅くなる。ticket を直接交換すればつながる | 既定の DHT は公開の網で、運用費は無い |
| 保持する端末 | 投稿とアセットを持ち続け、他の端末へ渡す | 発行者も保持する端末も居ないと、アセットを取得できない（取得できないことが返る） | プレイヤーの端末。作者が常時動く端末を置くなら作者の負担 |
| Community Node・relay | 発見と hole punching を助ける。直接つながらない端末の実データを中継する（Relay Fallback） | 直接つながらない組み合わせはつながらないことがある。Web 版は Community Node が要る | 運用者の負担（VM と通信量）。中継の通信量は利用に応じて増える |
| Steam | 鍵の同期（外部ログイン） | 新しい PC での再開ができない。手元のプレイは続く | Steamworks の登録料はゲームの作者の負担 |

性能の閾値は、指定の構成（ADR 0070 §8.4）で確かめたものである。プレイヤーの回線と端末での性能を保証しない。

## 診断と停止

| 症状 | 確かめること |
| --- | --- |
| 画面が「起動しています…」のまま | 画面の「未接続」は `KukuriRunner` の `settings` が空のときに出る。設定 asset を入れる |
| SDK が開かない | `KukuriRunner.OpenError` の code。`abi_mismatch` は DLL と C# の版の違いで、package を入れ直す。DLL を差し替えたら Editor を再起動する（Windows は読み込んだ DLL を解放しない） |
| `consent_required` | 同意の前である。`KukuriAccountScreen` を置き、同意と「はじめる」を済ませる |
| `profile_in_use` | 同じ profile を別の process が使っている。Sidecar が動いていないか確かめ、別の Player には別の profile を渡す |
| `sidecar_missing` | package に Sidecar が入っていない。配布物の `.tgz` を入れ直す |
| 相手が見つからない | `get_sync_status` の `topic_diagnostics` の `peer_count` と `active_path`（経路）。ticket を交換するか、Community Node を設定する |
| アセットを取得できない（`unavailable`・`not_found`） | 発行者か保持する端末が動いているか。発行者の公開鍵と application ID・asset ID が合っているか |
| Live の映像が出ない | 状態（`KukuriLiveViewer.State`）が `no_media` なら配信者がまだ映像を置いていない。音声の出力の機器が無い PC では、音声のある Live の映像も届かない |
| 実行の log | Editor は Console、Player は `Player.log`。SDK は秘密を log に書かない |

停止は `KukuriRunner` を破棄するか、ゲームを終了する。profile の lock は終了で外れる。profile の dir を消すと、次の起動は新しいアカウントと game player で始まる。Steam Cloud や別の端末へ移していなければ、元のアカウントには戻れない。keyring の鍵の項目は消えずに残る。
