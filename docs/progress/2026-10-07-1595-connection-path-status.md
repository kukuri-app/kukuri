# #1595: 経路のみの切替を差分診断へ反映する

対象・受入条件・PR枠は [#1595](https://github.com/kukuri-app/kukuri/issues/1595) の
Scope revision `2026-10-05-r1` を再利用する。AC-1 / PR-1は区分B、独立監査は不要とした。
作業基準は `3d3d68a25c654a56137e6d36a26d1baff522e039`。接続制御・認証・wire・UI/API形式は変更しない。

## 再現と結果

使用中iroh fork `e0b0ad9892e8881480d0f48a74a3148c8b896d79` の
`Connection::path_events` / `paths_stream` は、開いている経路と選択の変化を通知する。
現行のtopic受信taskはgossipのイベントだけを待っており、経路変化では`StatusChanges`をmarkしていなかった。
`remote_info`のActiveは開いている経路を表す。選択された経路だけを変更しても診断が変わるとは限らない。

修正前に `path_only_switch_marks_the_topic_without_hints` を追加し、同じQUIC接続とneighborを保ち、
hintを送らずにrelay→IP / IP→relayを観測した。Windowsの実irohとローカルrelayで、両方向とも
topicの印が3秒以内に届かず、`missing path-only status trigger`で失敗した（11.33秒）。
IP経路の開閉は`paths_stream`で判定し、statusの定期・手動読取りは使っていない。

最終fixtureは、loopbackのIPv4だけをbindし、切替ごとに新しいUDP転送の到達候補を通知して
実IP経路を開き、selectorを再評価させる。QUIC datagramを選択中の経路だけへ送り、hintは送らない。
backup経路はidle timeoutで閉じる。全経路のkeep-aliveや外部IPの観測reportは使わず、
一度閉じた候補の再試行の有無や、選択変更だけで開いたままのIP経路に依存しない。
差分の印が届いた時だけ診断を読む1秒coalesceの受信fixtureで、各方向の`active_path`と
`fallback_peer_count`（1→0→1）、hint受信時刻が未設定、接続・neighborの維持、退出の通知を照合する。
pushの連番も照合し、古いfallback値が残っているだけではIP→relayの通知成功と扱わない。
最終fixtureで経路変化のmarkだけを一時的に外した対照実行でも、両方向が新しいpushを受け取れず
失敗した（7.28秒）。その際の実IP/relayのActive状態は期待どおりで、診断は古い値のままだった。
実desktop-runtime observerの差分適用・1秒coalesce・停止は既存のruntime testで別途検証した。

## 実装と所有・停止

- endpointのhandshake hookはgossip接続の弱参照だけを有界な窓（1024件）へ入れる。
  新しい接続の通知は256件まで。hookは常にAcceptを返し、他のhookの判定を変更しない。
  閉じた弱参照は次のhandshakeで除去し、transport停止で窓を空にする。
- 既存topic受信taskがneighborの接続の`path_events`を待ち、該当topicだけをmarkする。
  同じ接続の購読を重複させず、接続終了・NeighborDown・topic終了で解放する。
  購読終了後のhandshake通知receiver数が0になることをfixtureで確認する。
- 通常bindと共有nodeのendpointに同じhookを取り付ける。共有endpointを使うdesktop-runtimeと
  app-apiの実iroh test fixtureも、そのhookをtransportへ渡す。
- 既存の診断判定・`StatusChanges`・desktop-runtimeのobserverを利用する。
  新たなtask・probe・全peer/topicの周期読取り・並行する診断基盤は追加しない。
  frontendの60秒読取りも維持する。仕様はADR 0055 §3.2へ反映した。

## 検証

Windowsで以下を実行した。

- `cargo test --locked -p kukuri-transport iroh::tests::connection_path -- --nocapture`: 3件成功。
  relayのみのfallback、relayによる接続補助、経路のみの両方向の切替を確認。
- `cargo test --locked -p kukuri-transport iroh::tests::status_diff -- --nocapture`: 3件成功。
  変わったtopicだけの印、無変化時の待機、履歴を10倍にした時の読取り量不変を確認。
  終了時に既存iroh-gossip taskのCancelled panicが出たが、testの結果は成功。
- `cargo test --locked -p kukuri-transport iroh::tests::connection_release -- --nocapture`: 4件成功。
  弱参照が接続を延命しないことと、別topicの継続・接続の交代を確認。
- `cargo test --locked -p kukuri-desktop-runtime tests::runtime_events:: -- --nocapture`: 8件成功。
  既存observerの差分・1秒coalesce・停止・休止した履歴への非依存を含む。
  共有nodeとdesktop-runtimeのcompileも成功。MSVCのリンク成果物作成メッセージのみwarningとして出力。
- `cargo fmt --all -- --check` / `git diff --check`: 成功。

PRの初回CI（head `9d6b7526ced872e2b9400bdf370a57423d7f5559`）では、LinuxがbackupのIP経路を
keep-aliveで保持し続け、fixtureの最初のrelayのみの状態を作れずに失敗した。診断のassertへ到達する前の
準備条件の不足であり、上記の選択経路だけのdatagramとloopback限定へfixtureを修正した。
本番の通知実装・受入条件は変更していない。修正後は同じconnection_pathの3件をWindowsと
WSL Ubuntu 22.04のLinuxで確認し、いずれも成功した。

全体・Linux/Webの確認はPR CIで行う。CI・merge・Issueの最終判定はIssueのCurrent statusに集約する。
AC-2 / PR-2のログ修正は [PR #1643](https://github.com/kukuri-app/kukuri/pull/1643) として別に実施し、
22 check成功・3 skipの後に `88d6c2624c5f7a4abf4c26d00e5de9a0206dde77` へmerge済み。
