use crate::service::*;
use crate::views::*;
use kukuri_core::{AssetRole, MediaOrigin};
use kukuri_store::{BookmarkedMediaRow, MEDIA_BOOKMARK_PAGE_SIZE, MediaBookmarkCursor};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BookmarkedMediaView {
    pub hash: String,
    pub source: MediaOrigin,
    pub bookmarked_from_object_id: String,
    pub attachments: Vec<AttachmentView>,
    pub content_labels: Vec<String>,
    pub bookmarked_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BookmarkedMediaPageView {
    pub items: Vec<BookmarkedMediaView>,
    pub newer_cursor: Option<MediaBookmarkCursor>,
    pub older_cursor: Option<MediaBookmarkCursor>,
}

impl AppService {
    async fn bookmarked_media_view(&self, row: BookmarkedMediaRow) -> Result<BookmarkedMediaView> {
        Ok(BookmarkedMediaView {
            hash: row.hash,
            bookmarked_from_object_id: if row.bookmarked_from_object_id.is_empty() {
                row.source.object_id.clone()
            } else {
                row.bookmarked_from_object_id
            },
            source: row.source,
            content_labels: row.content_labels,
            bookmarked_at: row.bookmarked_at,
            attachments: attachment_views_from_refs(
                self.services.blob_service.as_ref(),
                &row.attachments,
            )
            .await?,
        })
    }

    pub async fn bookmark_media(
        &self,
        topic: &str,
        object_id: &str,
        hash: &str,
        channel_ref: ChannelRef,
    ) -> Result<BookmarkedMediaView> {
        self.ensure_object_projection(
            topic,
            &channel_ref.timeline_scope(),
            &EnvelopeId::from(object_id),
            DocFetchPolicy::LocalOnly,
        )
        .await?;
        let row = self
            .services
            .projection_store
            .get_object_projection(&EnvelopeId::from(object_id))
            .await?
            .context("media bookmark target was not found")?;
        anyhow::ensure!(
            row.topic_id == topic
                && ChannelRef::for_channel(channel_id_from_storage(&row.channel_id).as_ref())
                    == channel_ref,
            "media bookmark scope does not match"
        );
        anyhow::ensure!(
            matches!(row.object_kind.as_str(), "post" | "comment"),
            "media bookmark target must be a post"
        );
        let asset = row
            .attachments
            .iter()
            .find(|asset| asset.hash.as_str() == hash)
            .context("media bookmark attachment was not found")?;
        let video = matches!(
            asset.role,
            AssetRole::VideoManifest | AssetRole::VideoPoster
        );
        anyhow::ensure!(
            video || (asset.mime.starts_with("image/") && asset.role != AssetRole::VideoPoster),
            "media bookmark requires image or video"
        );
        let attachments: Vec<_> = if video {
            let ordinal = row
                .attachments
                .iter()
                .take_while(|item| item.hash.as_str() != hash)
                .filter(|item| item.role == asset.role)
                .count();
            let main = row
                .attachments
                .iter()
                .filter(|item| item.role == AssetRole::VideoManifest)
                .nth(ordinal)
                .context("video attachment was not found")?;
            let poster = row
                .attachments
                .iter()
                .filter(|item| item.role == AssetRole::VideoPoster)
                .nth(ordinal);
            std::iter::once(main).chain(poster).cloned().collect()
        } else {
            vec![asset.clone()]
        };
        let primary = attachments
            .iter()
            .find(|asset| asset.role == AssetRole::VideoManifest)
            .unwrap_or(&attachments[0]);
        let source = primary.origin.clone().unwrap_or(MediaOrigin {
            topic_id: row.topic_id,
            object_id: row.object_id.as_str().into(),
            author_pubkey: row.author_pubkey,
            created_at: row.created_at,
        });
        let saved = BookmarkedMediaRow {
            bookmarked_from_object_id: row.object_id.as_str().into(),
            hash: primary.hash.as_str().into(),
            source,
            attachments,
            content_labels: row.content_labels,
            bookmarked_at: Utc::now().timestamp_millis(),
        };
        self.services
            .projection_store
            .put_bookmarked_media(saved.clone())
            .await?;
        self.bookmarked_media_view(saved).await
    }

    pub async fn get_bookmarked_media(&self, hash: &str) -> Result<Option<BookmarkedMediaView>> {
        match self
            .services
            .projection_store
            .get_bookmarked_media(hash)
            .await?
        {
            Some(row) => self.bookmarked_media_view(row).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn remove_bookmarked_media(&self, hash: &str) -> Result<()> {
        self.services
            .projection_store
            .remove_bookmarked_media(hash)
            .await
    }

    pub async fn list_bookmarked_media(
        &self,
        cursor: Option<&MediaBookmarkCursor>,
        before: bool,
    ) -> Result<BookmarkedMediaPageView> {
        let mut rows = self
            .services
            .projection_store
            .list_bookmarked_media(cursor, before)
            .await?;
        let more = rows.len() > MEDIA_BOOKMARK_PAGE_SIZE;
        rows.truncate(MEDIA_BOOKMARK_PAGE_SIZE);
        if before {
            rows.reverse();
        }
        let at = |row: &BookmarkedMediaRow| MediaBookmarkCursor {
            bookmarked_at: row.bookmarked_at,
            hash: row.hash.clone(),
        };
        let newer_cursor = (if before { more } else { cursor.is_some() })
            .then(|| rows.first().map(at))
            .flatten();
        let older_cursor = (if before { cursor.is_some() } else { more })
            .then(|| rows.last().map(at))
            .flatten();
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(self.bookmarked_media_view(row).await?);
        }
        Ok(BookmarkedMediaPageView {
            items,
            newer_cursor,
            older_cursor,
        })
    }
}
