use std::io::{Cursor, Error, Result as IoResult, Write};

use crate::{
    DEVICE_BACKUP_COMPONENT_VERSION, DEVICE_BACKUP_FORMAT_VERSION, DeviceBackupEntryV1,
    DeviceBackupManifestV1, DeviceBackupReader, DeviceBackupWriter, KukuriKeys,
};

fn fixture() -> (DeviceBackupManifestV1, Vec<Vec<u8>>) {
    let keys = KukuriKeys::generate();
    let values = vec![b"sqlite fixture".to_vec(), vec![7u8; 70_000], Vec::new()];
    let entries = ["database", "iroh/blobs/data", "frontend-state"]
        .into_iter()
        .zip(&values)
        .map(|(name, value)| DeviceBackupEntryV1 {
            name: name.to_string(),
            bytes: value.len() as u64,
            blake3: blake3::hash(value).to_hex().to_string(),
        })
        .collect();
    (
        DeviceBackupManifestV1 {
            format_version: DEVICE_BACKUP_FORMAT_VERSION,
            component_version: DEVICE_BACKUP_COMPONENT_VERSION,
            created_at: 1_700_000_000,
            app_version: "0.1.8".to_string(),
            public_key: keys.public_key_hex(),
            account_label: Some("main".to_string()),
            included: vec!["database".to_string(), "local media".to_string()],
            requires_reconsent: vec!["app consent".to_string()],
            entries,
        },
        values,
    )
}

fn archive_bytes(passphrase: &str) -> (DeviceBackupManifestV1, Vec<Vec<u8>>, Vec<u8>) {
    let (manifest, values) = fixture();
    let mut writer = DeviceBackupWriter::new(Vec::new(), passphrase, manifest.clone()).unwrap();
    for value in &values {
        writer.write_entry(Cursor::new(value), |_| Ok(())).unwrap();
    }
    let bytes = writer.finish().unwrap();
    (manifest, values, bytes)
}

#[derive(Debug)]
struct FailingWriter {
    remaining: Option<usize>,
    fail_flush: bool,
}

impl FailingWriter {
    fn after(bytes: usize) -> Self {
        Self {
            remaining: Some(bytes),
            fail_flush: false,
        }
    }

    fn on_flush() -> Self {
        Self {
            remaining: None,
            fail_flush: true,
        }
    }
}

impl Write for FailingWriter {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        let Some(remaining) = self.remaining.as_mut() else {
            return Ok(bytes.len());
        };
        if *remaining == 0 {
            return Err(Error::other("simulated storage exhaustion"));
        }
        let written = bytes.len().min(*remaining);
        *remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> IoResult<()> {
        if self.fail_flush {
            return Err(Error::other("simulated flush failure"));
        }
        Ok(())
    }
}

fn archive_with_header_version(mut bytes: Vec<u8>, replacement: u8) -> Vec<u8> {
    let needle = b"\"version\":1";
    let index = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("backup header version");
    bytes[index + needle.len() - 1] = replacement;
    bytes
}

#[test]
fn encrypted_backup_round_trips_multiple_chunks_and_empty_entry() {
    let (manifest, values, bytes) = archive_bytes("long enough passphrase");
    let mut reader =
        DeviceBackupReader::open(Cursor::new(bytes), "long enough passphrase").unwrap();
    assert_eq!(reader.manifest(), &manifest);
    for expected in values {
        let mut actual = Vec::new();
        reader.read_entry(&mut actual, |_| Ok(())).unwrap();
        assert_eq!(actual, expected);
    }
    reader.finish().unwrap();
}

