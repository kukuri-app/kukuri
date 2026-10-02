//! private channel の参加と、世代の鍵（ADR 0061 §9）。参加は channel ごと、鍵は (channel, epoch) ごとの行で、
//! 各操作は対象の行だけを読み書きする。世代の秘密は app-api が封をしてから渡す。native は `SqliteStore`、Web は
//! IndexedDB の保護行が実装する（W4 AC-2）。

use anyhow::Result;
use async_trait::async_trait;

/// 参加の行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateChannelRow {
    /// `<topic id>::<channel id>`。一覧・巡回はこの順。
    pub channel_key: String,
    pub topic_id: String,
    pub channel_id: String,
    pub label: String,
    pub creator_pubkey: String,
    pub owner_pubkey: String,
    pub joined_via_pubkey: Option<String>,
    /// `ChannelAudienceKind` の serde の名前。
    pub audience_kind: String,
    pub current_epoch_id: String,
    /// 鍵更新の担当の記録（JSON。ADR 0018 §8）。
    pub controller: Option<String>,
    /// false は退会の tombstone。
    pub joined: bool,
    pub updated_at: i64,
    pub op_id: String,
}

/// 世代の鍵の行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateChannelEpochRow {
    pub channel_id: String,
    pub epoch_id: String,
    /// 世代の開始時刻（epoch id の時刻。`legacy` は最小）。
    pub started_at: i64,
    /// 受信 route の識別子（`receive_epoch_key_id`）。
    pub receive_key_id: String,
    /// 鍵を受け取った時刻。
    pub updated_at: i64,
    pub sealed_secret: Vec<u8>,
}

/// 参加中の行の一覧の絞り込み。どれも索引の範囲で読む(全件を走査しない)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateChannelFilter<'a> {
    All,
    Topic(&'a str),
    Owner(&'a str),
}

/// 開始時刻の索引で読む向き。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateChannelEpochRange {
    /// この時刻以前に始まった世代を、新しい順に。
    AtOrBefore(i64),
    /// この時刻より後に始まった世代を、古い順に。
    After(i64),
}

#[async_trait]
pub trait PrivateChannelKeyStore: Send + Sync {
    /// 参加の行を置き換え、世代の鍵の行を足す（既にある鍵の行は置き換えない）。1 transaction で書く。
    async fn put_private_channel(
        &self,
        row: &PrivateChannelRow,
        epochs: &[PrivateChannelEpochRow],
    ) -> Result<()>;
    /// 世代の鍵の行だけを足す（参加の行の無い channel の鍵を受け取ったとき）。既にあれば何もしない。足したら true。
    async fn put_private_channel_epoch(&self, epoch: &PrivateChannelEpochRow) -> Result<bool>;
    async fn get_private_channel(&self, channel_key: &str) -> Result<Option<PrivateChannelRow>>;
    /// channel id で引く参加の行（世代の鍵の行から参加を引くとき）。
    async fn get_private_channel_by_id(
        &self,
        channel_id: &str,
    ) -> Result<Option<PrivateChannelRow>>;
    /// 参加中の行を `channel_key` の順に、`after` より後から `limit` 件。
    async fn list_joined_private_channels(
        &self,
        filter: PrivateChannelFilter<'_>,
        after: &str,
        limit: usize,
    ) -> Result<Vec<PrivateChannelRow>>;
    async fn get_private_channel_epoch(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>>;
    async fn find_private_channel_epoch(
        &self,
        receive_key_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>>;
    async fn list_private_channel_epochs(
        &self,
        channel_id: &str,
        range: PrivateChannelEpochRange,
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>>;
    /// その channel の世代の鍵の行を `limit` 件まで消し、消した世代の ID を返す。
    async fn delete_private_channel_epochs(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<String>>;
}
