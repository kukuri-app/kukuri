use crate::DesktopRuntime;
use anyhow::Result;
use kukuri_app_api::{BookmarkedMediaPageView, BookmarkedMediaView};
use kukuri_core::ChannelRef;
use kukuri_store::{MediaBookmarkCursor, MediaShelf};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BookmarkMediaRequest {
    pub topic: String,
    pub object_id: String,
    pub hash: String,
    pub channel_ref: ChannelRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MediaBookmarkRequest {
    pub hash: String,
    /// 外す一覧（#1949）。省略はブックマーク。取得（`get_bookmarked_media`）はブックマークだけを読む。
    #[serde(default)]
    pub shelf: MediaShelf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ListBookmarkedMediaRequest {
    #[serde(default)]
    pub cursor: Option<MediaBookmarkCursor>,
    #[serde(default)]
    pub before: bool,
    /// 読む一覧（#1949）。省略はブックマーク。
    #[serde(default)]
    pub shelf: MediaShelf,
}

impl DesktopRuntime {
    pub async fn bookmark_media(
        &self,
        request: BookmarkMediaRequest,
    ) -> Result<BookmarkedMediaView> {
        self.app_service
            .bookmark_media(
                &request.topic,
                &request.object_id,
                &request.hash,
                request.channel_ref,
            )
            .await
    }
    pub async fn get_bookmarked_media(
        &self,
        request: MediaBookmarkRequest,
    ) -> Result<Option<BookmarkedMediaView>> {
        self.app_service.get_bookmarked_media(&request.hash).await
    }
    pub async fn list_bookmarked_media(
        &self,
        request: ListBookmarkedMediaRequest,
    ) -> Result<BookmarkedMediaPageView> {
        self.app_service
            .list_bookmarked_media(request.shelf, request.cursor.as_ref(), request.before)
            .await
    }
    pub async fn remove_bookmarked_media(&self, request: MediaBookmarkRequest) -> Result<()> {
        self.app_service
            .remove_bookmarked_media(request.shelf, &request.hash)
            .await
    }
}
