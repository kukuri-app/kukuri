use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{AssetRef, MediaOrigin};
use serde::{Deserialize, Serialize};

pub const MEDIA_BOOKMARK_PAGE_SIZE: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MediaBookmarkCursor {
    pub bookmarked_at: i64,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkedMediaRow {
    pub hash: String,
    pub source: MediaOrigin,
    pub attachments: Vec<AssetRef>,
    pub content_labels: Vec<String>,
    pub bookmarked_at: i64,
}

impl BookmarkedMediaRow {
    pub fn cache_refs(&self) -> Vec<(String, String)> {
        self.attachments
            .iter()
            .map(|asset| ("blob".into(), asset.hash.as_str().into()))
            .collect()
    }
}

#[async_trait]
pub trait MediaBookmarkStore: Send + Sync {
    async fn put_bookmarked_media(&self, row: BookmarkedMediaRow) -> Result<()>;
    async fn get_bookmarked_media(&self, hash: &str) -> Result<Option<BookmarkedMediaRow>>;
    async fn list_bookmarked_media(
        &self,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>>;
    async fn remove_bookmarked_media(&self, hash: &str) -> Result<()>;
    /// 自分の投稿として使ったblobを、通常のファイル添付と同じ所有参照へ移す。
    async fn retain_bookmarked_media(&self, row: &BookmarkedMediaRow) -> Result<()>;
}
