use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const PROFILE_MARKER_FILE: &str = ".kukuri-profile.json";
const PROFILE_MARKER_TEMP_FILE: &str = ".kukuri-profile.json.tmp";
const PROFILE_LOCK_FILE: &str = ".kukuri-profile.lock";
const PROFILE_MARKER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientProfileKind {
    Gui,
    Cli,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientProfile {
    pub name: String,
    pub app_data_dir: PathBuf,
    pub kind: ClientProfileKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileErrorKind {
    InvalidProfile,
    ProfileSelectorConflict,
    ProfileInUse,
    ProfileKindMismatch,
    LegacyProfileUnclassified,
    ProfilePathUnavailable,
}

impl ProfileErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidProfile => "invalid_profile",
            Self::ProfileSelectorConflict => "profile_selector_conflict",
            Self::ProfileInUse => "profile_in_use",
            Self::ProfileKindMismatch => "profile_kind_mismatch",
            Self::LegacyProfileUnclassified => "legacy_profile_unclassified",
            Self::ProfilePathUnavailable => "profile_path_unavailable",
        }
    }
}

#[derive(Debug)]
pub struct ProfileError {
    pub kind: ProfileErrorKind,
    pub message: String,
}

impl ProfileError {
    fn new(kind: ProfileErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProfileError {}

#[derive(Debug, Serialize, Deserialize)]
struct ProfileMarker {
    version: u32,
    kind: ClientProfileKind,
}

/// profile内の永続stateより先に取得し、所有期間中はfile handleを保持する。
#[derive(Debug)]
pub struct ProfileLease {
    profile: ClientProfile,
    _lock: File,
}

impl ProfileLease {
    pub fn acquire(profile: ClientProfile) -> Result<Self, ProfileError> {
        std::fs::create_dir_all(&profile.app_data_dir).map_err(|error| {
            ProfileError::new(
                ProfileErrorKind::ProfilePathUnavailable,
                format!(
                    "failed to create profile directory `{}`: {error}",
                    profile.app_data_dir.display()
                ),
            )
        })?;
        set_private_directory_permissions(&profile.app_data_dir)?;

        let lock_path = profile.app_data_dir.join(PROFILE_LOCK_FILE);
        let lock = open_private_lock_file(&lock_path)?;
        if let Err(error) = lock.try_lock() {
            let (kind, message) = match error {
                std::fs::TryLockError::WouldBlock => (
                    ProfileErrorKind::ProfileInUse,
                    format!(
                        "profile `{}` is already owned by another process",
                        profile.name
                    ),
                ),
                std::fs::TryLockError::Error(error) => (
                    ProfileErrorKind::ProfilePathUnavailable,
                    format!(
                        "failed to lock profile `{}` at `{}`: {error}",
                        profile.name,
                        lock_path.display()
                    ),
                ),
            };
            return Err(ProfileError::new(kind, message));
        }

        validate_or_create_marker(&profile)?;
        Ok(Self {
            profile,
            _lock: lock,
        })
    }

    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }
}

pub fn resolve_cli_profile(
    argument_profile: Option<&str>,
    environment_instance: Option<&str>,
    environment_app_data_dir: Option<&str>,
    xdg_data_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<ClientProfile, ProfileError> {
    let argument_profile = normalized_selector(argument_profile);
    let environment_instance = normalized_selector(environment_instance);
    let environment_app_data_dir = normalized_selector(environment_app_data_dir);

    if argument_profile.is_some()
        && environment_instance.is_some()
        && argument_profile != environment_instance
    {
        return Err(ProfileError::new(
            ProfileErrorKind::ProfileSelectorConflict,
            "--profile and KUKURI_INSTANCE select different profiles",
        ));
    }
    if environment_app_data_dir.is_some()
        && (argument_profile.is_some() || environment_instance.is_some())
    {
        return Err(ProfileError::new(
            ProfileErrorKind::ProfileSelectorConflict,
            "KUKURI_APP_DATA_DIR cannot be combined with --profile or KUKURI_INSTANCE",
        ));
    }

    let selected_name = argument_profile
        .or(environment_instance)
        .unwrap_or("default");
    validate_profile_name(selected_name)?;
    let (name, app_data_dir) = match environment_app_data_dir {
        Some(path) => (custom_profile_name(path), PathBuf::from(path)),
        None => {
            let base = match xdg_data_home {
                Some(path) => path.to_path_buf(),
                None => home
                    .map(|path| path.join(".local").join("share"))
                    .ok_or_else(|| {
                        ProfileError::new(
                            ProfileErrorKind::ProfilePathUnavailable,
                            "HOME is required when XDG_DATA_HOME is not set",
                        )
                    })?,
            };
            (
                selected_name.to_string(),
                base.join("kukuri")
                    .join("cli")
                    .join("profiles")
                    .join(selected_name),
            )
        }
    };

    Ok(ClientProfile {
        name,
        app_data_dir,
        kind: ClientProfileKind::Cli,
    })
}

pub fn gui_profile(name: impl Into<String>, app_data_dir: PathBuf) -> ClientProfile {
    ClientProfile {
        name: name.into(),
        app_data_dir,
        kind: ClientProfileKind::Gui,
    }
}

fn normalized_selector(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn custom_profile_name(path: &str) -> String {
    let digest = blake3::hash(path.as_bytes()).to_hex();
    format!("custom-{}", &digest[..32])
}

fn validate_profile_name(name: &str) -> Result<(), ProfileError> {
    let valid = !matches!(name, "." | "..")
        && !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(ProfileError::new(
            ProfileErrorKind::InvalidProfile,
            format!("invalid profile name `{name}`"),
        ))
    }
}

fn open_private_lock_file(path: &Path) -> Result<File, ProfileError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            ProfileError::new(
                ProfileErrorKind::ProfilePathUnavailable,
                format!("failed to open profile lock `{}`: {error}", path.display()),
            )
        })?;
    set_private_file_permissions(path)?;
    Ok(file)
}

