terraform {
  required_providers {
    google = {
      source = "hashicorp/google"
    }
  }
}

locals {
  install_dir = "/var/lib/kukuri/community-node"
  certs_dir   = "/var/lib/kukuri/certs" # host path（certbot が書き込む）
  certs_mount = "/certs"                # container 内のマウント先
  # api/relay を 1 つの SAN 証明書 lineage にまとめる。初回発行と更新で同じ lineage を
  # 使うことで、relay 用 PEM が更新されず期限切れになる事故を防ぐ。
  cert_name = var.api_domain

  # ポートは single source of truth として local に集約し、全 template へ渡す。
  # api/relay-http は内部固定ポート、relay-quic の内部 bind も固定 7842。
  api_port                 = 8080
  admin_port               = 9090
  relay_http_port          = 3340
  relay_quic_internal_port = 7842

  # Postgres data は boot disk 上の docker named volume ではなく、別 PD に置けるようにする
  # （VM 置換で boot disk が消えても data を残すため）。
  postgres_data_path  = "/var/lib/kukuri/postgres"
  use_postgres_disk   = var.deploy_local_postgres && var.postgres_data_disk_gb > 0
  use_blob_cache_disk = var.blob_cache_enabled && var.blob_cache_size_gb > 0

  # index / moderation stack（#615）。indexer data（iroh endpoint 同一性 / docs replica）は
  # 別 PD に置けるようにする。ArcadeDB data は rebuildable projection（canonical store では
  # ない）ため docker named volume に置き、空からの再構築手順を runbook に持つ。
  indexer_data_path = "/var/lib/kukuri/cn-indexer"
  # Disk ownership is independent from whether the indexer containers are
  # currently enabled. A feature-flag rollback must not plan data deletion.
  use_indexer_disk    = var.indexer_data_disk_gb > 0
  indexer_status_port = 8630

  # operator-config.yaml を VM に配置するか（#380）。空文字なら配置せず manifest endpoint は 404。
  operator_config_enabled = trimspace(var.operator_config_file) != ""
  operator_config_path    = "/etc/kukuri/operator-config.yaml"
  operator_config_b64     = local.operator_config_enabled ? base64encode(var.operator_config_file) : ""
  deployment_revision = sha256(jsonencode({
    cn_user_api_image                      = var.cn_user_api_image
    cn_indexer_image                       = var.cn_indexer_image
    cn_cli_image                           = var.cn_cli_image
    operator_config_sha256                 = sha256(var.operator_config_file)
    index_query_enabled                    = var.index_query_enabled
    trust_read_enabled                     = var.trust_read_enabled
    relation_distance_optout_min_proximity = var.relation_distance_optout_min_proximity
    safety_provider_known_csam             = var.safety_provider_known_csam
    safety_provider_general                = var.safety_provider_general
    safety_provider_unknown_csam           = var.safety_provider_unknown_csam
    vlm_api_base_url                       = var.vlm_api_base_url
    moderation                             = var.moderation
    vlm_model                              = var.vlm_model
    vlm_response_format                    = var.vlm_response_format
    safety_emit_signed_events              = var.safety_emit_signed_events
    safety_suspected_threshold             = var.safety_suspected_threshold
    safety_suspected_signal_visibility     = var.safety_suspected_signal_visibility
    safety_general_action                  = var.safety_general_action
    safety_operator_review                 = var.safety_operator_review
    media_fetch_max_bytes                  = var.media_fetch_max_bytes
    media_fetch_timeout_secs               = var.media_fetch_timeout_secs
  }))

  # 各テンプレートを base64 で metadata に渡し、startup script が展開する。
  compose_b64 = base64encode(replace(templatefile("${path.module}/templates/docker-compose.yml.tftpl", {
    deploy_local_postgres    = var.deploy_local_postgres
    deploy_local_valkey      = var.deploy_local_valkey
    postgres_image           = var.postgres_image
    valkey_image             = var.valkey_image
    cn_user_api_image        = var.cn_user_api_image
    cn_iroh_relay_image      = var.cn_iroh_relay_image
    cn_cli_image             = var.cn_cli_image
    caddy_image              = var.caddy_image
    postgres_user            = var.postgres_user
    postgres_db              = var.postgres_db
    relay_quic_port          = var.relay_quic_port
    relay_quic_internal_port = local.relay_quic_internal_port
    api_port                 = local.api_port
    admin_port               = local.admin_port
    admin_actor              = trimspace(var.admin_actor)
    project_id               = var.project_id
    relay_http_port          = local.relay_http_port
    certs_mount              = local.certs_mount
    certs_dir                = local.certs_dir
    use_postgres_disk        = local.use_postgres_disk
    postgres_data_path       = local.postgres_data_path
    blob_cache_enabled       = var.blob_cache_enabled
    blob_cache_path          = var.blob_cache_path
    operator_config_enabled  = local.operator_config_enabled
    operator_config_path     = local.operator_config_path
    legal_data_key_secret_id = var.legal_data_key_secret_id
    deploy_indexer_stack     = var.deploy_indexer_stack
    cn_indexer_image         = var.cn_indexer_image
    arcadedb_image           = var.arcadedb_image
    use_indexer_disk         = local.use_indexer_disk
    indexer_data_path        = local.indexer_data_path
    indexer_status_port      = local.indexer_status_port
  }), "\r\n", "\n"))

  caddyfile_b64 = base64encode(replace(templatefile("${path.module}/templates/Caddyfile.tftpl", {
    api_domain      = var.api_domain
    relay_domain    = var.relay_domain
    certs_mount     = local.certs_mount
    cert_name       = local.cert_name
    api_port        = local.api_port
    relay_http_port = local.relay_http_port
  }), "\r\n", "\n"))

  env_runtime_b64 = base64encode(replace(templatefile("${path.module}/templates/community-node.env.tftpl", {
    rendezvous_key_prefix                  = var.rendezvous_key_prefix
    api_domain                             = var.api_domain
    relay_domain                           = var.relay_domain
    jwt_issuer                             = var.jwt_issuer
    jwt_ttl_seconds                        = var.jwt_ttl_seconds
    rate_limit_enabled                     = var.rate_limit_enabled
    rate_limit_per_second                  = var.rate_limit_per_second
    rate_limit_burst                       = var.rate_limit_burst
    api_port                               = local.api_port
    relay_http_port                        = local.relay_http_port
    relay_quic_internal_port               = local.relay_quic_internal_port
    iroh_relay_client_rx_bytes_per_second  = var.iroh_relay_client_rx_bytes_per_second
    iroh_relay_client_rx_max_burst_bytes   = var.iroh_relay_client_rx_max_burst_bytes
    blob_cache_enabled                     = var.blob_cache_enabled
    blob_cache_ttl_hours                   = var.blob_cache_ttl_hours
    blob_cache_path                        = var.blob_cache_path
    certs_mount                            = local.certs_mount
    cert_name                              = local.cert_name
    deploy_indexer_stack                   = var.deploy_indexer_stack
    indexer_data_path                      = local.indexer_data_path
    indexer_status_port                    = local.indexer_status_port
    indexer_own_relay                      = var.indexer_own_relay
    indexer_external_relay_urls            = join(",", var.indexer_external_relay_urls)
    indexer_retention_days                 = var.indexer_retention_days
    indexer_capacity_rows                  = var.indexer_capacity_rows
    index_query_enabled                    = var.index_query_enabled
    trust_read_enabled                     = var.trust_read_enabled
    relation_distance_optout_min_proximity = var.relation_distance_optout_min_proximity
    trust_w_abs_negative                   = var.trust_w_abs_negative
    trust_w_abs_positive                   = var.trust_w_abs_positive
    trust_relative_half_life_days          = var.trust_relative_half_life_days
    safety_provider_known_csam             = var.safety_provider_known_csam
    safety_provider_known_csam_required    = var.safety_provider_known_csam_required
    safety_provider_general                = var.safety_provider_general
    safety_provider_general_required       = var.safety_provider_general_required
    safety_provider_unknown_csam           = var.safety_provider_unknown_csam
    safety_provider_unknown_csam_required  = var.safety_provider_unknown_csam_required
    safety_emit_signed_events              = var.safety_emit_signed_events
    safety_suspected_threshold             = var.safety_suspected_threshold
    safety_suspected_signal_visibility     = var.safety_suspected_signal_visibility
    safety_general_action                  = var.safety_general_action
    safety_operator_review                 = var.safety_operator_review
    media_fetch_max_bytes                  = var.media_fetch_max_bytes
    media_fetch_timeout_secs               = var.media_fetch_timeout_secs
    vlm_api_base_url                       = var.vlm_api_base_url
    moderation                             = var.moderation
    vlm_model                              = var.vlm_model
    vlm_response_format                    = var.vlm_response_format
    vlm_api_timeout_secs                   = var.vlm_api_timeout_secs
    deployment_revision                    = local.deployment_revision
  }), "\r\n", "\n"))

  backup_script_b64 = base64encode(replace(templatefile("${path.module}/templates/backup.sh.tftpl", {
    install_dir   = local.install_dir
    backup_bucket = var.backup_bucket
    postgres_user = var.postgres_user
    postgres_db   = var.postgres_db
  }), "\r\n", "\n"))

  renew_script_b64 = base64encode(replace(templatefile("${path.module}/templates/renew-certs.sh.tftpl", {
    install_dir  = local.install_dir
    certs_dir    = local.certs_dir
    acme_image   = var.acme_image
    api_domain   = var.api_domain
    relay_domain = var.relay_domain
    acme_email   = var.acme_email
    cert_name    = local.cert_name
  }), "\r\n", "\n"))

  monitor_script_b64 = base64encode(replace(templatefile("${path.module}/templates/monitor.sh.tftpl", {
    index_health_helpers  = file("${path.module}/scripts/index-health.sh")
    relation_age_helpers  = file("${path.module}/scripts/relation-age.sh")
    index_expected_topics = sort(tolist(var.index_expected_topics))
    install_dir           = local.install_dir
    postgres_data_path    = local.postgres_data_path
    indexer_data_path     = local.indexer_data_path
    deploy_indexer_stack  = var.deploy_indexer_stack
    indexer_status_port   = local.indexer_status_port
  }), "\r\n", "\n"))

  startup_script = replace(templatefile("${path.module}/templates/startup.sh.tftpl", {
    install_dir           = local.install_dir
    certs_dir             = local.certs_dir
    acme_image            = var.acme_image
    api_domain            = var.api_domain
    relay_domain          = var.relay_domain
    acme_email            = var.acme_email
    deploy_local_postgres = var.deploy_local_postgres
    deploy_local_valkey   = var.deploy_local_valkey
    postgres_user         = var.postgres_user
    postgres_db           = var.postgres_db
    cert_name             = local.cert_name
    use_postgres_disk     = local.use_postgres_disk
    postgres_data_path    = local.postgres_data_path
    use_blob_cache_disk   = local.use_blob_cache_disk
    # 外部 DB（managed-db/ha）は password を metadata に焼かず、boot 時に Secret Manager から取得する。
    external_db_host                    = var.external_db_host
    external_db_port                    = var.external_db_port
    external_db_user                    = var.external_db_user
    external_db_name                    = var.external_db_name
    external_db_password_secret_id      = var.external_db_password_secret_id
    external_db_password_secret_version = var.external_db_password_secret_version
    external_redis_url                  = var.external_redis_url
    postgres_password_secret_id         = var.postgres_password_secret_id
    postgres_password_secret_version    = var.postgres_password_secret_version
    jwt_secret_id                       = var.jwt_secret_id
    jwt_secret_version                  = var.jwt_secret_version
    blob_cache_enabled                  = var.blob_cache_enabled
    blob_cache_path                     = var.blob_cache_path
    backup_enabled                      = var.backup_enabled
    backup_schedule_oncalendar          = var.backup_schedule_oncalendar
    deploy_indexer_stack                = var.deploy_indexer_stack
    use_indexer_disk                    = local.use_indexer_disk
    indexer_data_path                   = local.indexer_data_path
    relation_analyze_interval_minutes   = var.relation_analyze_interval_minutes
    channel_secret_key_secret_id        = var.channel_secret_key_secret_id
    legal_data_key_secret_id            = var.legal_data_key_secret_id
    arcadedb_password_secret_id         = var.arcadedb_password_secret_id
    safety_signing_key_secret_id        = var.safety_signing_key_secret_id
    arachnid_username_secret_id         = var.arachnid_username_secret_id
    arachnid_password_secret_id         = var.arachnid_password_secret_id
    vlm_api_key_secret_id               = var.vlm_api_key_secret_id
    operator_config_enabled             = local.operator_config_enabled
    operator_config_path                = local.operator_config_path
    operator_config_b64                 = local.operator_config_b64
    compose_b64                         = local.compose_b64
    caddyfile_b64                       = local.caddyfile_b64
    env_runtime_b64                     = local.env_runtime_b64
    backup_script_b64                   = local.backup_script_b64
    renew_script_b64                    = local.renew_script_b64
    monitor_script_b64                  = local.monitor_script_b64
  }), "\r\n", "\n")
}

