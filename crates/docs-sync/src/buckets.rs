//! 版付きの時間bucket識別子（ADR 0054）。時計をここで読まず、操作開始時の時刻を渡す。

use anyhow::{Result, bail, ensure};
use kukuri_core::ReplicaId;

pub const BUCKET_SECONDS_V1: u64 = 86_400;
const MAX_SCOPE_BYTES: usize = 1_024;
const MAX_REPLICA_BYTES: usize = 4 * MAX_SCOPE_BYTES + 96;
const PRIVATE_KEY_CONTEXT: &[u8] = b"kukuri.app docs bucket namespace v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeBucket(u64);

impl TimeBucket {
    pub fn from_unix_seconds(seconds: i64) -> Result<Self> {
        ensure!(seconds >= 0, "bucket timestamp must not be negative");
        Self::from_index(seconds as u64 / BUCKET_SECONDS_V1)
    }

    pub fn from_index(index: u64) -> Result<Self> {
        ensure!(
            index <= i64::MAX as u64 / BUCKET_SECONDS_V1,
            "bucket index exceeds the timestamp range"
        );
        Ok(Self(index))
    }

    pub fn index(self) -> u64 {
        self.0
    }

    pub fn start_seconds(self) -> u64 {
        self.0 * BUCKET_SECONDS_V1
    }

    pub fn end_seconds(self) -> u64 {
        (self.0 + 1) * BUCKET_SECONDS_V1
    }

    pub fn previous(self) -> Option<Self> {
        self.0.checked_sub(1).map(Self)
    }

    /// 全履歴を列挙せず、現在と直前の候補だけを返す。実際のsync許可は作業集合が決める。
    pub fn live_window(self) -> impl Iterator<Item = Self> {
        [Some(self), self.previous()].into_iter().flatten()
    }

    pub fn contains(self, unix_seconds: i64) -> bool {
        unix_seconds >= 0
            && (self.start_seconds()..self.end_seconds()).contains(&(unix_seconds as u64))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BucketScope {
    Topic {
        topic_id: String,
    },
    PrivateChannel {
        channel_id: String,
        epoch_id: String,
    },
    Author {
        author_pubkey: String,
    },
}

impl BucketScope {
    fn validate(&self) -> Result<()> {
        fn part(value: &str) -> Result<()> {
            ensure!(
                !value.is_empty() && value.len() <= MAX_SCOPE_BYTES,
                "bucket scope must contain 1..=1024 UTF-8 bytes"
            );
            Ok(())
        }
        match self {
            Self::Topic { topic_id } => part(topic_id),
            Self::Author { author_pubkey } => part(author_pubkey),
            Self::PrivateChannel {
                channel_id,
                epoch_id,
            } => {
                part(channel_id)?;
                part(epoch_id)
            }
        }
    }
}

/// 検証済みのv1 replica。生文字列のprefix判定だけでprivate/publicを決めない。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BucketReplica {
    scope: BucketScope,
    bucket: TimeBucket,
}

impl BucketReplica {
    pub fn new(scope: BucketScope, bucket: TimeBucket) -> Result<Self> {
        scope.validate()?;
        Ok(Self { scope, bucket })
    }

    pub fn scope(&self) -> &BucketScope {
        &self.scope
    }

    pub fn bucket(&self) -> TimeBucket {
        self.bucket
    }

    pub fn is_private(&self) -> bool {
        matches!(self.scope, BucketScope::PrivateChannel { .. })
    }

    pub fn replica_id(&self) -> ReplicaId {
        let scope = match &self.scope {
            BucketScope::Topic { topic_id } => format!("topic::{}", hex::encode(topic_id)),
            BucketScope::Author { author_pubkey } => {
                format!("author::{}", hex::encode(author_pubkey))
            }
            BucketScope::PrivateChannel {
                channel_id,
                epoch_id,
            } => {
                format!(
                    "channel::{}::{}",
                    hex::encode(channel_id),
                    hex::encode(epoch_id)
                )
            }
        };
        ReplicaId::new(format!("bucket::v1::{scope}::{}", self.bucket.index()))
    }

    pub fn parse(replica_id: &ReplicaId) -> Result<Self> {
        let raw = replica_id.as_str();
        ensure!(
            raw.len() <= MAX_REPLICA_BYTES,
            "bucket replica id is too long"
        );
        let Some(rest) = raw.strip_prefix("bucket::v1::") else {
            bail!("unsupported bucket replica version");
        };
        let parts: Vec<_> = rest.split("::").collect();
        let decode = |value: &str| -> Result<String> {
            ensure!(
                !value.is_empty()
                    && value.len() <= MAX_SCOPE_BYTES * 2
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "bucket scope is not canonical hexadecimal"
            );
            Ok(String::from_utf8(hex::decode(value)?)?)
        };
        let (scope, index) = match parts.as_slice() {
            ["topic", topic, index] => (
                BucketScope::Topic {
                    topic_id: decode(topic)?,
                },
                *index,
            ),
            ["author", author, index] => (
                BucketScope::Author {
                    author_pubkey: decode(author)?,
                },
                *index,
            ),
            ["channel", channel, epoch, index] => (
                BucketScope::PrivateChannel {
                    channel_id: decode(channel)?,
                    epoch_id: decode(epoch)?,
                },
                *index,
            ),
            _ => bail!("invalid bucket replica shape"),
        };
        let bucket = TimeBucket::from_index(index.parse()?)?;
        ensure!(
            index == bucket.index().to_string(),
            "bucket index is not canonical"
        );
        Self::new(scope, bucket)
    }

    /// epochの元secretを持つ呼び出し元だけが導出する。返り値をログやwireへ出さない。
    /// IDはcapabilityの証明ではない。epochの許可判定は登録より前に行う。
    pub fn derive_private_secret(&self, epoch_secret: &[u8; 32]) -> Result<[u8; 32]> {
        ensure!(
            self.is_private(),
            "only private buckets derive epoch secrets"
        );
        let mut hash = blake3::Hasher::new_keyed(epoch_secret);
        hash.update(PRIVATE_KEY_CONTEXT);
        hash.update(self.replica_id().as_str().as_bytes());
        Ok(*hash.finalize().as_bytes())
    }
}
