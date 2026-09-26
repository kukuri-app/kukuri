use crate::*;

const DEFAULT_CN_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
const DEFAULT_CN_RENDEZVOUS_REDIS_URL: &str = "redis://127.0.0.1:16379/";
const EXTERNAL_CN_BASE_URL_ENV: &str = "KUKURI_HARNESS_COMMUNITY_NODE_BASE_URL";
const EXTERNAL_CN_CONNECTIVITY_URLS_ENV: &str = "KUKURI_HARNESS_COMMUNITY_NODE_CONNECTIVITY_URLS";

pub(crate) struct ScenarioRuntime {
    pub(crate) db_path: PathBuf,
    pub(crate) network: FakeNetwork,
    pub(crate) app: Option<AppService>,
    pub(crate) current_topic: Option<String>,
    pub(crate) current_channel_id: Option<String>,
    pub(crate) private_channels: BTreeMap<String, String>,
    pub(crate) docs_sync: Arc<MemoryDocsSync>,
    pub(crate) blob_service: Arc<MemoryBlobService>,
    pub(crate) keys: KukuriKeys,
    pub(crate) private_channel_capabilities: Arc<StdMutex<Vec<PrivateChannelCapability>>>,
}

impl ScenarioRuntime {
    pub(crate) async fn launch(&mut self) -> Result<()> {
        let store = Arc::new(
            SqliteStore::connect_file(&self.db_path)
                .await
                .with_context(|| {
                    format!("failed to open scenario db {}", self.db_path.display())
                })?,
        );
        let transport = Arc::new(FakeTransport::new("desktop", self.network.clone()));
        let app = AppService::from_handles(ServiceHandles::new(
            store.clone(),
            store,
            transport.clone(),
            transport,
            self.docs_sync.clone(),
            self.blob_service.clone(),
            self.keys.clone(),
        ));
        let capabilities = self
            .private_channel_capabilities
            .lock()
            .map_err(|_| anyhow::anyhow!("private channel capability lock is poisoned"))?
            .clone();
        for capability in capabilities {
            app.restore_private_channel_capability(capability).await?;
        }
        let persisted_capabilities = self.private_channel_capabilities.clone();
        app.set_private_channel_capability_persist(Arc::new(move |capabilities| {
            *persisted_capabilities
                .lock()
                .map_err(|_| anyhow::anyhow!("private channel capability lock is poisoned"))? =
                capabilities.to_vec();
            Ok(())
        }));
        self.app = Some(app);
        Ok(())
    }

    pub(crate) fn app(&self) -> Result<&AppService> {
        self.app
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("desktop app is not running"))
    }

    /// bookmark の先頭ページ(#1221 R5-G: 全件の一覧 API は無い)。
    pub(crate) async fn bookmarks(&self) -> Result<Vec<kukuri_app_api::BookmarkedPostView>> {
        Ok(self
            .app()?
            .list_bookmarked_posts_page(None, false)
            .await?
            .items)
    }

    pub(crate) fn topic_or_default(&self, default_topic: &str) -> String {
        self.current_topic
            .clone()
            .unwrap_or_else(|| default_topic.to_string())
    }

    pub(crate) fn current_scope(&self) -> TimelineScope {
        match self.current_channel_id.as_ref() {
            Some(channel_id) => TimelineScope::Channel {
                channel_id: ChannelId::new(channel_id.clone()),
            },
            None => TimelineScope::Public,
        }
    }
}

pub(crate) struct CommunityNodeStack {
    pub(crate) external: bool,
    pub(crate) database: Option<TestDatabase>,
    pub(crate) user_api_task: Option<tokio::task::JoinHandle<()>>,
    pub(crate) _iroh_relay: Option<SpawnedIrohRelay>,
    pub(crate) base_url: String,
    pub(crate) expected_connectivity_urls: Option<Vec<String>>,
}

