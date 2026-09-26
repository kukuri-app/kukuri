variable "name_prefix" {
  description = "リソース名の接頭辞。"
  type        = string
}

variable "project_id" {
  description = "GCP project ID。"
  type        = string
}

variable "zone" {
  description = "VM を作成する zone。"
  type        = string
}

variable "machine_type" {
  description = "Compute Engine machine type。"
  type        = string
  default     = "e2-small"
}

variable "disk_size_gb" {
  description = "boot/persistent disk サイズ（GB）。Postgres data も同 disk 上に置く。"
  type        = number
  default     = 30
}

variable "boot_image" {
  description = "VM boot image。Docker を含む Container-Optimized OS 既定。"
  type        = string
  default     = "projects/cos-cloud/global/images/family/cos-stable"
}

variable "network_self_link" {
  description = "接続する VPC network の self_link。"
  type        = string
}

variable "subnet_self_link" {
  description = "接続する subnet の self_link。"
  type        = string
}

variable "static_ip" {
  description = "VM に割り当てる static external IP アドレス。"
  type        = string
}

variable "network_tags" {
  description = "firewall を適用する network tag 群。"
  type        = list(string)
  default     = ["kukuri-community-node"]
}

# --- public endpoints / TLS ---
variable "api_domain" {
  description = "cn-user-api の公開 hostname。"
  type        = string
}

variable "relay_domain" {
  description = "cn-iroh-relay の公開 hostname。"
  type        = string
}

variable "acme_email" {
  description = "ACME(Let's Encrypt) 登録に使う email。"
  type        = string
}

variable "relay_quic_port" {
  description = "cn-iroh-relay の QUIC/UDP ポート。"
  type        = number
  default     = 7842
}

# --- container images (public GHCR) ---
variable "cn_user_api_image" {
  description = "cn-user-api の公開 container image（GHCR、tag/digest 込み）。"
  type        = string
}

variable "cn_iroh_relay_image" {
  description = "cn-iroh-relay の公開 container image（GHCR、tag/digest 込み）。"
  type        = string
}

variable "cn_cli_image" {
  description = "cn-cli (migrate) の公開 container image（GHCR、tag/digest 込み）。"
  type        = string
}

variable "caddy_image" {
  description = "Caddy reverse proxy container image。"
  type        = string
  default     = "caddy:2"
}

variable "acme_image" {
  description = "ACME companion (certbot) container image。"
  type        = string
  default     = "certbot/certbot:latest"
}

# --- data tier (low-cost local containers) ---
variable "deploy_local_postgres" {
  description = "true なら VM 上に Postgres container を立てる（low-cost）。false なら external_database_url を使う。"
  type        = bool
  default     = true
}

variable "deploy_local_valkey" {
  description = "true なら VM 上に Valkey container を立てる（low-cost）。false なら external_redis_url を使う。"
  type        = bool
  default     = true
}

variable "postgres_image" {
  description = "local Postgres container image。"
  type        = string
  default     = "postgres:17-bookworm"
}

variable "valkey_image" {
  description = "local Valkey container image。"
  type        = string
  default     = "valkey/valkey:8-alpine"
}

variable "postgres_user" {
  description = "Postgres user 名。"
  type        = string
  default     = "cn"
}

variable "postgres_db" {
  description = "Postgres database 名。"
  type        = string
  default     = "cn"
}

variable "postgres_data_disk_gb" {
  description = "deploy_local_postgres=true のとき、Postgres data 用の専用 persistent disk サイズ（GB）。0 なら boot disk 上の docker volume を使う（VM 置換でデータ消失リスクあり）。"
  type        = number
  default     = 0
}

# 外部 DB（managed-db / ha 拡張点）。password は URL に焼かず、Secret Manager から
# boot 時に取得して URL-encode する。Terraform state / VM metadata に平文を残さない。
variable "external_db_host" {
  description = "deploy_local_postgres=false のときの外部 Postgres host。"
  type        = string
  default     = ""
}

variable "external_db_port" {
  description = "外部 Postgres port。"
  type        = number
  default     = 5432
}

variable "external_db_user" {
  description = "外部 Postgres user。"
  type        = string
  default     = ""
}