#[test]
fn wrong_passphrase_and_corruption_are_rejected() {
    let (_, _, bytes) = archive_bytes("long enough passphrase");
    let err = match DeviceBackupReader::open(Cursor::new(bytes.clone()), "wrong passphrase") {
        Ok(_) => panic!("wrong passphrase unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(err.to_string().contains("wrong passphrase or corrupted"));

    let mut corrupted = bytes;
    let index = corrupted.len() - 8;
    corrupted[index] ^= 0x80;
    let mut reader =
        DeviceBackupReader::open(Cursor::new(corrupted), "long enough passphrase").unwrap();
    let mut failed = false;
    for _ in 0..reader.manifest().entries.len() {
        if reader.read_entry(Vec::new(), |_| Ok(())).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
}

#[test]
fn truncation_and_trailing_data_are_rejected() {
    let (_, values, bytes) = archive_bytes("long enough passphrase");
    let truncated = bytes[..bytes.len() - 6].to_vec();
    let mut reader =
        DeviceBackupReader::open(Cursor::new(truncated), "long enough passphrase").unwrap();
    let mut saw_error = false;
    for _ in values {
        if reader.read_entry(Vec::new(), |_| Ok(())).is_err() {
            saw_error = true;
            break;
        }
    }
    if !saw_error {
        saw_error = reader.finish().is_err();
    }
    assert!(saw_error);

    let (_, values, mut trailing) = archive_bytes("long enough passphrase");
    trailing.push(1);
    let mut reader =
        DeviceBackupReader::open(Cursor::new(trailing), "long enough passphrase").unwrap();
    for _ in values {
        reader.read_entry(Vec::new(), |_| Ok(())).unwrap();
    }
    assert!(reader.finish().is_err());
}

#[test]
fn changed_entry_and_cancel_are_rejected() {
    let (manifest, values) = fixture();
    let mut writer =
        DeviceBackupWriter::new(Vec::new(), "long enough passphrase", manifest.clone()).unwrap();
    let mut changed = values[0].clone();
    changed[0] ^= 1;
    assert!(
        writer
            .write_entry(Cursor::new(changed), |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("changed while it was read")
    );

    let mut writer =
        DeviceBackupWriter::new(Vec::new(), "long enough passphrase", manifest).unwrap();
    let err = writer
        .write_entry(Cursor::new(&values[0]), |_| anyhow::bail!("canceled"))
        .unwrap_err();
    assert!(err.to_string().contains("canceled"));
}

#[test]
fn weak_passphrase_and_invalid_manifest_are_rejected() {
    let (mut manifest, _) = fixture();
    let err = match DeviceBackupWriter::new(Vec::new(), "short", manifest.clone()) {
        Ok(_) => panic!("weak passphrase unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(err.to_string().contains("at least"));

    manifest.entries.push(manifest.entries[0].clone());
    let err = match DeviceBackupWriter::new(Vec::new(), "long enough passphrase", manifest) {
        Ok(_) => panic!("duplicate manifest entry unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(err.to_string().contains("duplicate entry name"));
}

#[test]
fn storage_exhaustion_and_flush_failures_are_propagated() {
    let (manifest, values) = fixture();
    let mut archive = DeviceBackupWriter::new(
        FailingWriter::after(10_000),
        "long enough passphrase",
        manifest.clone(),
    )
    .expect("header and manifest fit before the simulated failure");
    let mut error = None;
    for value in &values {
        if let Err(current) = archive.write_entry(Cursor::new(value), |_| Ok(())) {
            error = Some(current);
            break;
        }
    }
    assert!(
        error
            .expect("entry write must hit the simulated capacity")
            .to_string()
            .contains("failed to write device backup frame")
    );

    let mut archive = DeviceBackupWriter::new(
        FailingWriter::on_flush(),
        "long enough passphrase",
        manifest,
    )
    .expect("create writer");
    for value in &values {
        archive
            .write_entry(Cursor::new(value), |_| Ok(()))
            .expect("write entry before flush failure");
    }
    assert!(
        archive
            .finish()
            .expect_err("flush failure must be returned")
            .to_string()
            .contains("failed to flush device backup")
    );
}

#[test]
fn unknown_format_and_component_versions_are_rejected() {
    let (_, _, bytes) = archive_bytes("long enough passphrase");
    let unknown_header = archive_with_header_version(bytes, b'9');
    let error =
        match DeviceBackupReader::open(Cursor::new(unknown_header), "long enough passphrase") {
            Ok(_) => panic!("unknown header version unexpectedly succeeded"),
            Err(error) => error,
        };
    assert!(
        error
            .to_string()
            .contains("unsupported device backup version")
    );

    let (mut manifest, _) = fixture();
    manifest.format_version += 1;
    let error = match DeviceBackupWriter::new(Vec::new(), "long enough passphrase", manifest) {
        Ok(_) => panic!("unknown manifest version unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("unsupported device backup manifest version")
    );

    // #1221 R5-G: 旧形式(1: `iroh-data` 入り)も未知の版と同じく拒否する。
    for version in [DEVICE_BACKUP_COMPONENT_VERSION + 1, 1] {
        let (mut manifest, _) = fixture();
        manifest.component_version = version;
        let error = match DeviceBackupWriter::new(Vec::new(), "long enough passphrase", manifest) {
            Ok(_) => panic!("unsupported component version unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("unsupported device backup component version")
        );
    }
}
