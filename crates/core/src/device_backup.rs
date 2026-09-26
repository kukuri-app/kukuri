use std::collections::HashSet;
use std::io::{Read, Write};

use anyhow::{Context, Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use secp256k1::rand::{RngCore, rng};
use serde::{Deserialize, Serialize};

pub const DEVICE_BACKUP_FORMAT_VERSION: u32 = 1;
/// 2: 旧 `iroh-data` を含めず、保護所有先(`kukuri.db` 一式と保護された `kukuri.remote-blobs/`)を含める(#1221 R5-G)。
/// 1(`iroh-data` 入り)の復元は拒否する。
pub const DEVICE_BACKUP_COMPONENT_VERSION: u32 = 2;
pub const DEVICE_BACKUP_MIN_PASSPHRASE_CHARS: usize = 8;
pub const DEVICE_BACKUP_CHUNK_BYTES: usize = 64 * 1024;
pub const DEVICE_BACKUP_MAX_ENTRY_COUNT: usize = 100_000;
pub const DEVICE_BACKUP_MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const DEVICE_BACKUP_MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;

const MAGIC: &[u8; 20] = b"KUKURI-DEVICE-BACKUP";
const KDF: &str = "argon2id";
const KDF_M_COST_KIB: u32 = 65_536;
const KDF_T_COST: u32 = 3;
const KDF_P_COST: u32 = 1;
const KDF_MAX_M_COST_KIB: u32 = 1 << 20;
const KDF_MAX_T_COST: u32 = 16;
const KDF_MAX_P_COST: u32 = 8;
const SALT_LEN: usize = 16;
const NONCE_PREFIX_LEN: usize = 16;
const HEADER_MAX_BYTES: usize = 4 * 1024;
const MANIFEST_MAX_BYTES: usize = 8 * 1024 * 1024;
const AEAD_TAG_BYTES: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBackupEntryV1 {
    pub name: String,
    pub bytes: u64,
    pub blake3: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBackupManifestV1 {
    pub format_version: u32,
    pub component_version: u32,
    pub created_at: i64,
    pub app_version: String,
    pub public_key: String,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub included: Vec<String>,
    #[serde(default)]
    pub requires_reconsent: Vec<String>,
    pub entries: Vec<DeviceBackupEntryV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DeviceBackupHeaderV1 {
    version: u32,
    kdf: String,
    m_cost_kib: u32,
    t_cost: u32,
    p_cost: u32,
    salt_hex: String,
    nonce_prefix_hex: String,
}

pub struct DeviceBackupWriter<W> {
    writer: W,
    cipher: XChaCha20Poly1305,
    header_digest: [u8; 32],
    nonce_prefix: [u8; NONCE_PREFIX_LEN],
    counter: u64,
    manifest: DeviceBackupManifestV1,
    next_entry: usize,
}

impl<W: Write> DeviceBackupWriter<W> {
    pub fn new(mut writer: W, passphrase: &str, manifest: DeviceBackupManifestV1) -> Result<Self> {
        validate_manifest(&manifest)?;
        if passphrase.chars().count() < DEVICE_BACKUP_MIN_PASSPHRASE_CHARS {
            bail!(
                "device backup passphrase must be at least {DEVICE_BACKUP_MIN_PASSPHRASE_CHARS} characters"
            );
        }

        let mut salt = [0u8; SALT_LEN];
        rng().fill_bytes(&mut salt);
        let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
        rng().fill_bytes(&mut nonce_prefix);
        let header = DeviceBackupHeaderV1 {
            version: DEVICE_BACKUP_FORMAT_VERSION,
            kdf: KDF.to_string(),
            m_cost_kib: KDF_M_COST_KIB,
            t_cost: KDF_T_COST,
            p_cost: KDF_P_COST,
            salt_hex: hex::encode(salt),
            nonce_prefix_hex: hex::encode(nonce_prefix),
        };
        let header_bytes = serde_json::to_vec(&header).context("failed to encode backup header")?;
        if header_bytes.len() > HEADER_MAX_BYTES {
            bail!("device backup header exceeds supported size");
        }
        let key = derive_key(
            passphrase,
            &salt,
            header.m_cost_kib,
            header.t_cost,
            header.p_cost,
        )?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key)
            .context("failed to initialize device backup cipher")?;
        let header_digest = *blake3::hash(&header_bytes).as_bytes();

        writer
            .write_all(MAGIC)
            .context("failed to write backup magic")?;
        write_u32(&mut writer, header_bytes.len() as u32)?;
        writer
            .write_all(&header_bytes)
            .context("failed to write backup header")?;

        let manifest_bytes =
            serde_json::to_vec(&manifest).context("failed to encode backup manifest")?;
        if manifest_bytes.len() > MANIFEST_MAX_BYTES {
            bail!("device backup manifest exceeds supported size");
        }
        let mut archive = Self {
            writer,
            cipher,
            header_digest,
            nonce_prefix,
            counter: 0,
            manifest,
            next_entry: 0,
        };
        archive.write_encrypted_frame(&manifest_bytes)?;
        Ok(archive)
    }

    pub fn write_entry<R, F>(&mut self, mut reader: R, mut on_progress: F) -> Result<()>
    where
        R: Read,
        F: FnMut(u64) -> Result<()>,
    {
        let expected = self
            .manifest
            .entries
            .get(self.next_entry)
            .cloned()
            .ok_or_else(|| anyhow!("device backup contains more entry data than its manifest"))?;
        let mut remaining = expected.bytes;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; DEVICE_BACKUP_CHUNK_BYTES];
        while remaining > 0 {
            let wanted = usize::try_from(remaining.min(DEVICE_BACKUP_CHUNK_BYTES as u64))
                .expect("chunk length fits usize");
            reader
                .read_exact(&mut buffer[..wanted])
                .with_context(|| format!("device backup entry `{}` is truncated", expected.name))?;
            hasher.update(&buffer[..wanted]);
            self.write_encrypted_frame(&buffer[..wanted])?;
            remaining -= wanted as u64;
            on_progress(wanted as u64)?;
        }
        let mut extra = [0u8; 1];
        if reader
            .read(&mut extra)
            .context("failed to check backup entry length")?
            != 0
        {
            bail!(
                "device backup entry `{}` exceeds declared length",
                expected.name
            );
        }
        let actual_hash = hasher.finalize().to_hex().to_string();
        if actual_hash != expected.blake3 {
            bail!(
                "device backup entry `{}` changed while it was read",
                expected.name
            );
        }
        self.next_entry += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<W> {
        if self.next_entry != self.manifest.entries.len() {
            bail!("device backup is missing declared entries");
        }
        write_u32(&mut self.writer, 0)?;
        self.writer
            .flush()
            .context("failed to flush device backup")?;
        Ok(self.writer)
    }

    fn write_encrypted_frame(&mut self, plaintext: &[u8]) -> Result<()> {
        let nonce = nonce(self.nonce_prefix, self.counter);
        let aad = frame_aad(self.header_digest, self.counter);
        let ciphertext = self
            .cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow!("failed to encrypt device backup frame"))?;
        write_u32(&mut self.writer, ciphertext.len() as u32)?;
        self.writer
            .write_all(&ciphertext)
            .context("failed to write device backup frame")?;
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| anyhow!("device backup frame counter overflow"))?;
        Ok(())
    }
}

