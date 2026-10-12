# OpenAI Moderation と動画スキャンの運用

本手順は [ADR 0028 §9](../adr/0028-nondeterministic-moderation-vlm.md) と
[#1060 の検証記録](../progress/2026-09-17-1060-openai-video-moderation.md) に対応する。
専用 provider 名は `openai-moderation`。既存の `openai-compatible-vlm` と設定・応答形式が異なる。

## 設定と配備

`safety.providers.general.provider: openai-moderation` を指定する。known_csam は既存の
`project-arachnid-shield` を維持する。OpenAI provider を known_csam / unknown_csam へ指定しない。
この provider は未知CSAM専用検出器や grooming 検出器を提供しない。

キーは既存の `COMMUNITY_NODE_VLM_API_KEY` を必須として読む。`OPENAI_API_KEY` への fallback はない。
GCPでは既存の `deploy.vlm_api_key_secret_id` に Secret Manager のIDを指定し、キー値を
operator config / Terraform / Git に書かない。非秘密設定は `deploy.moderation` に置く。

```yaml
safety:
  providers:
    known_csam:
      provider: project-arachnid-shield
      required: true
    general:
      provider: openai-moderation
      required: true
      hosting: external
deploy:
  vlm_api_key_secret_id: your-existing-secret-id
  moderation:
    model: omni-moderation-latest
    config_version: "1"
    rpm: 400
    rpd: 8000
    tpm: 8000
    image_tokens: 2048
```

これは既存 operator config へ加える断片。その他の必須項目は
[operator docs](community-node-operator-docs.md) と [GCP配備](community-node-gcp-terraform.md) を参照する。
`cn-operator` の生成する tfvars は low-cost profile に対応する。

| 環境変数 | 既定値・意味 |
| --- | --- |
| `COMMUNITY_NODE_MODERATION_API_BASE_URL` | `https://api.openai.com/v1`。本番はHTTPS、資格情報・query・fragmentをURLへ含めない |
| `COMMUNITY_NODE_MODERATION_MODEL` | `omni-moderation-latest` |
| `COMMUNITY_NODE_MODERATION_CONFIG_VERSION` | `1`。モデル更新・再評価の明示的な世代 |
| `COMMUNITY_NODE_MODERATION_RPM` / `RPD` / `TPM` | 400 / 8000 / 8000。初期Tier 1の500 / 10000 / 10000以内 |
| `COMMUNITY_NODE_MODERATION_IMAGE_TOKENS` | 2048。単画像requestの保守的予約量 |
| `COMMUNITY_NODE_VIDEO_TMPDIR` | Composeは `/run/kukuri-video` の64 MiB tmpfs。直接起動は `/dev/shm/kukuri-video` |

本文・静止画・動画各frame・readiness・retryはPostgreSQL上の同じ予算へ予約する。
予約は送信失敗時も戻さず、キー・構成変更やprocess再起動でも予算をリセットしない。
複数processが同じDBを使う場合も共有する。別DB・別アプリの同じOpenAI project使用量までは管理しない。
運営者の実projectのLimitsを確認し、他用途の余裕も残す。本文はUTF-8 byte数を保守的なtoken予約量とするため、
単requestのTPM枠を超える長文は切り詰めずfail-closedとなる。

`docker/cn/Dockerfile` は cn-indexer / cn-cli imageへFFmpeg・ffprobeを導入し、decoder build情報を保存する。
ComposeのindexerとTerraformのreadinessには専用tmpfsを設定する。通常diskへfallbackしない。
入力decoder・probe・JPEG encoderは各1threadに固定する（`video-midpoints-v2`）。
MP4/H.264、WebM/VP8・VP9、32 MiB・600秒以下・長辺3840/短辺2160以下を扱う。
`N=min(8,max(1,ceil(duration/5秒)))` で全区間の中央時刻から長辺512px以下のJPEGを作り、1requestにつき1枚送る。
静止画はJPEG/PNG/GIF/WebPを正規化し、アニメーションGIF/WebP/APNGは拒否する。

## 公開前の確認・更新・障害

1. DB migration、image更新、既存Secret Managerの注入、tmpfsのmountを先に行う。
2. `cn-indexer validate-config` でキーとdecoder構成を検証する。
3. `cn-cli readiness --config <operator-config.yaml> --force-probe` を実行する。
   `COMMUNITY_NODE_DEPLOYMENT_REVISION` と既存のDB・indexer・relation設定も必要。
   OpenAIのprobeは同梱した無害なMP4/WebMを実decodeし、合成本文とJPEGを実APIへ送る。
   Arachnidは従来の合成PDQ probeを維持する。全readinessがPASSの場合だけ公開を有効化する。
   GCP構成では、この実行の直後に `sudo systemctl start kukuri-readiness.service` を実行し、
   `systemctl list-timers kukuri-readiness.timer` の `NEXT` が時刻になっていることを確認する。
   `docker-compose run` だけで終えると、timerの記録と次回実行の確認から外れる
   （手順は [production rollout §5.2](community-node-production-rollout.md#52-readiness)、#1097）。
   force-probeの失敗結果も15分間は再利用されるため、原因を直した後は再度 `--force-probe` から実行する。
4. モデル、前処理、decoder build、node署名ID、policyの変更は内容判定キーを変える。
   `latest` の提供内容が更新された場合は `config_version` を増やし、再起動・`--force-probe`を行う。再取り込みが必要なら対象の投稿・範囲と終了条件を先に決め、構成変更を理由に全履歴の再取得・再scanを必須にしない。
   secretの値はfingerprintに含めないため、キーrotationも `--force-probe` を行う。
5. rollbackも構成世代を明示し、旧imageに新providerを指定したまま公開しない。
   decoder・認証・DB・応答形式が不正な場合やframeの一部が失敗した場合はhold / de-indexを維持する。

readinessのキャッシュはproviderとdecoder構成の一致、および期限内の時刻が必要。
キー欠落やdecoder/tmpfs欠落時は古いPASSを再利用しない。401/403・入力4xxは自動retryしない。
429・一時5xx・通信障害は最大3attempt、Retry-Afterとbackoff、scan全体300秒の範囲で再試行する。
失敗・cancel・途中成功を共通完了cacheに保存しない。claimは取消時に解放し、crash時は320秒のlease満了で回復する。

### readinessの失敗表示と一時失敗（#1091）

`provider_credential_valid` の general は `<段階>に失敗: <分類>` を表示する。表示は固定文言とHTTP statusの数値だけで作り、
キー・API応答本文・mediaを含めない。段階は順に実行し、decoderの確認に失敗した場合はOpenAIへの要求と共有予算の予約を行わない。

| 段階 | 主な分類と確認先 |
| --- | --- |
| 設定不備 / 資格情報未設定 | `COMMUNITY_NODE_MODERATION_*` の値、`COMMUNITY_NODE_VLM_API_KEY` の注入 |
| 動画decoderの初期化 | 実行ファイル、作業領域（専用tmpfs）の有無・権限、抽出設定値、作業領域の保守処理との競合 |
| 動画decoderの確認 | 初回起動の準備が時間切れ、ffprobe / ffmpeg の時間切れ、起動失敗、異常終了または資源上限、同梱動画の検証失敗 |
| OpenAI本文 / 画像の確認 | 認証拒否（401/403）、頻度制限（429）、プロバイダ側エラー（5xx）、予期しない応答、共有予算の枯渇・DB不可、時間切れ、通信失敗、応答の解釈失敗 |

decoderの初回起動は、page cacheが冷えた直後（image更新直後など）に数秒かかることがある。
各extractorは最初の抽出の前に同じ隔離環境で `ffprobe -version` を実行し、decode期限（最大30秒）の範囲で読み込みを済ませる。
probe期限（最大5秒）は入力の解析だけに使う。起動後にprobeが時間切れになった場合は、準備を1回やり直してから1回だけ再試行する。
「初回起動の準備が時間切れ」が続く場合は、VMのCPU・disk I/Oの負荷とimageの配置を確認する。

作業領域の保守処理（残留jobの回収とjob directoryの作成）は、同じtmpfsを使う他のextractorや、
fork直後の子processが保持するlockと短時間競合する。作成処理は最大2秒待ち、それを超えた場合だけ「作業領域の保守処理との競合」で失敗する。
Composeでは各containerが専用tmpfsを持つ。直接起動でindexerとreadinessが `/dev/shm/kukuri-video` を共有する構成でも、待機の範囲内なら失敗しない。

## 保存範囲と開示

OpenAIへ送るのは本文・正規化した静止画・動画由来JPEGであり、原動画・音声は送らない。
既存Arachnidの既知hash照合経路は維持し、そのmedia APIへは元メディアを送る場合がある。
CNは原メディア・frame・data URL・API生応答を恒久保存しない。tmpfs上のjobは終了・失敗・cancelで回収し、
再起動時に使用中でない残留jobも回収する。子processはAPIキーを継承せず、seccompでnetwork syscallを拒否する。

DBの共通cacheは内容hash＋node/provider/model/policy/前処理構成に対応する正規化判定と最小coverageのみ。
投稿・著者・scope・appealは含めず、参照元の署名・supported scope・撤回・削除・送信防止を再確認して関連付ける。
完了結果cacheは [#1221 R5-F](../progress/2026-09-26-1221-r5f-cn-retention.md) のW/B/T回収に接続済み。`worker::run → IndexMaintenance::reclaim_retention_pass → reclaim_expired` が受入下限Wと容量B・期間Tに従い、投影と保存物を合わせて1回128件以内、巡回ごと最大16回で回収する。これはworkerによる段階回収で、load時の個別TTL失効ではない。構成世代の更新はキー変更であり、旧cacheの回収自体ではない。法務/運営保存は対象外。保持設定・operator操作は変えず、本手順のために全件GCを追加・実行しない。

カテゴリbooleanで判定し、scoreは同カテゴリのconfidenceとして保持する。動画はカテゴリごとにOR / MAXで集約する。
画像非対応のカテゴリと音声は未検査。サンプリングの間に短時間だけ現れる内容を見逃し得るため、全フレーム検査や
未知CSAM検出を表明しない。生成される外部送信文書にもこの範囲を記載する。

## 計測と検証

indexerの `GET /v1/status` の `moderation` にAPI attempt数、scan完了/失敗数・合計時間、decode数・合計時間、
動画duration合計・frame数を出す。既存の `media_fetch_success` / `scans_fresh` / `scans_reused` と併せて差分を測る。
本文・画像・API生応答・キーをmetricに含めない。counterはprocess再起動で0に戻り、予算と内容cacheはDBに残る。

- ローカルは変更したprovider・decoder・予算の関連testを選び、全体はPR CIで確認する。`cargo xtask cn-check`、`cargo xtask cn-test`、`cargo xtask cn-e2e`は全体確認の入口。
- 無害な実API検証: Linuxで `KUKURI_CN_RUN_LIVE_MODERATION_TESTS=1`、テスト専用の
  `COMMUNITY_NODE_DATABASE_URL`、`COMMUNITY_NODE_VLM_API_KEY` を設定し、
  `cargo test -p kukuri-cn-indexer --test live_moderation -- --nocapture`。
  投稿/blob sourceと投影はメモリ内、既知hash結果はテストdoubleを使う。PostgreSQL、署名付きingest、
  専用provider、decoder、共有予算・cacheは実装を通す。実APIへ送るのは合成blue映像とbenign本文だけ。
  通常CIでは実API呼出を実行しない。production image内でも同じtest binaryを起動して環境依存を確認する。