fn validate_or_create_marker(profile: &ClientProfile) -> Result<(), ProfileError> {
    let marker_path = profile.app_data_dir.join(PROFILE_MARKER_FILE);
    if marker_path.exists() {
        let bytes = std::fs::read(&marker_path).map_err(|error| {
            ProfileError::new(
                ProfileErrorKind::ProfilePathUnavailable,
                format!(
                    "failed to read profile marker `{}`: {error}",
                    marker_path.display()
                ),
            )
        })?;
        let marker: ProfileMarker = serde_json::from_slice(&bytes).map_err(|error| {
            ProfileError::new(
                ProfileErrorKind::ProfileKindMismatch,
                format!(
                    "invalid profile marker `{}`: {error}",
                    marker_path.display()
                ),
            )
        })?;
        if marker.version != PROFILE_MARKER_VERSION || marker.kind != profile.kind {
            return Err(ProfileError::new(
                ProfileErrorKind::ProfileKindMismatch,
                format!(
                    "profile `{}` is marked for {:?}, not {:?}",
                    profile.name, marker.kind, profile.kind
                ),
            ));
        }
        return Ok(());
    }

    let has_existing_state = std::fs::read_dir(&profile.app_data_dir)
        .map_err(|error| {
            ProfileError::new(
                ProfileErrorKind::ProfilePathUnavailable,
                format!(
                    "failed to inspect profile directory `{}`: {error}",
                    profile.app_data_dir.display()
                ),
            )
        })?
        .filter_map(Result::ok)
        .any(|entry| {
            entry.file_name() != PROFILE_LOCK_FILE && entry.file_name() != PROFILE_MARKER_TEMP_FILE
        });
    if has_existing_state && profile.kind == ClientProfileKind::Cli {
        return Err(ProfileError::new(
            ProfileErrorKind::LegacyProfileUnclassified,
            format!(
                "unmarked existing profile `{}` cannot be adopted by the CLI",
                profile.app_data_dir.display()
            ),
        ));
    }

    let marker = ProfileMarker {
        version: PROFILE_MARKER_VERSION,
        kind: profile.kind,
    };
    let bytes = serde_json::to_vec_pretty(&marker).map_err(|error| {
        ProfileError::new(
            ProfileErrorKind::ProfilePathUnavailable,
            format!("failed to encode profile marker: {error}"),
        )
    })?;
    crate::storage::write_private_file_atomically(&marker_path, &bytes).map_err(|error| {
        ProfileError::new(
            ProfileErrorKind::ProfilePathUnavailable,
            format!(
                "failed to persist profile marker `{}`: {error}",
                marker_path.display()
            ),
        )
    })
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), ProfileError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        ProfileError::new(
            ProfileErrorKind::ProfilePathUnavailable,
            format!(
                "failed to set private permissions on `{}`: {error}",
                path.display()
            ),
        )
    })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), ProfileError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ProfileError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|error| {
        ProfileError::new(
            ProfileErrorKind::ProfilePathUnavailable,
            format!(
                "failed to set private permissions on profile directory `{}`: {error}",
                path.display()
            ),
        )
    })
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), ProfileError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_profile_uses_xdg_path_and_rejects_conflicting_selectors() {
        let root = Path::new("/data");
        let profile = resolve_cli_profile(Some("alpha"), Some("alpha"), None, Some(root), None)
            .expect("matching selectors");
        assert_eq!(profile.app_data_dir, root.join("kukuri/cli/profiles/alpha"));

        let error = resolve_cli_profile(Some("alpha"), Some("beta"), None, Some(root), None)
            .expect_err("conflicting selectors");
        assert_eq!(error.kind, ProfileErrorKind::ProfileSelectorConflict);

        let error = resolve_cli_profile(Some("alpha"), None, Some("/custom"), Some(root), None)
            .expect_err("explicit directory and selector conflict");
        assert_eq!(error.kind, ProfileErrorKind::ProfileSelectorConflict);
    }

    #[test]
    fn cli_profile_rejects_path_traversal() {
        let error = resolve_cli_profile(Some("../gui"), None, None, Some(Path::new("/data")), None)
            .expect_err("path traversal");
        assert_eq!(error.kind, ProfileErrorKind::InvalidProfile);
    }

    #[test]
    fn custom_app_data_directories_have_stable_distinct_profile_names() {
        let first = resolve_cli_profile(None, None, Some("/custom/first"), None, None)
            .expect("first custom profile");
        let first_again = resolve_cli_profile(None, None, Some("/custom/first"), None, None)
            .expect("same custom profile");
        let second = resolve_cli_profile(None, None, Some("/custom/second"), None, None)
            .expect("second custom profile");

        assert_eq!(first.name, first_again.name);
        assert_ne!(first.name, second.name);
        assert!(first.name.starts_with("custom-"));
        assert_eq!(first.app_data_dir, Path::new("/custom/first"));
        assert_eq!(second.app_data_dir, Path::new("/custom/second"));
    }

    #[test]
    fn same_profile_has_one_owner_and_different_profiles_can_coexist() {
        let root = tempfile::tempdir().expect("tempdir");
        let first_profile = ClientProfile {
            name: "first".to_string(),
            app_data_dir: root.path().join("first"),
            kind: ClientProfileKind::Cli,
        };
        let second_profile = ClientProfile {
            name: "second".to_string(),
            app_data_dir: root.path().join("second"),
            kind: ClientProfileKind::Cli,
        };
        let first = ProfileLease::acquire(first_profile.clone()).expect("first owner");
        let error = ProfileLease::acquire(first_profile).expect_err("duplicate owner");
        assert_eq!(error.kind, ProfileErrorKind::ProfileInUse);
        let second = ProfileLease::acquire(second_profile).expect("different profile owner");
        drop((first, second));
    }

    #[test]
    fn profile_kind_is_fail_closed_and_gui_can_adopt_legacy_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let legacy = root.path().join("legacy");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        std::fs::write(legacy.join("accounts.json"), b"legacy").expect("legacy state");

        let cli_error = ProfileLease::acquire(ClientProfile {
            name: "legacy".to_string(),
            app_data_dir: legacy.clone(),
            kind: ClientProfileKind::Cli,
        })
        .expect_err("CLI must not adopt an unmarked existing profile");
        assert_eq!(cli_error.kind, ProfileErrorKind::LegacyProfileUnclassified);

        let gui = ProfileLease::acquire(gui_profile("default", legacy.clone()))
            .expect("GUI adopts its legacy directory");
        drop(gui);
        let cli_error = ProfileLease::acquire(ClientProfile {
            name: "legacy".to_string(),
            app_data_dir: legacy,
            kind: ClientProfileKind::Cli,
        })
        .expect_err("marked GUI profile");
        assert_eq!(cli_error.kind, ProfileErrorKind::ProfileKindMismatch);
    }

    #[test]
    fn interrupted_marker_write_does_not_make_a_fresh_cli_profile_unusable() {
        let root = tempfile::tempdir().expect("tempdir");
        let profile_dir = root.path().join("interrupted");
        std::fs::create_dir_all(&profile_dir).expect("profile dir");
        std::fs::write(profile_dir.join(PROFILE_MARKER_TEMP_FILE), b"partial")
            .expect("partial marker");

        ProfileLease::acquire(ClientProfile {
            name: "interrupted".to_string(),
            app_data_dir: profile_dir,
            kind: ClientProfileKind::Cli,
        })
        .expect("recover marker write");
    }
}
