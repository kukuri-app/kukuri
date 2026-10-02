//! JS の object を持つ状態（IndexedDB の接続・`CryptoKey`）を 1 つの task に置き、操作を channel で受けて 1 つずつ
//! 行う（ADR 0056 §4。native の `remote_cache_gate` と同じ直列化）。呼出元の future は `Send` のまま待てる。

use std::future::Future;
use std::pin::Pin;

use anyhow::{Result, anyhow};
use tokio::sync::{mpsc, oneshot};

/// 処理待ちの操作の上限。超えた呼出元は空くまで待つ。
const QUEUE: usize = 64;

type Job<S> = Box<dyn FnOnce(S) -> Pin<Box<dyn Future<Output = ()>>> + Send>;

pub(crate) struct Actor<S> {
    jobs: mpsc::Sender<Job<S>>,
}

impl<S: Clone + 'static> Actor<S> {
    /// task を起こし、`open` で状態を作る。`open` の返す値（`T`）を呼出元へ返す。drop で `close` を呼んで止まる。
    pub(crate) async fn start<T, F, Fut>(open: F, close: fn(&S)) -> Result<(Self, T)>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(S, T)>> + 'static,
    {
        let (jobs, mut queue) = mpsc::channel::<Job<S>>(QUEUE);
        let (opened, receiver) = oneshot::channel();
        n0_future::task::spawn(async move {
            let state = match open().await {
                Ok((state, value)) => {
                    let _ = opened.send(Ok(value));
                    state
                }
                Err(error) => {
                    let _ = opened.send(Err(error));
                    return;
                }
            };
            while let Some(job) = queue.recv().await {
                job(state.clone()).await;
            }
            close(&state);
        });
        let value = receiver
            .await
            .map_err(|_| anyhow!("the browser storage task stopped"))??;
        Ok((Self { jobs }, value))
    }

    /// 操作を task で行う。呼出元が待つのをやめても、始めた transaction は最後まで進む。
    pub(crate) async fn run<T, F, Fut>(&self, job: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(S) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let job: Job<S> = Box::new(move |state| {
            Box::pin(async move {
                let _ = sender.send(job(state).await);
            })
        });
        let closed = || anyhow!("the browser storage is closed");
        self.jobs.send(job).await.map_err(|_| closed())?;
        receiver.await.map_err(|_| closed())?
    }
}