resource "google_service_account" "vm" {
  account_id   = "${var.name_prefix}-vm"
  display_name = "kukuri community node VM (${var.deployment_profile})"
  project      = var.project_id
}

# Secret Manager 取得 / GCS backup を VM が行うため最小権限を付与する。
resource "google_project_iam_member" "logging" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.vm.email}"
}

# secret accessor binding を VM 起動前に確実に作るため module 内に置き、
# instance が depends_on する。
resource "google_secret_manager_secret_iam_member" "accessor" {
  for_each = toset(var.accessor_secret_ids)

  project   = var.project_id
  secret_id = each.value
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.vm.email}"
}

resource "google_compute_disk" "blob_cache" {
  count = var.blob_cache_enabled && var.blob_cache_size_gb > 0 ? 1 : 0

  name    = "${var.name_prefix}-blob-cache"
  type    = "pd-standard"
  zone    = var.zone
  size    = var.blob_cache_size_gb
  project = var.project_id
}

# Postgres data 専用 PD。VM が置換されても data を残すため auto_delete=false で attach し、
# 誤削除を防ぐため prevent_destroy する。
resource "google_compute_disk" "postgres_data" {
  count = local.use_postgres_disk ? 1 : 0

  name    = "${var.name_prefix}-postgres-data"
  type    = "pd-ssd"
  zone    = var.zone
  size    = var.postgres_data_disk_gb
  project = var.project_id

  lifecycle {
    prevent_destroy = true
  }
}

