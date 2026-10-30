//! 投稿の行の保存(他人の投稿と自分の投稿で同じ関数を通す)。新しい行は `post_changes` で知らせる(#1981)。

use super::*;

impl ServiceHandles {
    pub(crate) async fn put_post_projection(&self, row: ObjectProjectionRow) -> Result<()> {
        let me = self.keys.public_key_hex();
        let new = self
            .projection_store
            .get_object_projection(&row.object_id)
            .await?
            .is_none();
        let change = new.then(|| row.clone());
        // 公開 channel(#1840)の投稿は、その channel の一覧の行の最後の投稿の時刻を進める。非表示の作者(mute と、
        // どちらの向きでも有効な block の相手)の投稿は数えない。
        let public_channel_post = ChannelId::new(row.channel_id.as_str())
            .is_public_channel()
            .then(|| {
                (
                    row.channel_id.clone(),
                    row.created_at,
                    row.author_pubkey.clone(),
                )
            });
        if row.author_pubkey == me {
            self.projection_store.put_object_projection(row).await?;
        } else {
            self.projection_store
                .put_remote_object_projection(row)
                .await?;
        }
        if let Some(change) = change {
            let _ = self.post_changes.send(change);
        }
        let Some((channel_id, created_at, author)) = public_channel_post else {
            return Ok(());
        };
        let hidden = author != me
            && (self
                .projection_store
                .get_muted_author(&author)
                .await?
                .is_some()
                || self.blocks_either_direction(&me, &author).await?);
        if !hidden {
            self.projection_store
                .note_public_channel_post(&channel_id, created_at.saturating_mul(1_000))
                .await?;
        }
        Ok(())
    }

    async fn blocks_either_direction(&self, left: &str, right: &str) -> Result<bool> {
        for (owner, visitor) in [(left, right), (right, left)] {
            if self
                .store
                .get_block_edge(owner, visitor)
                .await?
                .is_some_and(|edge| edge.status == BlockEdgeStatus::Active)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