variable "external_db_name" {
  description = "外部 Postgres database 名。"
  type        = string
  default     = ""
}

variable "external_db_password_secret_id" {
  description = "外部 Postgres password を保持する Secret Manager secret ID（deploy_local_postgres=false のとき必須）。"
  type        = string
  default     = ""
}

variable "external_db_password_secret_version" {
  description = "外部 Postgres password の version。"
  type        = string
  default     = "latest"
}

variable "external_redis_url" {
  description = "deploy_local_valkey=false のときに使う外部 Redis/Valkey URL（managed-db 拡張点）。"
  type        = string
  default     = ""
}

# --- secrets (Secret Manager IDs, not payloads) ---
variable "jwt_secret_id" {
  description = "COMMUNITY_NODE_JWT_SECRET を保持する Secret Manager secret ID。"
  type        = string
}

variable "jwt_secret_version" {
  description = "JWT secret の version（既定 latest）。"
  type        = string
  default     = "latest"
}

variable "postgres_password_secret_id" {
  description = "Postgres password を保持する Secret Manager secret ID（local Postgres のとき必須）。"
  type        = string
  default     = ""
}

variable "postgres_password_secret_version" {
  description = "Postgres password の version（既定 latest）。"
  type        = string
  default     = "latest"
}

variable "accessor_secret_ids" {
  description = "VM service account に read 権限を付与する Secret Manager secret ID 群。VM 起動前に binding を確実に作るため、instance が depends_on する。"
  type        = list(string)
  default     = []
}

# --- cn-user-api tuning ---
variable "jwt_issuer" {
  description = "COMMUNITY_NODE_JWT_ISSUER。"
  type        = string
  default     = "kukuri-cn"
}

variable "jwt_ttl_seconds" {
  description = "COMMUNITY_NODE_JWT_TTL_SECONDS。"
  type        = number
  default     = 86400
}

variable "rendezvous_key_prefix" {
  description = "COMMUNITY_NODE_RENDEZVOUS_KEY_PREFIX。"
  type        = string
  default     = "cn:rendezvous:v1"
}

variable "rate_limit_enabled" {
  description = "COMMUNITY_NODE_RATE_LIMIT_ENABLED。"
  type        = bool
  default     = true
}

variable "rate_limit_per_second" {
  description = "COMMUNITY_NODE_RATE_LIMIT_PER_SECOND。"
  type        = number
  default     = 10
}

variable "rate_limit_burst" {
  description = "COMMUNITY_NODE_RATE_LIMIT_BURST。"
  type        = number
  default     = 30
}

variable "admin_actor" {
  description = "IAP 内部 admin browser write の append-only audit に記録する deployment-controlled actor。空なら write を fail-closed で無効化する。"
  type        = string
  default     = ""

  validation {
    condition = (
      var.admin_actor == "" || (
        var.admin_actor == trimspace(var.admin_actor) &&
        length(var.admin_actor) <= 254 &&
        !can(regex("[\\r\\n\\x00-\\x1F\\x7F]", var.admin_actor))
      )
    )
    error_message = "admin_actor must be empty or a trimmed, single-line value of at most 254 characters."
  }
}

variable "iroh_relay_client_rx_bytes_per_second" {
  description = "任意の COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_BYTES_PER_SECOND。0 なら未設定。"
  type        = number
  default     = 0
}

variable "iroh_relay_client_rx_max_burst_bytes" {
  description = "任意の COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_MAX_BURST_BYTES。0 なら未設定。"
  type        = number
  default     = 0
}

# --- blob cache (disabled by default) ---
variable "blob_cache_enabled" {
  description = "blob cache を有効化するか。低コスト既定は false。"
  type        = bool
  default     = false
}

variable "blob_cache_size_gb" {
  description = "blob cache 専用ディスクサイズ（GB）。blob_cache_enabled=true のときのみ作成。"
  type        = number
  default     = 0
}

variable "blob_cache_ttl_hours" {
  description = "blob cache の TTL（時間）。env として記録（cache eviction の上限指針）。"
  type        = number
  default     = 24
}

variable "blob_cache_path" {
  description = "blob cache のマウント先 path（backup 対象外）。"
  type        = string
  default     = "/var/lib/kukuri/blob-cache"
}

