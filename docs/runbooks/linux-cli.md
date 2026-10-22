# Linux CLIの利用

Linux CLIの配布対象はx86_64／aarch64。[v0.2.0-preview.2](https://github.com/kukuri-app/kukuri/releases/tag/v0.2.0-preview.2)以降の各Previewリリースで両archを[最新のGitHub Release](https://github.com/kukuri-app/kukuri/releases/latest)に同時公開している。
`kukuri-cli_<version>_<target>.tar.gz`を展開し、`bin/kukuri-cli --version`で版を確認する。
別の版を使う場合も、取得先と提供archはそのReleaseの成果物一覧を確認する。

GUIとは異なるCLI専用profileを使用する。日常GUIの保存先を`KUKURI_APP_DATA_DIR`で共有しない。
必要な同意内容と年齢条件は本人が確認してから、次の明示操作を行う。

```bash
bin/kukuri-cli --profile work consent status
bin/kukuri-cli --profile work consent accept --accept-documents --age-confirmed
bin/kukuri-cli --profile work daemon run
```

`daemon run`はforegroundで待機し、readyになったら別端末から状態とschemaを取得できる。

```bash
bin/kukuri-cli --profile work call client.status
bin/kukuri-cli --profile work call protocol.schema
bin/kukuri-cli --profile work call protocol.commands
```

指定した公開topicの内容を画面なしで取り込み、保持して、ほかのpeerへ返すときは、daemonの起動後に保持するtopicを設定する（#1909）。
設定はprofileに保存し、daemonを起動し直すと戻る。`get_topic_retention`は、保持するtopicと、topicごとに取り込んだ位置を返す。

```bash
echo '{"topic":"kukuri:topic:general","retained":true}' | bin/kukuri-cli --profile work call set_topic_retention --input -
bin/kukuri-cli --profile work call get_topic_retention
echo '{"topic":"kukuri:topic:general","retained":false}' | bin/kukuri-cli --profile work call set_topic_retention --input -
```

foreground端末のCtrl+Cまたは当該processへのSIGTERMで正常終了する。
`daemon start/status/stop`は既存systemd user instance用で、CIのforeground smokeとは区別する。
systemdの利用は任意で、新しいサービス管理機能を配布の前提にしない。

通常の応答はJSON、streamはNDJSON。exit codeとcommand metadataの正本は
`protocol.schema`／`protocol.commands`と[protocol仕様](../adr/0049-linux-gui-cli-control-plane.md)。
1入力はhandlerを最大1回実行し、timeout／切断時に自動再実行しない。成否不明の変更を無条件に再送しない。
idempotency keyや永続的な重複排除台帳は提供しない。

秘密値は専用のfile／FD入力・出力を使い、argvや通常stdoutへ載せない。
初回keyring不在時の設定と既存identityの保護を区別し、鍵取得失敗を新identity生成で回避しない。
配布binaryのsmokeは一時profile・一時runtime directory・file-backend fixtureだけを使用し、
ホストのprofile／keyring／systemd設定を変更しない。aarch64はQEMUまたはnative ARMで実行結果を記録する。
