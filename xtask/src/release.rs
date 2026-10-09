use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[allow(unused_imports)]
use crate::*;

pub(crate) fn release_check(tag: Option<&str>) -> Result<()> {
    let root = root_dir();
    asset_check()?;
    let workspace_version = read_workspace_version(&root.join("Cargo.toml"))?;
    let tauri_version = read_package_version(&desktop_dir().join("src-tauri").join("Cargo.toml"))?;
    let desktop_package_version = read_json_version(&desktop_dir().join("package.json"))
        .context("desktop package version")?;
    let tauri_config_version =
        read_json_version(&desktop_dir().join("src-tauri").join("tauri.conf.json"))
            .context("tauri config version")?;

    for (label, version) in [
        ("apps/desktop/src-tauri/Cargo.toml", tauri_version.as_str()),
        (
            "apps/desktop/package.json",
            desktop_package_version.as_str(),
        ),
        (
            "apps/desktop/src-tauri/tauri.conf.json",
            tauri_config_version.as_str(),
        ),
    ] {
        if version != workspace_version {
            bail!(
                "release version mismatch: workspace version is {workspace_version}, {label} has {version}"
            );
        }
    }

    let mut checked_tag = "<not checked>".to_string();
    if let Some(tag) = tag {
        let preview = validate_release_tag(&workspace_version, tag)?;
        let version_code = android_version_code(&workspace_version, Some(preview))?;
        checked_tag = format!("{tag} android_version_code={version_code}");
    }

    println!(
        "[xtask] release version ok: workspace={workspace_version} channel=preview tag={checked_tag}"
    );
    Ok(())
}

/// release tag の preview 番号を返す。
pub(crate) fn validate_release_tag(workspace_version: &str, tag: &str) -> Result<u32> {
    let expected = format!("v{workspace_version}-preview.");
    if !tag.starts_with(&expected) {
        bail!("release tag must start with {expected} and include a preview number, got {tag}");
    }
    let suffix = &tag[expected.len()..];
    if suffix.is_empty() || !suffix.chars().all(|value| value.is_ascii_digit()) {
        bail!("release tag preview suffix must be numeric, got {tag}");
    }
    // `-preview.01` は `-preview.1` と別の tag でも同じ番号（同じ versionCode）になる（#1199）。
    if suffix.starts_with('0') {
        bail!("release tag の preview 番号は 0 で始めない: {tag}");
    }
    suffix
        .parse()
        .with_context(|| format!("release tag の preview 番号が大きすぎる: {tag}"))
}

/// Google Play の versionCode（#1199 AC-2）。Play へ出す候補の release tag の版と preview 番号（`None` は
/// 将来の正式版）だけで決まる。Tauri の既定（major×1,000,000＋minor×1,000＋patch）では同じ版の preview
/// どうしが同じ値になるので、その 100 倍の下 2 桁に段階（preview は 1〜98、正式版は 99）を置く。桁があふれて
/// 別の版・段階と同じ値や逆の順になる入力は拒否する。最大は Play の上限（2,100,000,000）の内側。
pub(crate) fn android_version_code(version: &str, preview: Option<u32>) -> Result<u32> {
    let parts = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("版は major.minor.patch の数字: {version}"))?;
    let &[major, minor, patch] = parts.as_slice() else {
        bail!("版は major.minor.patch の数字: {version}");
    };
    if major > 20 || minor > 999 || patch > 999 {
        bail!("versionCode に収まらない版（major は 20、minor と patch は 999 まで）: {version}");
    }
    let stage = match preview {
        Some(number @ 1..=98) => number,
        Some(number) => bail!("versionCode の preview 番号は 1〜98（99 は正式版）: {number}"),
        None => 99,
    };
    Ok(((major * 1_000 + minor) * 1_000 + patch) * 100 + stage)
}

