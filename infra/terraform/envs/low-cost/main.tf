provider "google" {
  project = var.project_id
  region  = var.region
  zone    = var.zone
}

locals {
  # 同一 static IP を api/relay 2 hostname が指す。
  dns_records = {
    api = {
      name = "${var.api_domain}."
    }
    relay = {
      name = "${var.relay_domain}."
    }
  }

  # operator-config.yaml を VM 配置して public manifest endpoint を有効化する（#380）。
  # .tfvars では file()/path.module を使えないため、パスをここ（.tf）で解決して中身を module へ渡す。
  operator_config_file = trimspace(var.operator_config_path) != "" ? file("${path.module}/${var.operator_config_path}") : ""
}

module "network" {
  source = "../../modules/gcp-network"

  name_prefix                 = var.name_prefix
  region                      = var.region
  extra_ingress_source_ranges = var.extra_ingress_source_ranges
}

module "vm" {
  source = "../../modules/gcp-vm-compose"

  name_prefix        = var.name_prefix
  project_id         = var.project_id
  zone               = var.zone
  machine_type       = var.machine_type
  disk_size_gb       = var.disk_size_gb
  network_self_link  = module.network.network_self_link
  subnet_self_link   = module.network.subnet_self_link
  static_ip          = module.network.static_ip
  network_tags       = module.network.network_tags
  deployment_profile = var.deployment_profile

  api_domain   = var.api_domain
  relay_domain = var.relay_domain
  acme_email   = var.acme_email
  admin_actor  = var.admin_actor

  cn_user_api_image   = var.cn_user_api_image
  cn_iroh_relay_image = var.cn_iroh_relay_image
  cn_cli_image        = var.cn_cli_image

  # low-cost: local Postgres + Valkey containers
  deploy_local_postgres = true
  deploy_local_valkey   = true
  postgres_user         = var.postgres_user
  postgres_db           = var.postgres_db
  postgres_data_disk_gb = var.postgres_data_disk_gb

  jwt_secret_id               = var.jwt_secret_id
  postgres_password_secret_id = var.postgres_password_secret_id
  accessor_secret_ids = concat(
    compact([var.jwt_secret_id, var.postgres_password_secret_id, var.legal_data_key_secret_id]),
    # index / moderation stack の runtime secrets（#615）。stack 無効時は binding を作らない。
    var.deploy_indexer_stack ? compact([
      var.channel_secret_key_secret_id,
      var.arcadedb_password_secret_id,
      var.safety_signing_key_secret_id,
      var.arachnid_username_secret_id,
      var.arachnid_password_secret_id,
      var.vlm_api_key_secret_id,
    ]) : [],
  )

  # --- index / moderation stack (#615) ---
  deploy_indexer_stack                   = var.deploy_indexer_stack
  index_expected_topics                  = var.index_expected_topics
  cn_indexer_image                       = var.cn_indexer_image
  arcadedb_image                         = var.arcadedb_image
  indexer_data_disk_gb                   = var.indexer_data_disk_gb
  relation_analyze_interval_minutes      = var.relation_analyze_interval_minutes
  indexer_own_relay                      = var.indexer_own_relay
  indexer_external_relay_urls            = var.indexer_external_relay_urls
  indexer_retention_days                 = var.indexer_retention_days
  indexer_capacity_rows                  = var.indexer_capacity_rows
  index_query_enabled                    = var.index_query_enabled
  trust_read_enabled                     = var.trust_read_enabled
  relation_distance_optout_min_proximity = var.relation_distance_optout_min_proximity

  channel_secret_key_secret_id = var.channel_secret_key_secret_id
  legal_data_key_secret_id     = var.legal_data_key_secret_id
  arcadedb_password_secret_id  = var.arcadedb_password_secret_id
  safety_signing_key_secret_id = var.safety_signing_key_secret_id
  arachnid_username_secret_id  = var.arachnid_username_secret_id
  arachnid_password_secret_id  = var.arachnid_password_secret_id
  vlm_api_key_secret_id        = var.vlm_api_key_secret_id

  safety_provider_known_csam            = var.safety_provider_known_csam
  safety_provider_known_csam_required   = var.safety_provider_known_csam_required
  safety_provider_general               = var.safety_provider_general
  safety_provider_general_required      = var.safety_provider_general_required
  safety_provider_unknown_csam          = var.safety_provider_unknown_csam
  safety_provider_unknown_csam_required = var.safety_provider_unknown_csam_required
  safety_emit_signed_events             = var.safety_emit_signed_events
  safety_suspected_threshold            = var.safety_suspected_threshold
  safety_suspected_signal_visibility    = var.safety_suspected_signal_visibility
  safety_general_action                 = var.safety_general_action
  safety_operator_review                = var.safety_operator_review
  media_fetch_max_bytes                 = var.media_fetch_max_bytes
  media_fetch_timeout_secs              = var.media_fetch_timeout_secs