# --- backup ---
variable "backup_enabled" {
  description = "pg_dump -> GCS backup を有効化するか。"
  type        = bool
  default     = true
}

variable "backup_bucket" {
  description = "backup 先 GCS bucket 名（backup_enabled=true のとき必須）。"
  type        = string
  default     = ""
}

variable "backup_schedule_oncalendar" {
  description = "systemd OnCalendar 形式の backup 実行スケジュール。"
  type        = string
  default     = "*-*-* 03:30:00"
}

# --- monitoring helpers passthrough ---
variable "index_expected_topics" {
  description = "索引対象に必須の公開topic集合。明示したnodeだけで欠落・空索引を監視し、自動登録はしない。"
  type        = set(string)
  default     = []
  validation {
    condition     = alltrue([for topic in var.index_expected_topics : can(regex("^[a-zA-Z0-9:_-]{1,200}$", topic))])
    error_message = "Expected index topics must be 1-200 ASCII letters, digits, colons, underscores or hyphens."
  }
}

variable "deployment_profile" {
  description = "deployment profile 名（メタ表示用）。"
  type        = string
  default     = "low-cost"
}

# --- index / moderation stack (#615) ---
variable "deploy_indexer_stack" {
  description = "true なら cn-indexer + ArcadeDB + relation 定期解析を VM compose に追加する。false へ戻すと従来の API / relay のみ構成になる（rollback 経路）。"
  type        = bool
  default     = false
}

variable "cn_indexer_image" {
  description = "cn-indexer の公開 container image（GHCR、tag/digest 込み）。"
  type        = string
  default     = "ghcr.io/kukuri-app/kukuri-cn-indexer:latest"
}

variable "arcadedb_image" {
  description = "ArcadeDB container image。latest は SNAPSHOT を指すため stable tag を使う。"
  type        = string
  default     = "arcadedata/arcadedb:26.8.1"
}

variable "indexer_data_disk_gb" {
  description = "cn-indexer data（iroh endpoint 同一性 / docs replica / blob store）用の専用 persistent disk サイズ（GB）。0 なら boot disk 上の docker volume（VM 置換でデータ消失。再同期で復元は可能だが endpoint 同一性は失われる）。"
  type        = number
  default     = 0
}

variable "relation_analyze_interval_minutes" {
  description = "`cn-cli relation analyze` の systemd timer 実行間隔（分、1〜90）。oneshot unit のため overlap しない。"
  type        = number
  default     = 60
  validation {
    # readiness は関係解析の成功を既定 7200 秒以内に要求する。boot / startup 再実行の直後は初回まで
    # 最大 16 分（OnBootSec 15 分 + 遅延 1 分）かかるため、解析と boot の所要時間に 14 分を残す（#1101）。
    condition     = var.relation_analyze_interval_minutes >= 1 && var.relation_analyze_interval_minutes <= 90 && floor(var.relation_analyze_interval_minutes) == var.relation_analyze_interval_minutes
    error_message = "relation_analyze_interval_minutes は 1〜90 の整数で指定してください（readiness が求める関係解析の成功記録 7200 秒以内を保つため）。"
  }
}

variable "indexer_own_relay" {
  description = "COMMUNITY_NODE_INDEXER_OWN_RELAY。low-cost は cn-iroh-relay を同梱するため既定 true。"
  type        = bool
  default     = true
}

variable "indexer_external_relay_urls" {
  description = "COMMUNITY_NODE_INDEXER_EXTERNAL_RELAY_URLS（カンマ区切りへ join される）。"
  type        = list(string)
  default     = []
}

variable "indexer_retention_days" {
  description = "COMMUNITY_NODE_INDEXER_RETENTION_DAYS。再取得可能な保存物の保持期間 T（日、3 以上）。deploy_indexer_stack=true のとき必須。"
  type        = string
  default     = ""
}

variable "indexer_capacity_rows" {
  description = "COMMUNITY_NODE_INDEXER_CAPACITY_ROWS。再取得可能な保存物の容量 B（行数）。deploy_indexer_stack=true のとき必須。"
  type        = string
  default     = ""
}