impl CommunityNodeStack {
    pub(crate) async fn spawn(prefix: &str) -> Result<Self> {
        if let Some(base_url) = external_community_node_base_url() {
            let expected_connectivity_urls = external_community_node_connectivity_urls();
            return Ok(Self {
                external: true,
                database: None,
                user_api_task: None,
                _iroh_relay: None,
                base_url,
                expected_connectivity_urls,
            });
        }

        let admin_database_url = community_node_admin_database_url();
        let database = TestDatabase::create(admin_database_url.as_str(), prefix).await?;
        let iroh_relay = kukuri_cn_iroh_relay::spawn_server(IrohRelayConfig {
            http_bind_addr: "127.0.0.1:0"
                .parse()
                .expect("valid loopback relay bind addr"),
            tls: None,
            client_rx_limit: None,
        })
        .await?;
        let iroh_relay_url = format!("http://{}", iroh_relay.http_addr());

        let user_api_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind community-node user-api listener")?;
        let user_api_addr = user_api_listener.local_addr()?;
        let base_url = format!("http://{user_api_addr}");

        // The consent-first connectivity scenario must exercise the same versioned legal
        // catalog as an operator-configured node. A missing config intentionally exposes no
        // policies, so give the in-process harness node the canonical sample config.
        let operator_config_dir = tempfile::tempdir()
            .context("failed to create community-node operator config directory")?;
        let operator_config_path = operator_config_dir.path().join("operator-config.yaml");
        std::fs::write(
            &operator_config_path,
            kukuri_cn_operator::SAMPLE_CONFIG.as_bytes(),
        )
        .context("failed to write community-node operator config")?;

        let user_api_state = build_user_api_state(&UserApiConfig {
            bind_addr: user_api_addr,
            database_url: database.database_url.clone(),
            rendezvous_redis_url: community_node_rendezvous_redis_url(),
            rendezvous_key_prefix: format!("cn:harness:{prefix}"),
            base_url: base_url.clone(),
            public_base_url: base_url.clone(),
            connectivity_urls: vec![iroh_relay_url.clone()],
            jwt_config: JwtConfig::new("kukuri-cn-harness", "test-secret", 3600),
            operator_config_path: Some(operator_config_path),
            channel_secret_key: None,
            legal_data_key: Some("harness-legal-data-key-0123456789abcdef".to_string()),
            index_query_enabled: false,
            trust_read_enabled: false,
            relation_distance_optout_min_proximity: None,
            deployment_revision: String::new(),
            readiness_activation_max_age_secs: 900,
            expected_issuer_node_id: None,
        })
        .await
        .context("failed to build community-node user-api state")?;
        let user_api_task = tokio::spawn(async move {
            axum::serve(
                user_api_listener,
                user_api_app_router(user_api_state)
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("community-node user-api server");
        });

        Ok(Self {
            external: false,
            database: Some(database),
            user_api_task: Some(user_api_task),
            _iroh_relay: Some(iroh_relay),
            base_url,
            expected_connectivity_urls: Some(vec![iroh_relay_url]),
        })
    }

    pub(crate) async fn shutdown(self) -> Result<()> {
        if let Some(user_api_task) = self.user_api_task {
            user_api_task.abort();
        }
        if let Some(database) = self.database {
            database.cleanup().await?;
        }
        Ok(())
    }
}

pub(crate) async fn shutdown_runtime(runtime: DesktopRuntime, label: &str) -> Result<()> {
    timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .with_context(|| format!("timed out waiting for {label}"))?;
    Ok(())
}

pub(crate) fn community_node_admin_database_url() -> String {
    std::env::var("COMMUNITY_NODE_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CN_ADMIN_DATABASE_URL.to_string())
}

pub(crate) fn community_node_rendezvous_redis_url() -> String {
    std::env::var("COMMUNITY_NODE_RENDEZVOUS_REDIS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CN_RENDEZVOUS_REDIS_URL.to_string())
}

fn external_community_node_base_url() -> Option<String> {
    std::env::var(EXTERNAL_CN_BASE_URL_ENV)
        .ok()
        .map(|value| normalize_url(value.as_str()))
        .filter(|value| !value.is_empty())
}

fn external_community_node_connectivity_urls() -> Option<Vec<String>> {
    let urls = std::env::var(EXTERNAL_CN_CONNECTIVITY_URLS_ENV)
        .ok()
        .map(|value| parse_connectivity_urls(value.as_str()))
        .unwrap_or_default();
    (!urls.is_empty()).then_some(urls)
}

fn parse_connectivity_urls(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(normalize_url)
        .filter(|url| !url.is_empty())
        .collect()
}

fn normalize_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

pub(crate) fn persist_runtime_identity(db_path: &Path, keys: &KukuriKeys) -> Result<()> {
    std::fs::write(
        db_path.with_extension("identity-key"),
        keys.export_secret_hex(),
    )
    .with_context(|| format!("failed to seed identity for {}", db_path.display()))
}

pub(crate) fn cleanup_runtime_artifacts(db_path: &Path) -> Result<()> {
    let config_paths = [
        db_path.to_path_buf(),
        db_path.with_extension("db-shm"),
        db_path.with_extension("db-wal"),
        db_path.with_extension("iroh-data"),
        db_path.with_extension("community-node.json"),
        db_path.with_extension("subscriptions.json"),
        db_path.with_extension("identity-store"),
        db_path.with_extension("identity-key"),
        db_path.with_extension("nsec"),
    ];
    for path in config_paths {
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove stale directory {}", path.display()))?;
        } else if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove stale file {}", path.display()))?;
        }
    }
    if let (Some(parent), Some(stem)) = (db_path.parent(), db_path.file_stem()) {
        let stem = stem.to_string_lossy();
        let optional_secret_prefixes = [
            format!("{stem}.private-channel-capabilities-"),
            format!("{stem}.community-node-token-"),
        ];
        for entry in std::fs::read_dir(parent)
            .with_context(|| format!("failed to read {}", parent.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if optional_secret_prefixes
                .iter()
                .any(|prefix| file_name.starts_with(prefix))
            {
                if path.is_dir() {
                    std::fs::remove_dir_all(&path).with_context(|| {
                        format!("failed to remove stale directory {}", path.display())
                    })?;
                } else if path.exists() {
                    std::fs::remove_file(&path).with_context(|| {
                        format!("failed to remove stale file {}", path.display())
                    })?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn remove_sqlite_runtime_db(db_path: &Path) -> Result<()> {
    for path in [
        db_path.to_path_buf(),
        db_path.with_extension("db-shm"),
        db_path.with_extension("db-wal"),
    ] {
        if !path.exists() {
            continue;
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove sqlite artifact {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connectivity_urls_normalizes_comma_separated_values() {
        assert_eq!(
            parse_connectivity_urls(" https://iroh-relay.kukuri.app/ , http://127.0.0.1:3340 "),
            vec![
                "https://iroh-relay.kukuri.app".to_string(),
                "http://127.0.0.1:3340".to_string(),
            ]
        );
    }

    #[test]
    fn parse_connectivity_urls_ignores_empty_values() {
        assert_eq!(
            parse_connectivity_urls(" , https://iroh-relay.kukuri.app///, "),
            vec!["https://iroh-relay.kukuri.app".to_string()]
        );
    }
}
