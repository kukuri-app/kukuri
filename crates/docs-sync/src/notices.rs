//! replica の購読の stream(#1239)。entry の event と通知を、同じ上限つきの buffer から作る。

use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::types::{DocEventStream, ReplicaNotice, ReplicaNoticeStream};

/// entry の event だけを流す(取りこぼしと同期の区切りは流さない。`subscribe_replica` の従来の挙動)。
pub(crate) fn entry_stream(receiver: broadcast::Receiver<ReplicaNotice>) -> DocEventStream {
    Box::pin(futures_util::StreamExt::filter_map(
        BroadcastStream::new(receiver),
        |item| async move {
            match item {
                Ok(ReplicaNotice::Entry(event)) => Some(Ok(event)),
                _ => None,
            }
        },
    ))
}

/// 通知をすべて流す。buffer が溢れて受け取れなかった分は、`Lagged` として知らせる。
pub(crate) fn notice_stream(receiver: broadcast::Receiver<ReplicaNotice>) -> ReplicaNoticeStream {
    Box::pin(futures_util::StreamExt::map(
        BroadcastStream::new(receiver),
        |item| {
            Ok(match item {
                Ok(notice) => notice,
                Err(BroadcastStreamRecvError::Lagged(missed)) => ReplicaNotice::Lagged { missed },
            })
        },
    ))
}