  vlm_api_base_url = var.vlm_api_base_url

  moderation           = var.moderation
  vlm_model            = var.vlm_model
  vlm_response_format  = var.vlm_response_format
  vlm_api_timeout_secs = var.vlm_api_timeout_secs

  rendezvous_key_prefix                 = var.rendezvous_key_prefix
  rate_limit_enabled                    = var.rate_limit_enabled
  rate_limit_per_second                 = var.rate_limit_per_second
  rate_limit_burst                      = var.rate_limit_burst
  iroh_relay_client_rx_bytes_per_second = var.iroh_relay_client_rx_bytes_per_second
  iroh_relay_client_rx_max_burst_bytes  = var.iroh_relay_client_rx_max_burst_bytes

  blob_cache_enabled   = var.blob_cache_enabled
  blob_cache_size_gb   = var.blob_cache_size_gb
  blob_cache_ttl_hours = var.blob_cache_ttl_hours
  blob_cache_path      = var.blob_cache_path

  backup_enabled = var.backup_enabled
  backup_bucket  = var.backup_enabled ? module.backup[0].bucket_name : ""

  # operator-config.yaml を VM 配置して public manifest endpoint を有効化する（#380）。
  operator_config_file = local.operator_config_file
}

module "backup" {
  source = "../../modules/gcp-low-cost-backup"
  count  = var.backup_enabled ? 1 : 0

  name_prefix           = var.name_prefix
  project_id            = var.project_id
  location              = var.region
  bucket_name           = var.backup_bucket_name
  retention_days        = var.backup_retention_days
  service_account_email = module.vm.service_account_email
  force_destroy         = var.backup_force_destroy
}

module "dns" {
  source = "../../modules/gcp-dns"

  manage_cloud_dns = var.manage_cloud_dns
  dns_zone_name    = var.dns_zone_name
  ip_address       = module.network.static_ip
  records          = local.dns_records
}

# 任意: VM boot disk の snapshot schedule。enable_disk_snapshots=true のとき
# resource policy を作成し、VM boot disk に attach する。
resource "google_compute_resource_policy" "disk_snapshot" {
  count   = var.enable_disk_snapshots ? 1 : 0
  name    = "${var.name_prefix}-disk-snapshot"
  project = var.project_id
  region  = var.region

  snapshot_schedule_policy {
    schedule {
      daily_schedule {
        days_in_cycle = 1
        start_time    = "18:00"
      }
    }
    retention_policy {
      max_retention_days    = var.snapshot_schedule_days
      on_source_disk_delete = "KEEP_AUTO_SNAPSHOTS"
    }
  }
}

resource "google_compute_disk_resource_policy_attachment" "boot" {
  count   = var.enable_disk_snapshots ? 1 : 0
  name    = google_compute_resource_policy.disk_snapshot[0].name
  disk    = module.vm.boot_disk_name
  zone    = var.zone
  project = var.project_id
}

resource "google_compute_disk_resource_policy_attachment" "postgres_data" {
  count   = var.enable_disk_snapshots && var.postgres_data_disk_gb > 0 ? 1 : 0
  name    = google_compute_resource_policy.disk_snapshot[0].name
  disk    = module.vm.postgres_data_disk_name
  zone    = var.zone
  project = var.project_id
}

resource "google_compute_disk_resource_policy_attachment" "indexer_data" {
  count   = var.enable_disk_snapshots && var.indexer_data_disk_gb > 0 ? 1 : 0
  name    = google_compute_resource_policy.disk_snapshot[0].name
  disk    = module.vm.indexer_data_disk_name
  zone    = var.zone
  project = var.project_id
}