pub(crate) fn read_package_version(path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]" || trimmed == "[workspace.package]";
            continue;
        }
        if in_package && trimmed.starts_with("version") {
            return parse_toml_string_value(trimmed)
                .with_context(|| format!("failed to parse version in {}", path.display()));
        }
    }
    bail!("version was not found in {}", path.display())
}

pub(crate) fn read_workspace_version(path: &Path) -> Result<String> {
    read_package_version(path)
}

pub(crate) fn parse_toml_string_value(line: &str) -> Result<String> {
    let (_, value) = line
        .split_once('=')
        .context("expected key = \"value\" TOML line")?;
    let value = value.trim();
    if !(value.starts_with('"') && value.ends_with('"') && value.len() >= 2) {
        bail!("expected quoted TOML string value");
    }
    Ok(value[1..value.len() - 1].to_string())
}

pub(crate) fn read_json_version(path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    value
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("version was not found in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tag_accepts_preview_tag_matching_workspace_version() {
        assert_eq!(
            validate_release_tag("0.1.2", "v0.1.2-preview.1").unwrap(),
            1
        );
        assert_eq!(
            validate_release_tag("0.1.2", "v0.1.2-preview.12").unwrap(),
            12
        );
    }

    #[test]
    fn release_tag_rejects_version_mismatch_and_bad_suffixes() {
        assert!(validate_release_tag("0.1.2", "v0.1.3-preview.1").is_err());
        assert!(validate_release_tag("0.1.2", "v0.1.2-preview.").is_err());
        assert!(validate_release_tag("0.1.2", "v0.1.2-preview.1a").is_err());
        assert!(validate_release_tag("0.1.2", "v0.1.2").is_err());
        assert!(validate_release_tag("0.1.2", "v0.1.2-preview.01").is_err());
        assert!(validate_release_tag("0.1.2", "v0.1.2-preview.0").is_err());
    }

    /// 出す順に並べた候補の versionCode が増え続ける（#1199 AC-2）。同じ版の次の候補（失敗した release の
    /// やり直し・作り直し）は次の preview 番号の tag で、同じ tag の再 build は同じ値になる。Play の track は
    /// 同じ候補を昇格するので、track ごとの値は無い。
    #[test]
    fn android_version_code_increases_through_previews_stable_and_next_versions() {
        let releases = [
            ("0.4.3", Some(1)),
            ("0.4.3", Some(2)),
            ("0.4.3", Some(98)),
            ("0.4.3", None),
            ("0.4.4", Some(1)),
            ("0.5.0", Some(1)),
            ("1.0.0", Some(1)),
            ("1.0.0", None),
            ("20.999.999", None),
        ];
        let codes = releases
            .iter()
            .map(|&(version, preview)| android_version_code(version, preview).unwrap())
            .collect::<Vec<_>>();
        assert!(codes.windows(2).all(|pair| pair[0] < pair[1]), "{codes:?}");
        assert_eq!(codes[0], 400_301);
        assert_eq!(codes[3], 400_399);
        assert_eq!(codes[8], 2_099_999_999);
        assert_eq!(android_version_code("0.4.3", Some(2)).unwrap(), codes[1]);
    }

    #[test]
    fn android_version_code_rejects_values_that_collide_or_go_backwards() {
        for (version, preview) in [
            ("0.4.3", Some(0)),
            ("0.4.3", Some(99)),
            ("0.1000.0", Some(1)),
            ("0.4.1000", Some(1)),
            ("21.0.0", Some(1)),
            ("0.4", Some(1)),
            ("0.4.3.1", Some(1)),
            ("0.4.x", Some(1)),
        ] {
            assert!(
                android_version_code(version, preview).is_err(),
                "{version} {preview:?}"
            );
        }
    }

    #[test]
    fn parse_toml_string_value_extracts_quoted_values_only() {
        assert_eq!(
            parse_toml_string_value("version = \"0.1.2\"").unwrap(),
            "0.1.2"
        );
        assert!(parse_toml_string_value("version = 3").is_err());
        assert!(parse_toml_string_value("no equals sign").is_err());
    }
}
