//! passphrase の鍵の導出（argon2id、64 MiB・3 回。ADR 0047 §1）を、command の処理と画面を止めずに行う。
//! native は blocking の thread、Web は同じ wasm を読む Dedicated Worker（ADR 0056 §1 の例外）で行う。

use anyhow::Result;
use kukuri_core::PassphraseKdf;

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn derive_passphrase_key(input: PassphraseKdf) -> Result<[u8; 32]> {
    tokio::task::spawn_blocking(move || input.derive()).await?
}

/// 導出ごとに Worker を作り、結果を受けたら止める。Worker には main thread の `WebAssembly.Module` を渡し、
/// glue を使わずに instantiate して `kukuri_kdf_derive` だけを呼ぶ（`kdf_worker.js`）。JS の object は 1 つの task の
/// 中だけで持つ（ADR 0056 §4）。
#[cfg(target_family = "wasm")]
pub(crate) async fn derive_passphrase_key(input: PassphraseKdf) -> Result<[u8; 32]> {
    use anyhow::{Context as _, anyhow};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let js = |error: JsValue| anyhow!("{error:?}");
    let (sender, receiver) = tokio::sync::oneshot::channel();
    n0_future::task::spawn(async move {
        let result = async {
            let worker =
                web_sys::Worker::new(&wasm_bindgen::link_to!(module = "/src/kdf_worker.js"))
                    .map_err(js)?;
            let reply = js_sys::Promise::new(&mut |resolve, reject| {
                worker.set_onmessage(Some(&resolve));
                worker.set_onerror(Some(&reject));
            });
            let message = js_sys::Array::new();
            message.push(&wasm_bindgen::module());
            message.push(&js_sys::Uint8Array::from(input.passphrase.as_slice()));
            message.push(&js_sys::Uint8Array::from(input.salt.as_slice()));
            for cost in [input.m_cost_kib, input.t_cost, input.p_cost] {
                message.push(&cost.into());
            }
            let reply = match worker.post_message(&message) {
                Ok(()) => JsFuture::from(reply).await,
                Err(error) => Err(error),
            };
            worker.terminate();
            let data = reply
                .map_err(js)?
                .unchecked_into::<web_sys::MessageEvent>()
                .data();
            let key = data
                .dyn_into::<js_sys::Uint8Array>()
                .map_err(|error| anyhow!("{}", error.as_string().unwrap_or_default()))?;
            <[u8; 32]>::try_from(key.to_vec())
                .map_err(|_| anyhow!("the derived key is not 32 bytes"))
        }
        .await;
        let _ = sender.send(result);
    });
    receiver.await.context("the key derivation task stopped")?
}

/// Worker の確保。`kukuri_kdf_derive` へ渡す領域を返す（Worker は導出ごとに止めるので解放しない）。
#[cfg(target_family = "wasm")]
#[unsafe(no_mangle)]
pub extern "C" fn kukuri_kdf_alloc(len: usize) -> *mut u8 {
    Box::leak(vec![0u8; len].into_boxed_slice()).as_mut_ptr()
}

/// Worker の導出。`buffer` には passphrase・salt・導出した鍵（32 bytes）の順に並ぶ。成功は 0。
///
/// # Safety
/// `buffer` は `kukuri_kdf_alloc(passphrase_len + salt_len + 32)` の領域であること。
#[cfg(target_family = "wasm")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kukuri_kdf_derive(
    buffer: *mut u8,
    passphrase_len: usize,
    salt_len: usize,
    m_cost_kib: u32,
    t_cost: u32,
    p_cost: u32,
) -> u32 {
    // SAFETY: 関数の前提のとおり、`buffer` は長さ分を確保した領域。
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer, passphrase_len + salt_len + 32) };
    let (input, key) = buffer.split_at_mut(passphrase_len + salt_len);
    let (passphrase, salt) = input.split_at(passphrase_len);
    let derived = PassphraseKdf {
        passphrase: passphrase.to_vec(),
        salt: salt.to_vec(),
        m_cost_kib,
        t_cost,
        p_cost,
    }
    .derive();
    match derived {
        Ok(derived) => {
            key.copy_from_slice(&derived);
            0
        }
        Err(_) => 1,
    }
}
