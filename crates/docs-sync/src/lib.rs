mod access;
mod iroh_sync;
mod keys;
mod memory;

mod notices;
mod replicas;
#[cfg(test)]
mod tests;
mod time_index;
mod types;

pub use iroh_sync::IrohDocsSync;
pub use keys::SharedReplicaKeyFamily;
pub use memory::MemoryDocsSync;

pub use replicas::{
    PostReplicaKind, author_replica_id, device_replica_id, post_replica_kind,
    private_channel_epoch_replica_id, private_channel_hint_topic, private_channel_replica_id,
    stable_key, topic_replica_id, value_hash,
};
pub use time_index::{
    TimeIndexCursor, TimeIndexEntry, TimeIndexPage, query_time_index_asc, query_time_index_desc,
    query_time_index_desc_by_author, query_time_index_window,
};
pub use types::{
    DocEvent, DocEventStream, DocFetchPolicy, DocKeyEntry, DocKeyOrder, DocKeyPage, DocKeyQuery,
    DocOp, DocQuery, DocRecord, DocsSync, ReplicaNotice, ReplicaNoticeStream,
};