# cn-indexer data 専用 PD（#615）。iroh endpoint 同一性 / docs replica を VM 置換で失わない
# ためのもの。ArcadeDB（rebuildable projection）や raw media は置かない。再同期で復元可能な
# ため postgres_data と違い prevent_destroy はしない。
resource "google_compute_disk" "indexer_data" {
  count = local.use_indexer_disk ? 1 : 0

  name    = "${var.name_prefix}-indexer-data"
  type    = "pd-ssd"
  zone    = var.zone
  size    = var.indexer_data_disk_gb
  project = var.project_id

  lifecycle {
    prevent_destroy = true
  }
}

resource "google_project_iam_member" "monitoring" {
  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.vm.email}"
}

resource "google_compute_instance" "vm" {
  name         = "${var.name_prefix}-vm"
  project      = var.project_id
  zone         = var.zone
  machine_type = var.machine_type
  tags         = var.network_tags

  boot_disk {
    initialize_params {
      image = var.boot_image
      size  = var.disk_size_gb
    }
  }

  dynamic "attached_disk" {
    for_each = local.use_blob_cache_disk ? [1] : []
    content {
      source      = google_compute_disk.blob_cache[0].self_link
      device_name = "blob-cache"
    }
  }

  dynamic "attached_disk" {
    for_each = local.use_postgres_disk ? [1] : []
    content {
      source      = google_compute_disk.postgres_data[0].self_link
      device_name = "postgres-data"
    }
  }

  dynamic "attached_disk" {
    for_each = local.use_indexer_disk ? [1] : []
    content {
      source      = google_compute_disk.indexer_data[0].self_link
      device_name = "indexer-data"
    }
  }

  network_interface {
    network    = var.network_self_link
    subnetwork = var.subnet_self_link

    access_config {
      nat_ip = var.static_ip
    }
  }

  service_account {
    email  = google_service_account.vm.email
    scopes = ["cloud-platform"]
  }

  metadata = {
    enable-oslogin = "TRUE"
  }

  metadata_startup_script = local.startup_script

  allow_stopping_for_update = true

  lifecycle {
    precondition {
      condition = !local.operator_config_enabled || !var.deploy_indexer_stack || (
        strcontains(base64decode(local.compose_b64), "./operator-config.yaml:${local.operator_config_path}:ro") &&
        strcontains(base64decode(local.compose_b64), "[\"readiness\", \"--config\", \"${local.operator_config_path}\"]")
      )
      error_message = "cn-readiness must mount the generated operator-config.yaml file at the configured container path."
    }
  }

  # secret accessor binding と logging 権限が VM 起動前に存在することを保証する。
  depends_on = [
    google_secret_manager_secret_iam_member.accessor,
    google_project_iam_member.logging,
    google_project_iam_member.monitoring,
  ]
}
