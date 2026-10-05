//! IndexedDB の request と transaction を future で待つ。JS の object は呼んだ task の中だけで持つ（ADR 0056 §4）。
//!
//! 結果は success の event の callback で channel へ送り、待つ task は同じ event の後の microtask で再開する。
//! transaction はその間も有効なので、読んだ結果に続けて同じ transaction へ request を出せる。
//! 失敗は `StorageFailure` で区別して返す（ADR 0059 §1）。

use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use js_sys::{Array, Function, Object, Reflect};
use tokio::sync::oneshot;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::{Closure, JsValue};
use web_sys::{
    DomException, IdbCursorWithValue, IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransaction,
};

/// 保存の失敗の区別（ADR 0059 §1）。呼出元は `anyhow::Error::downcast_ref` で引く。どれも既存の値を作り直す理由にしない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageFailure {
    /// 容量の超過。transaction は何も残していない。
    Quota,
    /// IndexedDB が使えない（無効にされた、private の制限、開けない）。
    Denied,
    /// 読んだ値を復号できない、形が合わない。
    Corrupt,
    /// database の版が合わない（この版より新しい、更新が別の接続で止まっている）。
    Upgrade,
    /// transaction が中断した（部分的な保存は何も残さない）。
    Interrupted,
}

impl std::fmt::Display for StorageFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Quota => "the browser storage quota is exceeded",
            Self::Denied => "the browser storage is unavailable",
            Self::Corrupt => "the browser storage holds an unreadable value",
            Self::Upgrade => "the browser storage has another schema version",
            Self::Interrupted => "the browser storage transaction was aborted",
        })
    }
}

impl std::error::Error for StorageFailure {}

/// JS の失敗を区別して返す。DOMException の名前で分け、名前で決まらなければ transaction の中断とする。
pub(crate) fn js_error(value: JsValue) -> anyhow::Error {
    js_error_or(value, StorageFailure::Interrupted)
}

/// 名前で決まらない失敗を `fallback`（開けない database は `Denied`、復号できない値は `Corrupt`）とする。
pub(crate) fn js_error_or(value: JsValue, fallback: StorageFailure) -> anyhow::Error {
    let failure = match value.dyn_ref::<DomException>().map(DomException::name) {
        Some(name) if name == "QuotaExceededError" => StorageFailure::Quota,
        Some(name) if name == "VersionError" => StorageFailure::Upgrade,
        Some(name) if name == "SecurityError" => StorageFailure::Denied,
        _ => fallback,
    };
    anyhow::Error::new(failure).context(format!("indexeddb: {value:?}"))
}

/// 成否の 2 つの callback を付け、どちらかが呼ばれるまで待つ。
async fn settle(
    set: impl Fn(Option<&js_sys::Function>, Option<&js_sys::Function>),
) -> Result<bool> {
    let (sender, receiver) = oneshot::channel();
    let sender = Rc::new(Cell::new(Some(sender)));
    let callback = |ok: bool| {
        let sender = sender.clone();
        Closure::<dyn FnMut()>::new(move || {
            if let Some(sender) = sender.take() {
                let _ = sender.send(ok);
            }
        })
    };
    let (success, failure) = (callback(true), callback(false));
    set(
        Some(success.as_ref().unchecked_ref()),
        Some(failure.as_ref().unchecked_ref()),
    );
    let ok = receiver.await;
    set(None, None);
    ok.map_err(|_| anyhow!("indexeddb callback dropped"))
}

/// request の結果。失敗した request は transaction を中断させる（既定の動作）。
pub(crate) async fn done(request: &IdbRequest) -> Result<JsValue> {
    done_or(request, StorageFailure::Interrupted).await
}

/// request の結果。名前で決まらない失敗は `fallback` とする。
async fn done_or(request: &IdbRequest, fallback: StorageFailure) -> Result<JsValue> {
    let ok = settle(|success, failure| {
        request.set_onsuccess(success);
        request.set_onerror(failure);
    })
    .await?;
    let error = |value| js_error_or(value, fallback);
    if ok {
        request.result().map_err(error)
    } else {
        let value = request.error().map_err(error)?;
        Err(error(value.map(JsValue::from).unwrap_or(JsValue::NULL)))
    }
}

