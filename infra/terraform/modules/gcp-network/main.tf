terraform {
  required_providers {
    google = {
      source = "hashicorp/google"
    }
  }
}

locals {
  network_self_link = var.create_network ? google_compute_network.this[0].self_link : data.google_compute_network.existing[0].self_link
  network_name      = var.create_network ? google_compute_network.this[0].name : var.network_name
}

resource "google_compute_network" "this" {
  count                   = var.create_network ? 1 : 0
  name                    = "${var.name_prefix}-net"
  auto_create_subnetworks = false
}

resource "google_compute_subnetwork" "this" {
  count         = var.create_network ? 1 : 0
  name          = "${var.name_prefix}-subnet"
  ip_cidr_range = var.subnet_cidr
  region        = var.region
  network       = google_compute_network.this[0].id
}

data "google_compute_network" "existing" {
  count = var.create_network ? 0 : 1
  name  = var.network_name
}

data "google_compute_subnetwork" "existing" {
  count  = var.create_network ? 0 : 1
  name   = var.subnet_name
  region = var.region
}

resource "google_compute_address" "this" {
  name   = "${var.name_prefix}-ip"
  region = var.region
}

resource "google_compute_firewall" "http" {
  count   = var.enable_http ? 1 : 0
  name    = "${var.name_prefix}-allow-http"
  network = local.network_name

  allow {
    protocol = "tcp"
    ports    = ["80"]
  }

  source_ranges = var.extra_ingress_source_ranges
  target_tags   = var.network_tags
}

resource "google_compute_firewall" "https" {
  count   = var.enable_https ? 1 : 0
  name    = "${var.name_prefix}-allow-https"
  network = local.network_name

  allow {
    protocol = "tcp"
    ports    = ["443"]
  }

  source_ranges = var.extra_ingress_source_ranges
  target_tags   = var.network_tags
}

resource "google_compute_firewall" "relay_quic" {
  name    = "${var.name_prefix}-allow-relay-quic"
  network = local.network_name

  allow {
    protocol = "udp"
    ports    = [tostring(var.relay_quic_port)]
  }

  source_ranges = var.extra_ingress_source_ranges
  target_tags   = var.network_tags
}

# STUN（#1483、ADR 0057 §6）。relay と同じ host で cn-stun が 3478/udp を待ち受ける。
resource "google_compute_firewall" "stun" {
  name    = "${var.name_prefix}-allow-stun"
  network = local.network_name

  allow {
    protocol = "udp"
    ports    = ["3478"]
  }

  source_ranges = var.extra_ingress_source_ranges
  target_tags   = var.network_tags
}

resource "google_compute_firewall" "public_blob_index" {
  count   = var.enable_public_blob_index ? 1 : 0
  name    = "${var.name_prefix}-allow-public-blob-index"
  network = local.network_name
  allow {
    protocol = "udp"
    ports    = ["60125"]
  }
  source_ranges = var.extra_ingress_source_ranges
  target_tags   = var.network_tags
}

resource "google_compute_firewall" "iap_ssh" {
  count   = var.enable_iap_ssh ? 1 : 0
  name    = "${var.name_prefix}-allow-iap-ssh"
  network = local.network_name

  allow {
    protocol = "tcp"
    # 9090 is the cn-user-api operator admin listener. It is not routed by Caddy and
    # is reachable only through IAP TCP forwarding with IAM authorization.
    ports = ["22", "9090"]
  }

  source_ranges = var.iap_source_ranges
  target_tags   = var.network_tags
}

# Cloud SQL などの managed service へ private IP で接続するための
# private services access（VPC peering）。managed-db / ha で有効化する。
resource "google_compute_global_address" "private_services" {
  count         = var.enable_private_services_access ? 1 : 0
  name          = "${var.name_prefix}-psa"
  purpose       = "VPC_PEERING"
  address_type  = "INTERNAL"
  prefix_length = var.private_services_prefix_length
  network       = local.network_self_link
}

resource "google_service_networking_connection" "private_services" {
  count                   = var.enable_private_services_access ? 1 : 0
  network                 = local.network_self_link
  service                 = "servicenetworking.googleapis.com"
  reserved_peering_ranges = [google_compute_global_address.private_services[0].name]
}
