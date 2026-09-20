//! Linux配布物の署名と成果物一覧を、build完了後に検査する。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Serialize)]
struct AppImageArtifacts {
    version: String,
    target: &'static str,
    file: String,
    sha256: String,
    signature_file: String,
    signature: String,
}

pub(crate) fn verify_package() -> Result<()> {
    let tauri_dir = crate::desktop_dir().join("src-tauri");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tauri_dir.join("tauri.conf.json"))?)?;
    let version = config["version"]
        .as_str()
        .context("missing package version")?;
    let name = config["productName"]
        .as_str()
        .context("missing product name")?;
    let pubkey = config["plugins"]["updater"]["pubkey"]
        .as_str()
        .context("missing updater public key")?;
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                tauri_dir.join(path)
            }
        })
        .unwrap_or_else(|| tauri_dir.join("target"));
    let bundle_root = target_dir.join("x86_64-unknown-linux-gnu/release/bundle");
    for format in ["appimage", "deb"] {
        let bundle_dir = bundle_root.join(format);
        let artifacts = if format == "appimage" {
            inspect_package(&bundle_dir, name, version, pubkey)?
        } else {
            inspect_signed_bundle(&bundle_dir, name, version, pubkey, "deb", b"!<arch>\n")?
        };
        if format == "appimage" {
            crate::linuxdeploy::ensure_no_host_libraries(
                &bundle_dir.join(format!("{name}.AppDir")),
                "AppDir",
            )?;
            ensure_appimage_has_no_host_libraries(&bundle_dir, &artifacts.file)?;
        }
        if format == "deb" {
            let status = std::process::Command::new("python3")
                .arg(crate::root_dir().join("scripts/release/deb_package.py"))
                .arg(bundle_dir.join(&artifacts.file))
                .args(["--version", version])
                .status()?;
            if !status.success() {
                bail!("Deb payload verification failed");
            }
        }
        std::fs::write(
            bundle_dir.join(format!("{format}-artifacts.json")),
            serde_json::to_vec_pretty(&artifacts)?,
        )?;
        println!(
            "[xtask] verified Linux package and updater signature: {}",
            artifacts.file
        );
    }
    Ok(())
}

/// AppDirではなく、配布するAppImage自体の中身を確認する。FUSEは使わない。
fn ensure_appimage_has_no_host_libraries(bundle_dir: &Path, file: &str) -> Result<()> {
    let extract_dir = bundle_dir.join("host-library-check");
    if extract_dir.exists() {
        std::fs::remove_dir_all(&extract_dir)?;
    }
    std::fs::create_dir_all(&extract_dir)?;
    crate::run(
        &bundle_dir.join(file).to_string_lossy(),
        ["--appimage-extract", "usr/lib/*"],
        &extract_dir,
    )?;
    let lib_dir = extract_dir.join("squashfs-root/usr/lib");
    if !lib_dir.is_dir() {
        bail!("AppImage から usr/lib を展開できません");
    }
    crate::linuxdeploy::ensure_no_host_libraries(&lib_dir, "AppImage")?;
    std::fs::remove_dir_all(&extract_dir)?;
    Ok(())
}

fn decode_text(value: &str) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .context("invalid base64 signing metadata")?;
    String::from_utf8(bytes).context("invalid signing metadata text")
}

fn inspect_package(dir: &Path, name: &str, version: &str, key: &str) -> Result<AppImageArtifacts> {
    inspect_signed_bundle(dir, name, version, key, "AppImage", b"\x7fELF")
}

fn inspect_signed_bundle(
    dir: &Path,
    name: &str,
    version: &str,
    key: &str,
    extension: &str,
    magic: &[u8],
) -> Result<AppImageArtifacts> {
    let file = format!("{name}_{version}_amd64.{extension}");
    if Path::new(&file).file_name().and_then(|part| part.to_str()) != Some(file.as_str()) {
        bail!("package name/version must not contain path components");
    }
    let signature_file = format!("{file}.sig");
    let bundle = std::fs::read(dir.join(&file)).context("missing Linux package")?;
    // ELF magicだけで実行可能性を断定しない。実起動はpackaged smokeが所有する。
    if !bundle.starts_with(magic) {
        bail!("Linux package is empty or has the wrong format");
    }
    let signature = std::fs::read_to_string(dir.join(&signature_file))
        .context("missing Linux package updater signature")?;
    verify_signature(&bundle, key, &signature)?;
    Ok(AppImageArtifacts {
        version: version.to_owned(),
        target: if extension == "deb" {
            "linux-x86_64-deb"
        } else {
            "linux-x86_64"
        },
        file,
        sha256: hex::encode(Sha256::digest(&bundle)),
        signature_file,
        signature: signature.trim().to_owned(),
    })
}

fn verify_signature(bundle: &[u8], key: &str, signature: &str) -> Result<()> {
    let public_key = PublicKey::decode(&decode_text(key)?).context("invalid updater public key")?;
    let signature =
        Signature::decode(&decode_text(signature)?).context("invalid updater signature")?;
    public_key
        .verify(bundle, &signature, true)
        .context("Linux package updater signature does not match configured public key")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_empty_signing_metadata_is_rejected() {
        for (key, signature) in [("", ""), ("invalid", "invalid"), ("Zg==", "Zg==")] {
            assert!(verify_signature(b"\x7fELFfixture", key, signature).is_err());
        }
    }

    #[test]
    fn missing_bundle_and_path_components_are_rejected() {
        let dir = crate::root_dir().join("test-results/nonexistent-appimage-contract");
        assert!(inspect_package(&dir, "kukuri", "0.1.8", "").is_err());
        assert!(inspect_package(&dir, "../kukuri", "0.1.8", "").is_err());
    }

    #[test]
    #[ignore = "requires an AppImage and its test signing metadata"]
    fn signed_appimage_accepts_only_matching_bundle_and_key() {
        let dir = PathBuf::from(std::env::var_os("KUKURI_APPIMAGE_TEST_DIR").unwrap());
        let key = std::fs::read_to_string(dir.join("test-updater-key.pub")).unwrap();
        let artifacts = inspect_package(&dir, "kukuri", env!("CARGO_PKG_VERSION"), &key).unwrap();
        assert_eq!(artifacts.target, "linux-x86_64");
        let mut bundle = std::fs::read(dir.join(&artifacts.file)).unwrap();
        bundle[0] ^= 1;
        assert!(verify_signature(&bundle, &key, &artifacts.signature).is_err());
        bundle[0] ^= 1;
        let decoded_key = decode_text(&key).unwrap();
        let mut lines: Vec<String> = decoded_key.lines().map(str::to_owned).collect();
        let mut key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&lines[1])
            .unwrap();
        *key_bytes.last_mut().unwrap() ^= 1;
        lines[1] = base64::engine::general_purpose::STANDARD.encode(key_bytes);
        let wrong_key = base64::engine::general_purpose::STANDARD.encode(lines.join("\n"));
        assert!(verify_signature(&bundle, &wrong_key, &artifacts.signature).is_err());
        assert!(verify_signature(&bundle, &key, "").is_err());
    }
}
