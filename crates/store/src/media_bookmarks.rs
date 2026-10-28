use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{AssetRef, AssetRole, MediaOrigin};
use serde::{Deserialize, Serialize};

pub const MEDIA_BOOKMARK_PAGE_SIZE: usize = 20;

/// 画像・動画の一覧の種類。ブックマークと、表示のために手元へ置いた他人の画像・動画(#1949)。
/// 同じ画像・動画はどちらか一方にだけ載る(ブックマークするとキャッシュから外れ、外すと戻る)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MediaShelf {
    #[default]
    Bookmarks,
    Cache,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MediaBookmarkCursor {
    pub bookmarked_at: i64,
    pub hash: String,
}

/// 一覧の 1 行。キャッシュの行では `bookmarked_at` は台帳へ載せた時刻、`bookmarked_from_object_id` は表示した投稿。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkedMediaRow {
    pub hash: String,
    pub source: MediaOrigin,
    #[serde(default)]
    pub bookmarked_from_object_id: String,
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

    /// 動画の poster の hash。キャッシュの台帳は主 hash と poster のどちらが回収されても行を外す。
    pub fn poster_hash(&self) -> Option<&str> {
        self.attachments
            .iter()
            .find(|asset| asset.role == AssetRole::VideoPoster)
            .map(|asset| asset.hash.as_str())
    }
}

#[async_trait]
pub trait MediaBookmarkStore: Send + Sync {
    /// ブックマークへ載せ、キャッシュの一覧から外す。
    async fn put_bookmarked_media(&self, row: BookmarkedMediaRow) -> Result<()>;
    async fn get_bookmarked_media(&self, hash: &str) -> Result<Option<BookmarkedMediaRow>>;
    async fn list_bookmarked_media(
        &self,
        shelf: MediaShelf,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<Vec<BookmarkedMediaRow>>;
    /// ブックマークを外す。保護の無い部分が cache に残れば、キャッシュの一覧へ戻す。
    async fn remove_bookmarked_media(&self, hash: &str) -> Result<()>;
    /// 自分の投稿として使ったblobを、通常のファイル添付と同じ所有参照へ移す。
    async fn retain_bookmarked_media(&self, row: &BookmarkedMediaRow) -> Result<()>;
    /// 表示のために手元へ置いた画像・動画をキャッシュの一覧へ載せる(#1949)。保護の無い部分が cache に無いもの
    /// (自分の投稿の添付など)とブックマーク済みは載せない。載っていれば変えない。
    async fn record_cached_media(&self, row: BookmarkedMediaRow) -> Result<()>;
    /// キャッシュの一覧から外し、保護の無い部分を cache から消す。
    async fn remove_cached_media(&self, hash: &str) -> Result<()>;
}