variable "index_query_enabled" {
  description = "COMMUNITY_NODE_INDEX_QUERY_ENABLED。full-stack E2E 完了までは false を維持する。"
  type        = bool
  default     = false
}

variable "trust_read_enabled" {
  description = "COMMUNITY_NODE_TRUST_READ_ENABLED。full-stack E2E 完了までは false を維持する。"
  type        = bool
  default     = false
}

variable "relation_distance_optout_min_proximity" {
  description = "COMMUNITY_NODE_RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY。(0, 1]。index/trust surface 有効時は明示必須。"
  type        = string
  default     = ""
}

# trust 合成の operator 可変パラメータ（ADR 0026 §6.2）。空文字なら binary 既定値。
variable "trust_w_abs_negative" {
  description = "COMMUNITY_NODE_TRUST_W_ABS_NEGATIVE。空なら未設定。"
  type        = string
  default     = ""
}

variable "trust_w_abs_positive" {
  description = "COMMUNITY_NODE_TRUST_W_ABS_POSITIVE。空なら未設定。"
  type        = string
  default     = ""
}

variable "trust_relative_half_life_days" {
  description = "COMMUNITY_NODE_TRUST_RELATIVE_HALF_LIFE_DAYS。空なら未設定。"
  type        = string
  default     = ""
}

# --- index / moderation secrets (Secret Manager IDs, not payloads) ---
variable "channel_secret_key_secret_id" {
  description = "COMMUNITY_NODE_CHANNEL_SECRET_KEY を保持する Secret Manager secret ID（deploy_indexer_stack=true のとき必須）。"
  type        = string
  default     = ""
}

variable "legal_data_key_secret_id" {
  description = "COMMUNITY_NODE_LEGAL_DATA_KEY を保持する Secret Manager secret ID（通報・権利侵害受付を有効にする場合は必須）。"
  type        = string
  default     = ""
}

variable "arcadedb_password_secret_id" {
  description = "ArcadeDB root password を保持する Secret Manager secret ID（deploy_indexer_stack=true のとき必須）。"
  type        = string
  default     = ""
}

variable "safety_signing_key_secret_id" {
  description = "moderation event signing key（COMMUNITY_NODE_SAFETY_SIGNING_KEY）を保持する Secret Manager secret ID。空なら注入しない。"
  type        = string
  default     = ""
}

variable "arachnid_username_secret_id" {
  description = "PROJECT_ARACHNID_API_USERNAME を保持する Secret Manager secret ID。空なら注入しない。"
  type        = string
  default     = ""
}

variable "arachnid_password_secret_id" {
  description = "PROJECT_ARACHNID_API_PASSWORD を保持する Secret Manager secret ID。空なら注入しない。"
  type        = string
  default     = ""
}

variable "vlm_api_key_secret_id" {
  description = "任意の COMMUNITY_NODE_VLM_API_KEY を保持する Secret Manager secret ID。空なら注入しない（self-host の無認証 endpoint）。"
  type        = string
  default     = ""
}

# --- safety provider slots / moderation tuning ---
variable "safety_provider_known_csam" {
  description = "COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM（例: project-arachnid-shield）。空なら slot 未構成。"
  type        = string
  default     = ""
}

variable "safety_provider_known_csam_required" {
  description = "COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM_REQUIRED。"
  type        = bool
  default     = false
}

variable "safety_provider_general" {
  description = "COMMUNITY_NODE_SAFETY_PROVIDER_GENERAL（例: openai-compatible-vlm）。空なら slot 未構成。"
  type        = string
  default     = ""
}

variable "safety_provider_general_required" {
  description = "COMMUNITY_NODE_SAFETY_PROVIDER_GENERAL_REQUIRED。"
  type        = bool
  default     = false
}

variable "safety_provider_unknown_csam" {
  description = "COMMUNITY_NODE_SAFETY_PROVIDER_UNKNOWN_CSAM（例: openai-compatible-vlm）。空なら slot 未構成。"
  type        = string
  default     = ""
}

variable "safety_provider_unknown_csam_required" {
  description = "COMMUNITY_NODE_SAFETY_PROVIDER_UNKNOWN_CSAM_REQUIRED。"
  type        = bool
  default     = false
}

