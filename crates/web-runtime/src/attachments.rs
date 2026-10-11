//! File/Blobは明示invokeの間だけ保持する。永続stagingを作らず、終了・切替・失敗で参照を捨てる。
use anyhow::{Context, Result, anyhow, ensure};
use async_trait::async_trait;
use js_sys::{Array, Reflect, Uint8Array};
use kukuri_desktop_runtime::{
    ClientHost, WebAttachmentInput, WebAttachmentRef, install_web_attachment_input,
};
use std::{cell::RefCell, collections::HashMap, sync::Arc};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::Blob;

const CHUNK: usize = 1024 * 1024;
#[derive(Clone)]
struct Input {
    blob: Blob,
    reference: WebAttachmentRef,
    host: Arc<ClientHost>,
    generation: u64,
}
thread_local! { static INPUTS: RefCell<HashMap<String, Input>> = RefCell::new(HashMap::new()); }

fn active(host: &ClientHost, generation: u64) -> Result<()> {
    ensure!(
        !host.is_stopped() && host.generation() == generation,
        "attachment account operation ended"
    );
    Ok(())
}

async fn chunks(
    blob: &Blob,
    check: impl Fn() -> Result<()>,
    mut consume: impl FnMut(&[u8]),
) -> Result<()> {
    let size = blob.size() as usize;
    for offset in (0..size).step_by(CHUNK) {
        check()?;
        let part = blob
            .slice_with_f64_and_f64(offset as f64, (offset + CHUNK).min(size) as f64)
            .map_err(|error| anyhow!("failed to slice attachment: {error:?}"))?;
        let value = JsFuture::from(part.array_buffer())
            .await
            .map_err(|error| anyhow!("failed to read attachment: {error:?}"))?;
        check()?;
        consume(&Uint8Array::new(&value).to_vec());
    }
    check()
}

pub(crate) async fn fingerprint(blob: Blob, host: Arc<ClientHost>) -> Result<serde_json::Value> {
    let generation = host.generation();
    let owner = host.runtime().local_author_pubkey();
    let mut hash = blake3::Hasher::new();
    chunks(
        &blob,
        || active(&host, generation),
        |bytes| {
            hash.update(bytes);
        },
    )
    .await?;
    Ok(serde_json::json!({ "owner": owner, "hash": hash.finalize().to_hex().to_string() }))
}

pub(crate) struct Inputs(Vec<String>);
impl Inputs {
    pub(crate) fn register(sources: Array, host: Arc<ClientHost>) -> Result<Self> {
        let mut guard = Self(Vec::new());
        let generation = host.generation();
        let owner = host.runtime().local_author_pubkey();
        for value in sources.iter() {
            let raw = Reflect::get(&value, &JsValue::from_str("reference"))
                .map_err(|error| anyhow!("attachment reference: {error:?}"))?;
            let json = js_sys::JSON::stringify(&raw)
                .map_err(|error| anyhow!("attachment reference: {error:?}"))?;
            let reference: WebAttachmentRef = serde_json::from_str(&String::from(json))?;
            ensure!(
                reference.owner == owner,
                "attachment belongs to another account"
            );
            let blob = Reflect::get(&value, &JsValue::from_str("blob"))
                .map_err(|error| anyhow!("attachment Blob: {error:?}"))?
                .dyn_into::<Blob>()
                .map_err(|_| anyhow!("attachment Blob is missing"))?;
            INPUTS.with_borrow_mut(|inputs| {
                inputs.insert(
                    reference.id.clone(),
                    Input {
                        blob,
                        reference: reference.clone(),
                        host: host.clone(),
                        generation,
                    },
                )
            });
            guard.0.push(reference.id);
        }
        Ok(guard)
    }
}
impl Drop for Inputs {
    fn drop(&mut self) {
        INPUTS.with_borrow_mut(|inputs| {
            for id in &self.0 {
                inputs.remove(id);
            }
        });
    }
}

struct Reader;
#[async_trait]
impl WebAttachmentInput for Reader {
    async fn read(&self, source: &WebAttachmentRef) -> Result<Vec<u8>> {
        let source = source.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // JS objectは既存runtimeと同じmain threadだけで読み、Sendな呼出元へbytesを返す。
        n0_future::task::spawn(async move {
            let result = async {
                let input = INPUTS
                    .with_borrow(|inputs| inputs.get(&source.id).cloned())
                    .context("attachment operation ended")?;
                ensure!(
                    input.reference == source,
                    "attachment reference does not match its operation"
                );
                let mut bytes = Vec::new();
                let mut hash = blake3::Hasher::new();
                chunks(
                    &input.blob,
                    || {
                        ensure!(
                            INPUTS.with_borrow(|inputs| inputs.contains_key(&source.id)),
                            "attachment operation ended"
                        );
                        active(&input.host, input.generation)
                    },
                    |chunk| {
                        bytes.extend_from_slice(chunk);
                        hash.update(chunk);
                    },
                )
                .await?;
                ensure!(
                    hash.finalize().to_hex().as_str() == source.hash,
                    "attachment original hash changed"
                );
                Ok(bytes)
            }
            .await;
            let _ = sender.send(result);
        });
        receiver.await.context("attachment reader stopped")?
    }
}
pub(crate) fn install() {
    install_web_attachment_input(&Reader);
}
