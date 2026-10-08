variable "name_prefix" {
  description = "リソース名の接頭辞。"
  type        = string
}

variable "region" {
  description = "static external IP を確保する GCP region。"
  type        = string
}

variable "create_network" {
  description = "true なら専用 VPC/subnet を作成する。false なら既存 network/subnet 名を参照する。"
  type        = bool
  default     = true
}

variable "network_name" {
  description = "create_network=false のときに参照する既存 VPC 名。"
  type        = string
  default     = ""
}

variable "subnet_name" {
  description = "create_network=false のときに参照する既存 subnet 名。"
  type        = string
  default     = ""
}

variable "subnet_cidr" {
  description = "create_network=true のときに作成する subnet の CIDR。"
  type        = string
  default     = "10.80.0.0/24"
}

variable "network_tags" {
  description = "firewall 規則を適用する VM の network tag 群。"
  type        = list(string)
  default     = ["kukuri-community-node"]
}

variable "enable_http" {
  description = "80/tcp（ACME HTTP-01 / redirect）を公開するか。"
  type        = bool
  default     = true
}

variable "enable_https" {
  description = "443/tcp（Caddy HTTPS: API + relay HTTP）を公開するか。"
  type        = bool
  default     = true
}

variable "relay_quic_port" {
  description = "cn-iroh-relay の QUIC/UDP ポート。"
  type        = number
  default     = 7842
}

variable "enable_iap_ssh" {
  description = "GCP IAP TCP forwarding 経由の 22/tcp を許可するか。"
  type        = bool
  default     = true
}

variable "iap_source_ranges" {
  description = "IAP TCP forwarding の送信元レンジ。既定は GCP IAP の固定レンジ。"
  type        = list(string)
  default     = ["35.235.240.0/20"]
}

variable "extra_ingress_source_ranges" {
  description = "API/relay/STUN の public ingress を絞りたい場合の許可レンジ。既定は全公開。"
  type        = list(string)
  default     = ["0.0.0.0/0"]
}

variable "enable_private_services_access" {
  description = "true なら Cloud SQL 等の managed service 向け private services access（VPC peering）を作成する。managed-db/ha で使用。"
  type        = bool
  default     = false
}

variable "private_services_prefix_length" {
  description = "private services access 用に確保する IP レンジの prefix 長。"
  type        = number
  default     = 16
}
variable "enable_public_blob_index" {
  description = "kukuri が運用する補助 index の 60125/udp を公開するか。"
  type        = bool
  default     = false
}
