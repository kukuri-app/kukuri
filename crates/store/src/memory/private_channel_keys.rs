use super::*;
use crate::{
    PrivateChannelEpochRange, PrivateChannelEpochRow, PrivateChannelFilter, PrivateChannelKeyStore,
    PrivateChannelRow,
};

impl MemoryStore {
    /// 読み書きした行を数えて、そのまま返す。
    pub(super) fn touched<T>(&self, rows: usize, value: T) -> T {
        self.private_channel_key_rows_touched
            .fetch_add(rows, std::sync::atomic::Ordering::SeqCst);
        value
    }
}

#[async_trait]
impl PrivateChannelKeyStore for MemoryStore {
    async fn put_private_channel(
        &self,
        row: &PrivateChannelRow,
        epochs: &[PrivateChannelEpochRow],
    ) -> Result<()> {
        let mut keys = self.private_channel_keys.write().await;
        keys.channels.insert(row.channel_key.clone(), row.clone());
        for epoch in epochs {
            let key = (epoch.channel_id.clone(), epoch.epoch_id.clone());
            if !keys.epochs.contains_key(&key) {
                keys.unwritten.insert(key.clone());
                keys.epochs.insert(key, epoch.clone());
            }
        }
        self.touched(1 + epochs.len(), ());
        Ok(())
    }

    async fn put_private_channel_epoch(&self, epoch: &PrivateChannelEpochRow) -> Result<bool> {
        let mut keys = self.private_channel_keys.write().await;
        let key = (epoch.channel_id.clone(), epoch.epoch_id.clone());
        let added = !keys.epochs.contains_key(&key);
        if added {
            keys.unwritten.insert(key.clone());
            keys.epochs.insert(key, epoch.clone());
        }
        Ok(self.touched(1, added))
    }

    async fn get_private_channel(&self, channel_key: &str) -> Result<Option<PrivateChannelRow>> {
        let row = self
            .private_channel_keys
            .read()
            .await
            .channels
            .get(channel_key)
            .cloned();
        Ok(self.touched(usize::from(row.is_some()), row))
    }

    async fn get_private_channel_by_id(
        &self,
        channel_id: &str,
    ) -> Result<Option<PrivateChannelRow>> {
        let row = self
            .private_channel_keys
            .read()
            .await
            .channels
            .values()
            .find(|row| row.channel_id == channel_id)
            .cloned();
        Ok(self.touched(usize::from(row.is_some()), row))
    }

    async fn list_joined_private_channels(
        &self,
        filter: PrivateChannelFilter<'_>,
        after: &str,
        limit: usize,
    ) -> Result<Vec<PrivateChannelRow>> {
        let rows = self
            .private_channel_keys
            .read()
            .await
            .channels
            .range::<str, _>((std::ops::Bound::Excluded(after), std::ops::Bound::Unbounded))
            .map(|(_, row)| row)
            .filter(|row| {
                row.joined
                    && match filter {
                        PrivateChannelFilter::All => true,
                        PrivateChannelFilter::Topic(topic) => row.topic_id == topic,
                        PrivateChannelFilter::Owner(owner) => row.owner_pubkey == owner,
                    }
            })
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(self.touched(rows.len(), rows))
    }

    async fn get_private_channel_epoch(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>> {
        let row = self
            .private_channel_keys
            .read()
            .await
            .epochs
            .get(&(channel_id.to_string(), epoch_id.to_string()))
            .cloned();
        Ok(self.touched(usize::from(row.is_some()), row))
    }

    async fn find_private_channel_epoch(
        &self,
        receive_key_id: &str,
    ) -> Result<Option<PrivateChannelEpochRow>> {
        let row = self
            .private_channel_keys
            .read()
            .await
            .epochs
            .values()
            .find(|epoch| epoch.receive_key_id == receive_key_id)
            .cloned();
        Ok(self.touched(usize::from(row.is_some()), row))
    }

    async fn list_private_channel_epochs(
        &self,
        channel_id: &str,
        range: PrivateChannelEpochRange,
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>> {
        let keys = self.private_channel_keys.read().await;
        let mut epochs = keys
            .epochs
            .values()
            .filter(|epoch| {
                epoch.channel_id == channel_id
                    && match range {
                        PrivateChannelEpochRange::AtOrBefore(at) => epoch.started_at <= at,
                        PrivateChannelEpochRange::After(at) => epoch.started_at > at,
                    }
            })
            .cloned()
            .collect::<Vec<_>>();
        epochs.sort_by(|left, right| {
            (left.started_at, &left.epoch_id).cmp(&(right.started_at, &right.epoch_id))
        });
        if matches!(range, PrivateChannelEpochRange::AtOrBefore(_)) {
            epochs.reverse();
        }
        epochs.truncate(limit);
        Ok(self.touched(epochs.len(), epochs))
    }

    async fn mark_private_channel_epoch_written(
        &self,
        channel_id: &str,
        epoch_id: &str,
    ) -> Result<()> {
        self.private_channel_keys
            .write()
            .await
            .unwritten
            .remove(&(channel_id.to_string(), epoch_id.to_string()));
        Ok(())
    }

    async fn list_unwritten_private_channel_epochs(
        &self,
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>> {
        let keys = self.private_channel_keys.read().await;
        let epochs = keys
            .unwritten
            .iter()
            .filter_map(|key| keys.epochs.get(key))
            .filter(|epoch| epoch.rotation_from.is_none() || epoch.rotation_after.is_some())
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(self.touched(epochs.len(), epochs))
    }

    async fn set_private_channel_rotation(
        &self,
        channel_id: &str,
        epoch_id: &str,
        after: Option<&str>,
    ) -> Result<()> {
        let mut keys = self.private_channel_keys.write().await;
        if let Some(epoch) = keys
            .epochs
            .get_mut(&(channel_id.to_string(), epoch_id.to_string()))
        {
            epoch.rotation_after = after.map(str::to_string);
            if after.is_none() {
                epoch.rotation_from = None;
            }
        }
        self.touched(1, ());
        Ok(())
    }

    async fn list_private_channel_rotations(
        &self,
        after: (&str, &str),
        limit: usize,
    ) -> Result<Vec<PrivateChannelEpochRow>> {
        let after = (after.0.to_string(), after.1.to_string());
        let rows = self
            .private_channel_keys
            .read()
            .await
            .epochs
            .range((std::ops::Bound::Excluded(after), std::ops::Bound::Unbounded))
            .map(|(_, epoch)| epoch)
            .filter(|epoch| epoch.rotation_from.is_some())
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(self.touched(rows.len(), rows))
    }

    async fn delete_private_channel_epochs(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut keys = self.private_channel_keys.write().await;
        let doomed = keys
            .epochs
            .keys()
            .filter(|(channel, _)| channel == channel_id)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        for key in &doomed {
            keys.epochs.remove(key);
            keys.unwritten.remove(key);
        }
        let epochs = doomed
            .into_iter()
            .map(|(_, epoch)| epoch)
            .collect::<Vec<_>>();
        Ok(self.touched(epochs.len(), epochs))
    }
}