locals {
  community_node_metric_descriptors = {
    disk_percent_used = {
      display_name = "Community Node disk used"
      unit         = "%"
    }
    postgres_healthy = {
      display_name = "Community Node Postgres healthy"
      unit         = "1"
    }
    arcadedb_healthy = {
      display_name = "Community Node ArcadeDB healthy"
      unit         = "1"
    }
    indexer_healthy = {
      display_name = "Community Node indexer healthy"
      unit         = "1"
    }
    indexer_last_ingest_age_seconds = {
      display_name = "Community Node last ingest age"
      unit         = "s"
    }
    indexer_backoff_active = {
      display_name = "Community Node indexer backoff active"
      unit         = "1"
    }
    provider_failure_detected = {
      display_name = "Community Node external safety provider failure detected"
      unit         = "1"
    }
    media_fetch_unavailable_total = {
      display_name = "Community Node media fetch unavailable total"
      unit         = "1"
    }
    body_fetch_failures_recent = {
      display_name = "Community Node post body fetch failures in last 10 minutes (-1 unknown)"
      unit         = "1"
    }
    index_expected_topics_present = {
      display_name = "Community Node expected public topics present"
      unit         = "1"
    }
    index_expected_topics_entries = {
      display_name = "Community Node expected public topics index entries (-1 unknown)"
      unit         = "1"
    }
    relation_last_success_age_seconds = {
      display_name = "Community Node relation analysis age"
      unit         = "s"
    }
  }

  community_node_alerts = merge(
    {
      disk = {
        metric     = "disk_percent_used"
        display    = "Community Node disk usage high"
        comparison = "COMPARISON_GT"
        threshold  = 85
      }
      postgres = {
        metric     = "postgres_healthy"
        display    = "Community Node Postgres unhealthy"
        comparison = "COMPARISON_LT"
        threshold  = 0.5
      }
    },
    var.deploy_indexer_stack ? {
      arcadedb = {
        metric     = "arcadedb_healthy"
        display    = "Community Node ArcadeDB unhealthy"
        comparison = "COMPARISON_LT"
        threshold  = 0.5
      }
      indexer = {
        metric     = "indexer_healthy"
        display    = "Community Node indexer unhealthy"
        comparison = "COMPARISON_LT"
        threshold  = 0.5
      }
      ingest_stale = {
        metric     = "indexer_last_ingest_age_seconds"
        display    = "Community Node ingest stale"
        comparison = "COMPARISON_GT"
        threshold  = 900
      }
      backoff = {
        metric     = "indexer_backoff_active"
        display    = "Community Node indexer backoff active"
        comparison = "COMPARISON_GT"
        threshold  = 0.5
      }
      provider = {
        metric     = "provider_failure_detected"
        display    = "Community Node external safety provider failure"
        comparison = "COMPARISON_GT"
        threshold  = 0.5
      }
      relation = {
        metric     = "relation_last_success_age_seconds"
        display    = "Community Node relation analysis stale"
        comparison = "COMPARISON_GT"
        threshold  = var.relation_analyze_interval_minutes * 180
      }
    } : {},
    var.deploy_indexer_stack && length(var.index_expected_topics) > 0 ? {
      index_topics_missing = {
        metric     = "index_expected_topics_present"
        display    = "Community Node expected index topics missing or unreadable"
        comparison = "COMPARISON_LT"
        threshold  = 0.5
      }
      index_empty = {
        metric     = "index_expected_topics_entries"
        display    = "Community Node expected public index empty or unreadable"
        comparison = "COMPARISON_LT"
        threshold  = 0.5
      }
    } : {}
  )
}

resource "google_monitoring_metric_descriptor" "community_node" {
  for_each = var.enable_community_node_monitoring ? local.community_node_metric_descriptors : {}

  project      = var.project_id
  type         = "custom.googleapis.com/kukuri/community_node/${each.key}"
  metric_kind  = "GAUGE"
  value_type   = "DOUBLE"
  display_name = each.value.display_name
  unit         = each.value.unit
}

resource "google_monitoring_alert_policy" "community_node" {
  for_each = var.enable_community_node_monitoring ? local.community_node_alerts : {}

  project               = var.project_id
  display_name          = each.value.display
  combiner              = "OR"
  enabled               = true
  notification_channels = var.monitoring_notification_channels

  conditions {
    display_name = each.value.display
    condition_threshold {
      filter                  = "resource.type = \"gce_instance\" AND metric.type = \"custom.googleapis.com/kukuri/community_node/${each.value.metric}\""
      comparison              = each.value.comparison
      threshold_value         = each.value.threshold
      duration                = "300s"
      evaluation_missing_data = "EVALUATION_MISSING_DATA_ACTIVE"
    }
  }

  documentation {
    mime_type = "text/markdown"
    content   = "Inspect `kukuri-monitor.service`, container health, and the Community Node operator runbook before recovery or rollback."
  }

  depends_on = [google_monitoring_metric_descriptor.community_node]
}