pub struct DeviceBackupReader<R> {
    reader: R,
    cipher: XChaCha20Poly1305,
    header_digest: [u8; 32],
    nonce_prefix: [u8; NONCE_PREFIX_LEN],
    counter: u64,
    manifest: DeviceBackupManifestV1,
    next_entry: usize,
}

impl<R: Read> DeviceBackupReader<R> {
    pub fn open(mut reader: R, passphrase: &str) -> Result<Self> {
        let mut magic = [0u8; MAGIC.len()];
        reader
            .read_exact(&mut magic)
            .context("invalid or truncated device backup")?;
        if &magic != MAGIC {
            bail!("unsupported device backup format or version");
        }
        let header_len = read_u32(&mut reader)? as usize;
        if header_len == 0 || header_len > HEADER_MAX_BYTES {
            bail!("device backup header exceeds supported size");
        }
        let mut header_bytes = vec![0u8; header_len];
        reader
            .read_exact(&mut header_bytes)
            .context("truncated device backup header")?;
        let header: DeviceBackupHeaderV1 =
            serde_json::from_slice(&header_bytes).context("invalid device backup header")?;
        validate_header(&header)?;
        let salt = decode_fixed::<SALT_LEN>(&header.salt_hex, "device backup salt")?;
        let nonce_prefix = decode_fixed::<NONCE_PREFIX_LEN>(
            &header.nonce_prefix_hex,
            "device backup nonce prefix",
        )?;
        let key = derive_key(
            passphrase,
            &salt,
            header.m_cost_kib,
            header.t_cost,
            header.p_cost,
        )?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key)
            .context("failed to initialize device backup cipher")?;
        let header_digest = *blake3::hash(&header_bytes).as_bytes();
        let mut archive = Self {
            reader,
            cipher,
            header_digest,
            nonce_prefix,
            counter: 0,
            manifest: DeviceBackupManifestV1 {
                format_version: 0,
                component_version: 0,
                created_at: 0,
                app_version: String::new(),
                public_key: String::new(),
                account_label: None,
                included: Vec::new(),
                requires_reconsent: Vec::new(),
                entries: Vec::new(),
            },
            next_entry: 0,
        };
        let manifest_plaintext = archive.read_encrypted_frame(MANIFEST_MAX_BYTES)?;
        let manifest: DeviceBackupManifestV1 = serde_json::from_slice(&manifest_plaintext)
            .context("invalid device backup manifest")?;
        validate_manifest(&manifest)?;
        archive.manifest = manifest;
        Ok(archive)
    }

    pub fn manifest(&self) -> &DeviceBackupManifestV1 {
        &self.manifest
    }

    pub fn read_entry<W, F>(&mut self, mut writer: W, mut on_progress: F) -> Result<()>
    where
        W: Write,
        F: FnMut(u64) -> Result<()>,
    {
        let expected = self
            .manifest
            .entries
            .get(self.next_entry)
            .cloned()
            .ok_or_else(|| anyhow!("device backup contains more entry data than its manifest"))?;
        let mut remaining = expected.bytes;
        let mut hasher = blake3::Hasher::new();
        while remaining > 0 {
            let plaintext = self.read_encrypted_frame(DEVICE_BACKUP_CHUNK_BYTES)?;
            if plaintext.is_empty() || plaintext.len() as u64 > remaining {
                bail!(
                    "device backup entry `{}` has an invalid chunk length",
                    expected.name
                );
            }
            writer.write_all(&plaintext).with_context(|| {
                format!("failed to restore device backup entry `{}`", expected.name)
            })?;
            hasher.update(&plaintext);
            remaining -= plaintext.len() as u64;
            on_progress(plaintext.len() as u64)?;
        }
        writer
            .flush()
            .with_context(|| format!("failed to flush device backup entry `{}`", expected.name))?;
        let actual_hash = hasher.finalize().to_hex().to_string();
        if actual_hash != expected.blake3 {
            bail!(
                "device backup entry `{}` failed its integrity check",
                expected.name
            );
        }
        self.next_entry += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<R> {
        if self.next_entry != self.manifest.entries.len() {
            bail!("device backup is missing declared entries");
        }
        let marker = read_u32(&mut self.reader)?;
        if marker != 0 {
            bail!("device backup contains undeclared entry data");
        }
        let mut trailing = [0u8; 1];
        if self
            .reader
            .read(&mut trailing)
            .context("failed to validate device backup ending")?
            != 0
        {
            bail!("device backup contains trailing data");
        }
        Ok(self.reader)
    }

    fn read_encrypted_frame(&mut self, plaintext_limit: usize) -> Result<Vec<u8>> {
        let ciphertext_len = read_u32(&mut self.reader)? as usize;
        if ciphertext_len < AEAD_TAG_BYTES || ciphertext_len > plaintext_limit + AEAD_TAG_BYTES {
            bail!("device backup frame exceeds supported size");
        }
        let mut ciphertext = vec![0u8; ciphertext_len];
        self.reader
            .read_exact(&mut ciphertext)
            .context("truncated device backup frame")?;
        let nonce = nonce(self.nonce_prefix, self.counter);
        let aad = frame_aad(self.header_digest, self.counter);
        let plaintext = self
            .cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| {
                anyhow!("failed to decrypt device backup: wrong passphrase or corrupted data")
            })?;
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| anyhow!("device backup frame counter overflow"))?;
        Ok(plaintext)
    }
}

