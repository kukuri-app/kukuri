use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) const DB_FILE_NAME: &str = "kukuri.db";
pub(crate) const DISCOVERY_CONFIG_FILE_EXTENSION: &str = "discovery.json";
pub(crate) const COMMUNITY_NODE_CONFIG_FILE_EXTENSION: &str = "community-node.json";

/// 同意記録などに残す build の種別。debug assertions の有無で判定する。
/// `tauri dev` や `cargo build` は `Development`、配布用の release build は `Release` になる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppBuildProfile {
    Release,
    Development,
}

impl AppBuildProfile {
    pub const fn current() -> Self {
        if cfg!(debug_assertions) {
            Self::Development
        } else {
            Self::Release
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Development => "development",
        }
    }
}

const DEVELOPMENT_APP_DATA_DIR_SUFFIX: &str = ".dev";

/// OS が返す app data dir から、build の種別ごとの既定 dir を決める(#1105)。
/// 開発ビルドは配布版 dir の兄弟 `<identifier>.dev` を使い、配布版の同意記録・
/// アカウント・DB を読み書きしない。配布版の dir は変えない。
pub fn default_app_data_dir(platform_app_data_dir: &Path, profile: AppBuildProfile) -> PathBuf {
    match profile {
        AppBuildProfile::Release => platform_app_data_dir.to_path_buf(),
        AppBuildProfile::Development => {
            let mut name = platform_app_data_dir
                .file_name()
                .map(OsString::from)
                .unwrap_or_else(|| OsString::from("kukuri"));
            name.push(DEVELOPMENT_APP_DATA_DIR_SUFFIX);
            platform_app_data_dir.with_file_name(name)
        }
    }
}

/// 同じ PC で複数起動するときの instance の名前(`KUKURI_INSTANCE`、空なら無し)。
pub fn desktop_instance_from_env() -> Option<String> {
    std::env::var("KUKURI_INSTANCE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn resolve_app_data_dir_from_env(base_app_data_dir: &Path) -> Result<PathBuf> {
    let mut app_data_dir = std::env::var("KUKURI_APP_DATA_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| base_app_data_dir.to_path_buf());

    if app_data_dir == base_app_data_dir
        && let Some(instance) = desktop_instance_from_env()
    {
        app_data_dir = app_data_dir.join(instance);
    }

    fs::create_dir_all(&app_data_dir)
        .with_context(|| format!("failed to create app data dir `{}`", app_data_dir.display()))?;

    Ok(app_data_dir)
}

pub fn resolve_db_path_from_env(base_app_data_dir: &Path) -> Result<PathBuf> {
    Ok(resolve_app_data_dir_from_env(base_app_data_dir)?.join(DB_FILE_NAME))
}
pub(crate) fn discovery_config_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(DISCOVERY_CONFIG_FILE_EXTENSION)
}

pub(crate) fn community_node_config_path(db_path: &Path) -> PathBuf {
    db_path.with_extension(COMMUNITY_NODE_CONFIG_FILE_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_build_keeps_platform_app_data_dir() {
        let platform = Path::new("/data").join("app.kukuri.desktop");
        assert_eq!(
            default_app_data_dir(&platform, AppBuildProfile::Release),
            platform
        );
    }

    #[test]
    fn development_build_uses_sibling_app_data_dir() {
        let platform = Path::new("/data").join("app.kukuri.desktop");
        let development = default_app_data_dir(&platform, AppBuildProfile::Development);
        assert_eq!(
            development,
            Path::new("/data").join("app.kukuri.desktop.dev")
        );
        // 配布版 dir の配下に置かない(端末 backup などの対象に混ざらない)。
        assert!(!development.starts_with(&platform));
    }

    #[test]
    fn test_build_is_development_profile() {
        assert_eq!(AppBuildProfile::current(), AppBuildProfile::Development);
        assert_eq!(AppBuildProfile::current().as_str(), "development");
        assert_eq!(AppBuildProfile::Release.as_str(), "release");
    }
}
