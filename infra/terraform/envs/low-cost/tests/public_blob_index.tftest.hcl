mock_provider "google" {}

variables {
  project_id                       = "test-project"
  api_domain                       = "api.example.com"
  relay_domain                     = "relay.example.com"
  acme_email                       = "ops@example.com"
  cn_user_api_image                = "example/user-api:fixed"
  cn_iroh_relay_image              = "example/relay:fixed"
  cn_cli_image                     = "example/cli:fixed"
  jwt_secret_id                    = "test-jwt-secret"
  postgres_password_secret_id      = "test-postgres-secret"
  backup_enabled                   = false
  operator_config_path             = ""
  public_blob_index_list_secret_id = ""
}

run "generic_node_has_no_public_blob_index" {
  command = plan
  assert {
    condition = !contains(keys(yamldecode(base64decode(regex(
      "echo \"([A-Za-z0-9+/=]+)\" [|] base64 -d > \"[$]INSTALL_DIR/docker-compose[.]yml\"",
      nonsensitive(module.vm.startup_script)
    )[0])).services), "cn-addr-index")
    error_message = "Generic nodes must not run kukuri's address index or list publisher."
  }
}

run "kukuri_index_uses_direct_udp_and_the_signed_list" {
  command = apply
  variables {
    public_blob_index_list_secret_id = "test-index-list-key"
  }
  assert {
    condition = yamldecode(base64decode(regex(
      "echo \"([A-Za-z0-9+/=]+)\" [|] base64 -d > \"[$]INSTALL_DIR/docker-compose[.]yml\"",
      nonsensitive(module.vm.startup_script)
    )[0])).services.cn-addr-index.network_mode == "host"
    error_message = "The address index must preserve the Mainline UDP source address."
  }
  assert {
    condition = yamldecode(base64decode(regex(
      "echo \"([A-Za-z0-9+/=]+)\" [|] base64 -d > \"[$]INSTALL_DIR/docker-compose[.]yml\"",
      nonsensitive(module.vm.startup_script)
    )[0])).services.cn-index-list.entrypoint[0] == "/usr/local/bin/iroh-index-list"
    error_message = "The list must be renewed by the pinned upstream publisher."
  }
  assert {
    condition     = strcontains(nonsensitive(module.vm.startup_script), "INDEX_LIST_SECRET=\"$(fetch_secret \"test-index-list-key\" \"latest\")\"")
    error_message = "The signing secret must be fetched at startup, not stored in Terraform metadata."
  }
  assert {
    condition     = strcontains(nonsensitive(module.vm.startup_script), "iptables -I INPUT -p udp --dport 60125 -j ACCEPT")
    error_message = "COS must admit direct UDP to the host-network index."
  }
}
