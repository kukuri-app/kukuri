use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;

use n0_future::future::block_on;

use crate::{
    ACCOUNT_KEY_EXPORT_KDF, ACCOUNT_KEY_EXPORT_PREFIX, ACCOUNT_KEY_EXPORT_VERSION, KukuriKeys,
    PassphraseKdf, decrypt_account_key_export, encrypt_account_key_export,
    preview_account_key_export,
};

const PASSPHRASE: &str = "correct horse battery staple";

async fn derive(input: PassphraseKdf) -> anyhow::Result<[u8; 32]> {
    input.derive()
}

fn seal(keys: &KukuriKeys, passphrase: &str) -> anyhow::Result<String> {
    block_on(encrypt_account_key_export(keys, passphrase, derive))
}

fn open(export: &str, passphrase: &str) -> anyhow::Result<KukuriKeys> {
    block_on(decrypt_account_key_export(export, passphrase, derive))
}

fn tamper_envelope_field(export: &str, field: &str, value: serde_json::Value) -> String {
    let body = export
        .strip_prefix(ACCOUNT_KEY_EXPORT_PREFIX)
        .expect("export prefix");
    let bytes = BASE64_URL.decode(body).expect("export base64");
    let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("export json");
    envelope[field] = value;
    format!(
        "{ACCOUNT_KEY_EXPORT_PREFIX}{}",
        BASE64_URL.encode(serde_json::to_vec(&envelope).expect("tampered json"))
    )
}

fn read_envelope_field(export: &str, field: &str) -> serde_json::Value {
    let body = export
        .strip_prefix(ACCOUNT_KEY_EXPORT_PREFIX)
        .expect("export prefix");
    let bytes = BASE64_URL.decode(body).expect("export base64");
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("export json");
    envelope[field].clone()
}

#[test]
fn export_round_trips_to_same_secret_and_public_key() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let restored = open(&export, PASSPHRASE).expect("import");
    assert_eq!(restored.public_key_hex(), keys.public_key_hex());
    assert_eq!(restored.export_secret_hex(), keys.export_secret_hex());
}

#[test]
fn export_envelope_does_not_contain_plaintext_secret() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let secret_hex = keys.export_secret_hex();
    assert!(!export.contains(secret_hex.as_str()));
    let body = export
        .strip_prefix(ACCOUNT_KEY_EXPORT_PREFIX)
        .expect("export prefix");
    let decoded = String::from_utf8(BASE64_URL.decode(body).expect("export base64"))
        .expect("export json utf8");
    assert!(!decoded.contains(secret_hex.as_str()));
}

#[test]
fn preview_returns_fingerprint_without_passphrase() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let preview = preview_account_key_export(&export).expect("preview");
    assert_eq!(preview.version, ACCOUNT_KEY_EXPORT_VERSION);
    assert_eq!(preview.kdf, ACCOUNT_KEY_EXPORT_KDF);
    assert_eq!(preview.public_key.as_str(), keys.public_key_hex());
}

#[test]
fn wrong_passphrase_fails_to_decrypt() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let err = open(&export, "not the passphrase").expect_err("must fail");
    assert!(err.to_string().contains("wrong passphrase or corrupted"));
}

#[test]
fn short_passphrase_is_rejected_on_export() {
    let keys = KukuriKeys::generate();
    let err = seal(&keys, "short").expect_err("must fail");
    assert!(err.to_string().contains("at least"));
}

#[test]
fn corrupted_ciphertext_fails_to_decrypt() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let ciphertext_hex = read_envelope_field(&export, "ciphertext_hex")
        .as_str()
        .expect("ciphertext hex")
        .to_string();
    let mut bytes = hex::decode(&ciphertext_hex).expect("ciphertext bytes");
    bytes[0] ^= 0x01;
    let tampered = tamper_envelope_field(
        &export,
        "ciphertext_hex",
        serde_json::Value::String(hex::encode(bytes)),
    );
    open(&tampered, PASSPHRASE).expect_err("must fail");
}

#[test]
fn truncated_export_fails_to_parse() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let truncated = &export[..export.len() - 10];
    preview_account_key_export(truncated).expect_err("must fail");
    open(truncated, PASSPHRASE).expect_err("must fail");
}

#[test]
fn unknown_version_is_rejected() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let tampered = tamper_envelope_field(&export, "version", serde_json::json!(2));
    let err = preview_account_key_export(&tampered).expect_err("must fail");
    assert!(
        err.to_string()
            .contains("unsupported account key export version")
    );
    open(&tampered, PASSPHRASE).expect_err("must fail");
}

#[test]
fn unknown_prefix_is_rejected() {
    let err = preview_account_key_export("kukuri-account-key.v9.abcdef").expect_err("must fail");
    assert!(err.to_string().contains("unsupported account key export"));
}

#[test]
fn tampered_public_key_fails_authentication() {
    let keys = KukuriKeys::generate();
    let other = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let tampered = tamper_envelope_field(
        &export,
        "public_key",
        serde_json::Value::String(other.public_key_hex()),
    );
    open(&tampered, PASSPHRASE).expect_err("must fail");
}

#[test]
fn tampered_kdf_params_fail_authentication() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let tampered = tamper_envelope_field(&export, "t_cost", serde_json::json!(1));
    open(&tampered, PASSPHRASE).expect_err("must fail");
}

#[test]
fn oversized_kdf_params_are_rejected_before_derivation() {
    let keys = KukuriKeys::generate();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let tampered = tamper_envelope_field(&export, "m_cost_kib", serde_json::json!(u32::MAX));
    let err = open(&tampered, PASSPHRASE).expect_err("must fail");
    assert!(err.to_string().contains("exceed supported bounds"));
}

#[test]
fn preview_and_export_debug_never_reveal_secret() {
    let keys = KukuriKeys::generate();
    let secret_hex = keys.export_secret_hex();
    let export = seal(&keys, PASSPHRASE).expect("export");
    let preview = preview_account_key_export(&export).expect("preview");
    let debug = format!("{keys:?} {preview:?}");
    assert!(!debug.contains(secret_hex.as_str()));
    let preview_json = serde_json::to_string(&preview).expect("preview json");
    assert!(!preview_json.contains(secret_hex.as_str()));
}