variable "safety_emit_signed_events" {
  description = "COMMUNITY_NODE_SAFETY_EMIT_SIGNED_EVENTS。"
  type        = bool
  default     = true
}

variable "safety_suspected_threshold" {
  description = "COMMUNITY_NODE_SAFETY_SUSPECTED_THRESHOLD（1-100）。0 なら未設定（policy 既定）。"
  type        = number
  default     = 0
}

variable "safety_operator_review" {
  description = "COMMUNITY_NODE_SAFETY_OPERATOR_REVIEW。異議申し立ての運営者審査（変更操作）を有効化する。既定 true。運用者設定 safety.moderation.operator_review と一致させる。"
  type        = bool
  default     = true
}

variable "safety_suspected_signal_visibility" {
  description = "COMMUNITY_NODE_SAFETY_SUSPECTED_SIGNAL_VISIBILITY（local / subscribed_nodes / public）。空なら未設定。"
  type        = string
  default     = ""
}

variable "safety_general_action" {
  description = "COMMUNITY_NODE_SAFETY_GENERAL_ACTION（label / hold / exclude。nsfw / objectionable の suspected の扱い。ADR 0028 §8.7）。空なら未設定（既定 label = content advisory 付きで索引）。allow は受理されない。"
  type        = string
  default     = ""

  validation {
    condition     = contains(["", "label", "hold", "exclude"], var.safety_general_action)
    error_message = "safety_general_action は空 / label / hold / exclude のいずれかを指定してください。"
  }
}

variable "media_fetch_max_bytes" {
  description = "COMMUNITY_NODE_MEDIA_FETCH_MAX_BYTES。0 なら未設定（binary 既定）。"
  type        = number
  default     = 0
}

variable "media_fetch_timeout_secs" {
  description = "COMMUNITY_NODE_MEDIA_FETCH_TIMEOUT_SECS。0 なら未設定（binary 既定）。"
  type        = number
  default     = 0
}

# --- VLM endpoint (#420) ---
variable "vlm_api_base_url" {
  description = "COMMUNITY_NODE_VLM_API_BASE_URL。空なら未設定。self-host endpoint は WireGuard 等の private 経路で到達させ、public internet へ公開しない。"
  type        = string
  default     = ""
}

variable "vlm_model" {
  description = "COMMUNITY_NODE_VLM_MODEL。空なら未設定。"
  type        = string
  default     = ""
}

variable "vlm_response_format" {
  description = "COMMUNITY_NODE_VLM_RESPONSE_FORMAT（json / guard）。空なら未設定（binary 既定 json）。"
  type        = string
  default     = ""
}

variable "vlm_api_timeout_secs" {
  description = "COMMUNITY_NODE_VLM_API_TIMEOUT_SECS。0 なら未設定（binary 既定）。"
  type        = number
  default     = 0
}

# --- operator manifest (#380) ---
variable "operator_config_file" {
  description = "operator-config.yaml の中身（YAML 文字列）。空でなければ VM に配置し、cn-user-api の COMMUNITY_NODE_OPERATOR_CONFIG に設定して public manifest endpoint / report_endpoint gating を有効化する。空なら manifest endpoint は 404 のまま。"
  type        = string
  default     = ""
}

variable "moderation" {
  description = "Non-secret OpenAI Moderation settings; credential uses vlm_api_key_secret_id."
  type = object({
    api_base_url   = string
    model          = string
    config_version = string
    rpm            = number
    rpd            = number
    tpm            = number
    image_tokens   = number
  })
  default = {
    api_base_url   = "https://api.openai.com/v1"
    model          = "omni-moderation-latest"
    config_version = "1"
    rpm            = 400
    rpd            = 8000
    tpm            = 8000
    image_tokens   = 2048
  }
  validation {
    condition     = var.moderation.rpm >= 1 && var.moderation.rpm <= 500 && var.moderation.rpd >= 1 && var.moderation.rpd <= 10000 && var.moderation.tpm >= 1 && var.moderation.tpm <= 10000 && var.moderation.image_tokens >= 1 && var.moderation.image_tokens <= var.moderation.tpm
    error_message = "Moderation limits must fit the initial Tier 1 budget."
  }
}