/// transaction の確定を待つ。中断（quota 超過・request の失敗）では何も書かれていない。
pub(crate) async fn committed(transaction: &IdbTransaction) -> Result<()> {
    let ok = settle(|success, failure| {
        transaction.set_oncomplete(success);
        transaction.set_onabort(failure);
    })
    .await?;
    if ok {
        Ok(())
    } else {
        Err(js_error(
            transaction
                .error()
                .map(JsValue::from)
                .unwrap_or(JsValue::NULL),
        ))
    }
}

/// cursor の次の行。`continue_` の後も同じ request が結果を返す。
pub(crate) async fn next(request: &IdbRequest) -> Result<Option<IdbCursorWithValue>> {
    let value = done(request).await?;
    Ok((!value.is_null()).then(|| value.unchecked_into()))
}

/// transaction の種類。`Strict` は OS の記録まで待つ readwrite（鍵・設定・private channel の行。ADR 0059 §1）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Read,
    Write,
    Strict,
}

/// `stores` にまたがる transaction を始める。durability の指定は web-sys では unstable なので、JS の関数として呼ぶ。
pub(crate) fn transaction(db: &IdbDatabase, stores: &[&str], mode: Mode) -> Result<IdbTransaction> {
    let names: Array = stores.iter().map(|name| JsValue::from_str(name)).collect();
    let options = Object::new();
    if mode == Mode::Strict {
        Reflect::set(&options, &"durability".into(), &"strict".into()).map_err(js_error)?;
    }
    let mode = if mode == Mode::Read {
        "readonly"
    } else {
        "readwrite"
    };
    let begin: Function = Reflect::get(db, &"transaction".into())
        .map_err(js_error)?
        .unchecked_into();
    Ok(begin
        .call3(db, &names, &mode.into(), &options)
        .map_err(js_error)?
        .unchecked_into())
}

/// database を開き、版が上がるときは `upgrade` で object store と索引を作る。IndexedDB が無い・開けないときは
/// `Denied`、版が合わない・別の接続が更新を止めているときは `Upgrade` を返す。
pub(crate) async fn open(
    name: &str,
    version: u32,
    upgrade: fn(&IdbDatabase) -> std::result::Result<(), JsValue>,
) -> Result<IdbDatabase> {
    let denied = |error: JsValue| js_error_or(error, StorageFailure::Denied);
    let factory = web_sys::window()
        .ok_or_else(|| anyhow!(StorageFailure::Denied))?
        .indexed_db()
        .map_err(denied)?
        .ok_or_else(|| anyhow!(StorageFailure::Denied))?;
    let request: IdbOpenDbRequest = factory.open_with_u32(name, version).map_err(denied)?;
    let upgrading = request.clone();
    let on_upgrade = Closure::<dyn FnMut()>::new(move || {
        let created = upgrading
            .result()
            .and_then(|db| upgrade(&db.unchecked_into()));
        if created.is_err()
            && let Some(transaction) = upgrading.transaction()
        {
            let _ = transaction.abort();
        }
    });
    let (blocked, on_blocked) = oneshot::channel::<()>();
    let blocked = Cell::new(Some(blocked));
    let on_block = Closure::<dyn FnMut()>::new(move || {
        if let Some(blocked) = blocked.take() {
            let _ = blocked.send(());
        }
    });
    request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
    request.set_onblocked(Some(on_block.as_ref().unchecked_ref()));
    let db = n0_future::future::or(
        async { Some(done_or(&request, StorageFailure::Denied).await) },
        async {
            let _ = on_blocked.await;
            None
        },
    )
    .await;
    request.set_onupgradeneeded(None);
    request.set_onblocked(None);
    match db {
        Some(Ok(db)) => Ok(db.unchecked_into()),
        Some(Err(error)) => Err(error),
        None => Err(anyhow!(StorageFailure::Upgrade).context(format!(
            "upgrading `{name}` is blocked by another connection"
        ))),
    }
}