fn validate_header(header: &DeviceBackupHeaderV1) -> Result<()> {
    if header.version != DEVICE_BACKUP_FORMAT_VERSION {
        bail!("unsupported device backup version `{}`", header.version);
    }
    if header.kdf != KDF {
        bail!("unsupported device backup kdf `{}`", header.kdf);
    }
    if header.m_cost_kib > KDF_MAX_M_COST_KIB
        || header.t_cost > KDF_MAX_T_COST
        || header.p_cost > KDF_MAX_P_COST
    {
        bail!("device backup kdf parameters exceed supported bounds");
    }
    Ok(())
}

fn validate_manifest(manifest: &DeviceBackupManifestV1) -> Result<()> {
    if manifest.format_version != DEVICE_BACKUP_FORMAT_VERSION {
        bail!(
            "unsupported device backup manifest version `{}`",
            manifest.format_version
        );
    }
    if manifest.component_version != DEVICE_BACKUP_COMPONENT_VERSION {
        bail!(
            "unsupported device backup component version `{}`",
            manifest.component_version
        );
    }
    crate::crypto::validate_pubkey(&manifest.public_key)
        .context("invalid device backup public key")?;
    if manifest.entries.is_empty() || manifest.entries.len() > DEVICE_BACKUP_MAX_ENTRY_COUNT {
        bail!("device backup entry count exceeds supported bounds");
    }
    let mut names = HashSet::new();
    let mut total = 0u64;
    for entry in &manifest.entries {
        if entry.name.is_empty() || entry.name.len() > 512 || !names.insert(entry.name.as_str()) {
            bail!("device backup contains an invalid or duplicate entry name");
        }
        if entry.bytes > DEVICE_BACKUP_MAX_ENTRY_BYTES {
            bail!(
                "device backup entry `{}` exceeds supported size",
                entry.name
            );
        }
        total = total
            .checked_add(entry.bytes)
            .ok_or_else(|| anyhow!("device backup total size overflow"))?;
        if total > DEVICE_BACKUP_MAX_TOTAL_BYTES {
            bail!("device backup total size exceeds supported bounds");
        }
        let hash = hex::decode(&entry.blake3)
            .with_context(|| format!("invalid hash for device backup entry `{}`", entry.name))?;
        if hash.len() != 32 {
            bail!("invalid hash for device backup entry `{}`", entry.name);
        }
    }
    Ok(())
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    m_cost_kib: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<[u8; 32]> {
    if m_cost_kib > KDF_MAX_M_COST_KIB || t_cost > KDF_MAX_T_COST || p_cost > KDF_MAX_P_COST {
        bail!("device backup kdf parameters exceed supported bounds");
    }
    let params = Params::new(m_cost_kib, t_cost, p_cost, Some(32))
        .map_err(|_| anyhow!("invalid device backup kdf parameters"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| anyhow!("failed to derive device backup key"))?;
    Ok(key)
}

fn nonce(prefix: [u8; NONCE_PREFIX_LEN], counter: u64) -> [u8; 24] {
    let mut nonce = [0u8; 24];
    nonce[..NONCE_PREFIX_LEN].copy_from_slice(&prefix);
    nonce[NONCE_PREFIX_LEN..].copy_from_slice(&counter.to_le_bytes());
    nonce
}

fn frame_aad(header_digest: [u8; 32], counter: u64) -> [u8; 40] {
    let mut aad = [0u8; 40];
    aad[..32].copy_from_slice(&header_digest);
    aad[32..].copy_from_slice(&counter.to_le_bytes());
    aad
}

fn decode_fixed<const N: usize>(value: &str, label: &str) -> Result<[u8; N]> {
    let decoded = hex::decode(value).with_context(|| format!("invalid {label}"))?;
    decoded
        .try_into()
        .map_err(|_| anyhow!("invalid {label} length"))
}

fn write_u32(writer: &mut impl Write, value: u32) -> Result<()> {
    writer
        .write_all(&value.to_le_bytes())
        .context("failed to write device backup framing")
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .context("truncated device backup framing")?;
    Ok(u32::from_le_bytes(bytes))
}
