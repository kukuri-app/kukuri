//! 鍵・設定・最小状態の保存（`ClientStorage` の Web 実装。ADR 0059 §1）。
//!
//! - `kukuri-device-v1`（origin に 1 つ）: 値を包む AES-GCM の `CryptoKey`（non-extractable）と、account に属さない値。
//! - `kukuri-vault-v1-<account id>`: path・keyring の account が `accounts/<account id>/` を含む値（アカウント鍵・endpoint
//!   秘密鍵・token・設定と最小状態）。
//!
//! keyring の値（`service` が file でないもの）は `secrets[[service, account]]`、file の値は `settings[path]` に置く。どの値も
//! device の `CryptoKey` で包み（`service` と `account` を AAD にして、別の key へ移した値は開けない）、strict の transaction
//! で書く。保存の成功は transaction の確定で判定する。起動時に全件を読まず、key で読む。回収しない。
//! 包みは profile の file を持ち出されたときの保護で、origin の中で動くコードからは守れない（信頼境界）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use js_sys::{Array, Object, Reflect, Uint8Array};
use kukuri_desktop_runtime::ClientStorage;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CryptoKey, IdbDatabase};

use crate::actor::Actor;
use crate::idb::{self, Mode, StorageFailure, js_error, js_error_or};
use crate::rows::{Txn, key, text};

const DEVICE: &str = "kukuri-device-v1";
const VERSION: u32 = 1;
const KEYS: &str = "keys";
const SECRETS: &str = "secrets";
const SETTINGS: &str = "settings";
/// 値を包む鍵の key（`keys` の store）。
const WRAP: &str = "wrap";
/// file の値を表す `service`（desktop-runtime の `FILE_SERVICE`）。
const FILE: &str = "file";
const IV_BYTES: usize = 12;
/// account の ID（公開鍵の hex の先頭 16 文字）。
const ACCOUNT_ID_CHARS: usize = 16;

/// device の接続・値を包む鍵・開いた vault の接続（account ごと）。
#[derive(Clone)]
struct Vaults {
    device: IdbDatabase,
    wrap: CryptoKey,
    opened: Rc<RefCell<HashMap<String, IdbDatabase>>>,
}

/// `ClientStorage` の Web 実装。
pub struct BrowserStorage {
    actor: Actor<Vaults>,
}

/// 値を置く account の vault（`accounts/<account id>/` を含む path・keyring の account）。無ければ device。
fn vault_of(account: &str) -> Option<&str> {
    let mut parts = account.split(['/', '\\']);
    while let Some(part) = parts.next() {
        if part == "accounts"
            && let Some(id) = parts.clone().next()
            && id.len() == ACCOUNT_ID_CHARS
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && parts.clone().nth(1).is_some()
        {
            return Some(id);
        }
    }
    None
}

fn create_device(db: &IdbDatabase) -> std::result::Result<(), JsValue> {
    db.create_object_store(KEYS)?;
    create_vault(db)
}

fn create_vault(db: &IdbDatabase) -> std::result::Result<(), JsValue> {
    db.create_object_store(SECRETS)?;
    db.create_object_store(SETTINGS)?;
    Ok(())
}

fn subtle() -> Result<web_sys::SubtleCrypto> {
    Ok(web_sys::window()
        .ok_or_else(|| anyhow!(StorageFailure::Denied))?
        .crypto()
        .map_err(js_error)?
        .subtle())
}

fn algorithm(iv: &[u8], aad: &str) -> Result<Object> {
    let params = Object::new();
    for (name, value) in [
        ("name", JsValue::from_str("AES-GCM")),
        ("iv", Uint8Array::from(iv).into()),
        ("additionalData", Uint8Array::from(aad.as_bytes()).into()),
    ] {
        Reflect::set(&params, &name.into(), &value).map_err(js_error)?;
    }
    Ok(params)
}

/// 値を包む鍵を読む。無ければ作って置く（同時に作られたときは先に置かれた鍵を使う）。
async fn wrap_key(device: &IdbDatabase) -> Result<CryptoKey> {
    let read = async || -> Result<Option<CryptoKey>> {
        let tx = Txn::begin(device, &[KEYS], Mode::Read)?;
        let request = crate::rows::store(&tx, KEYS)?
            .get(&text(WRAP))
            .map_err(js_error)?;
        let value = idb::done(&request).await?;
        Ok((!value.is_undefined()).then(|| value.unchecked_into()))
    };
    if let Some(key) = read().await? {
        return Ok(key);
    }
    let spec = Object::new();
    Reflect::set(&spec, &"name".into(), &"AES-GCM".into()).map_err(js_error)?;
    Reflect::set(&spec, &"length".into(), &256.into()).map_err(js_error)?;
    let usages = Array::of2(&"encrypt".into(), &"decrypt".into());
    let created: CryptoKey = JsFuture::from(
        subtle()?
            .generate_key_with_object(&spec, false, &usages)
            .map_err(js_error)?,
    )
    .await
    .map_err(js_error)?
    .unchecked_into();
    let tx = Txn::begin(device, &[KEYS], Mode::Strict)?;
    let store = crate::rows::store(&tx, KEYS)?;
    let existing = idb::done(&store.get(&text(WRAP)).map_err(js_error)?).await?;
    let key = if existing.is_undefined() {
        store
            .put_with_key(&created, &text(WRAP))
            .map_err(js_error)?;
        created
    } else {
        existing.unchecked_into()
    };
    tx.commit().await?;
    Ok(key)
}

impl Vaults {
    /// 値を置く database と store と key。
    async fn place(
        &self,
        service: &str,
        account: &str,
    ) -> Result<(IdbDatabase, &'static str, JsValue)> {
        let db = match vault_of(account) {
            None => self.device.clone(),
            Some(id) => {
                let opened = self.opened.borrow().get(id).cloned();
                match opened {
                    Some(db) => db,
                    None => {
                        let db = idb::open(&format!("kukuri-vault-v1-{id}"), VERSION, create_vault)
                            .await?;
                        self.opened.borrow_mut().insert(id.to_owned(), db.clone());
                        db
                    }
                }
            }
        };
        Ok(if service == FILE {
            (db, SETTINGS, text(account))
        } else {
            (db, SECRETS, key(&[text(service), text(account)]))
        })
    }
}

fn aad(service: &str, account: &str) -> String {
    format!("{service}\0{account}")
}

impl BrowserStorage {
    /// device の database を開き、値を包む鍵を用意する。値は読まない。
    pub async fn open() -> Result<Self> {
        let (actor, ()) = Actor::start(
            || async {
                let device = idb::open(DEVICE, VERSION, create_device).await?;
                let wrap = wrap_key(&device).await?;
                let vaults = Vaults {
                    device,
                    wrap,
                    opened: Rc::default(),
                };
                Ok((vaults, ()))
            },
            |vaults| {
                vaults.device.close();
                for db in vaults.opened.borrow().values() {
                    db.close();
                }
            },
        )
        .await?;
        Ok(Self { actor })
    }
}

#[async_trait]
impl ClientStorage for BrowserStorage {
    async fn get(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>> {
        let (service, account) = (service.to_owned(), account.to_owned());
        self.actor
            .run(move |vaults| async move {
                let (db, store, id) = vaults.place(&service, &account).await?;
                let tx = Txn::begin(&db, &[store], Mode::Read)?;
                let value =
                    idb::done(&crate::rows::store(&tx, store)?.get(&id).map_err(js_error)?).await?;
                if value.is_undefined() {
                    return Ok(None);
                }
                let sealed = value
                    .dyn_into::<Uint8Array>()
                    .map_err(|_| anyhow!(StorageFailure::Corrupt))?
                    .to_vec();
                if sealed.len() < IV_BYTES {
                    return Err(anyhow!(StorageFailure::Corrupt));
                }
                let (iv, body) = sealed.split_at(IV_BYTES);
                let opened = JsFuture::from(
                    subtle()?
                        .decrypt_with_object_and_buffer_source(
                            &algorithm(iv, &aad(&service, &account))?,
                            &vaults.wrap,
                            &Uint8Array::from(body),
                        )
                        .map_err(js_error)?,
                )
                .await
                .map_err(|error| js_error_or(error, StorageFailure::Corrupt))?;
                Ok(Some(Uint8Array::new(&opened).to_vec()))
            })
            .await
            .with_context(|| "failed to read the browser storage")
    }

    async fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<()> {
        let (service, account, value) = (service.to_owned(), account.to_owned(), value.to_vec());
        self.actor
            .run(move |vaults| async move {
                let mut iv = [0u8; IV_BYTES];
                web_sys::window()
                    .ok_or_else(|| anyhow!(StorageFailure::Denied))?
                    .crypto()
                    .map_err(js_error)?
                    .get_random_values_with_u8_array(&mut iv)
                    .map_err(js_error)?;
                let body = JsFuture::from(
                    subtle()?
                        .encrypt_with_object_and_buffer_source(
                            &algorithm(&iv, &aad(&service, &account))?,
                            &vaults.wrap,
                            &Uint8Array::from(value.as_slice()),
                        )
                        .map_err(js_error)?,
                )
                .await
                .map_err(js_error)?;
                let mut sealed = iv.to_vec();
                sealed.extend(Uint8Array::new(&body).to_vec());
                let (db, store, id) = vaults.place(&service, &account).await?;
                let tx = Txn::begin(&db, &[store], Mode::Strict)?;
                crate::rows::store(&tx, store)?
                    .put_with_key(&Uint8Array::from(sealed.as_slice()), &id)
                    .map_err(js_error)?;
                tx.commit().await
            })
            .await
            .with_context(|| "failed to write the browser storage")
    }

    async fn delete(&self, service: &str, account: &str) -> Result<()> {
        let (service, account) = (service.to_owned(), account.to_owned());
        self.actor
            .run(move |vaults| async move {
                let (db, store, id) = vaults.place(&service, &account).await?;
                let tx = Txn::begin(&db, &[store], Mode::Strict)?;
                crate::rows::store(&tx, store)?
                    .delete(&id)
                    .map_err(js_error)?;
                tx.commit().await
            })
            .await
            .with_context(|| "failed to delete from the browser storage")
    }
}

#[cfg(test)]
mod tests {
    use super::vault_of;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn values_under_an_account_directory_go_to_its_vault() {
        let id = "0123456789abcdef";
        assert_eq!(
            vault_of(&format!("/kukuri/accounts/{id}/kukuri.identity-key")),
            Some(id)
        );
        assert_eq!(
            vault_of(&format!("db:/kukuri/accounts/{id}/kukuri.db:token:00")),
            Some(id)
        );
        assert_eq!(vault_of("/kukuri/accounts.json"), None);
        assert_eq!(
            vault_of("/kukuri/account-transitions/create-1/kukuri.db"),
            None
        );
        assert_eq!(vault_of(&format!("/kukuri/accounts/{id}")), None);
    }
}
