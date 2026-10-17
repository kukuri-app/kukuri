//! 公開 channel(公開スレッド。#1840、ADR 0068)の作成と、ID からの取得。
//!
//! metadata は作成時刻の公開 topic の bucket の `channels/list/<channel ID>` に置く。channel の投稿は既存の投稿の作成
//! (`ChannelRef::PublicChannel`)で同じ topic の bucket に置く。
use crate::service::*;
use kukuri_core::{
    KukuriPublicChannelContentV1, build_public_channel_envelope, verify_public_channel,
};
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

/// 公開 channel の表示用の値。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PublicChannelView {
    pub channel_id: String,
    pub topic_id: String,
    pub title: String,
    pub creator_pubkey: String,
    /// 作成時刻(ミリ秒。channel の ID の時刻)。
    pub created_at: i64,
    /// 作成と同時に書いた最初の投稿。本文を空にして作った channel には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub first_post_id: Option<String>,
}

fn public_channel_key(channel_id: &ChannelId) -> String {
    stable_key("channels/list", channel_id.as_str())
}

/// metadata を置く bucket(ID の作成時刻の、公開 topic の bucket)。
fn public_channel_replica(topic_id: &str, channel_id: &ChannelId) -> Result<ReplicaId> {
    let created_at_ms = channel_id
        .public_channel_created_at_ms()
        .ok_or_else(|| anyhow::anyhow!("public channel id is invalid"))?;
    Ok(BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic_id.to_owned(),
        },
        TimeBucket::from_unix_seconds(created_at_ms.div_euclid(1_000))?,
    )?
    .replica_id())
}

fn public_channel_view(
    creator_pubkey: &str,
    content: KukuriPublicChannelContentV1,
) -> PublicChannelView {
    PublicChannelView {
        created_at: content
            .channel_id
            .public_channel_created_at_ms()
            .unwrap_or_default(),
        channel_id: content.channel_id.0,
        topic_id: content.topic_id.as_str().to_string(),
        title: content.title,
        creator_pubkey: creator_pubkey.to_string(),
        first_post_id: content.first_post_id.map(|id| id.0),
    }
}

impl AppService {
    /// 公開 channel を作る。`first_post` が空でなければ、最初の投稿を channel の投稿として先に保存し、その ID を
    /// metadata に入れる(投稿は channel の ID だけを参照するので、互いの hash を待つ循環は起きない)。
    pub async fn create_public_channel(
        &self,
        topic_id: &str,
        title: &str,
        first_post: &str,
    ) -> Result<PublicChannelView> {
        anyhow::ensure!(
            self.services.writes_buckets(),
            "public channels need the time-bucketed docs writer"
        );
        let creator = self.current_author_pubkey();
        let channel_id = ChannelId::public_channel(Utc::now().timestamp_millis(), creator.as_str());
        let mut content = KukuriPublicChannelContentV1 {
            channel_id: channel_id.clone(),
            topic_id: TopicId::new(topic_id),
            title: title.trim().to_string(),
            first_post_id: None,
        };
        // 最初の投稿を書く前に、metadata の上限を確かめる(投稿だけが残らないように)。
        build_public_channel_envelope(self.keys(), &content)?;
        if !first_post.trim().is_empty() {
            content.first_post_id = Some(EnvelopeId::from(
                self.create_post_object(
                    topic_id,
                    ChannelRef::PublicChannel {
                        channel_id: channel_id.clone(),
                    },
                    first_post,
                    None,
                    Vec::new(),
                    Vec::new(),
                    None,
                )
                .await?,
            ));
        }
        let envelope = build_public_channel_envelope(self.keys(), &content)?;
        let replica = public_channel_replica(topic_id, &channel_id)?;
        let docs = self.docs_sync();
        docs.open_replica(&replica).await?;
        docs.apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: public_channel_key(&channel_id),
                value: serde_json::to_value(&envelope)?,
            },
        )
        .await?;
        Ok(public_channel_view(creator.as_str(), content))
    }

    /// 公開 channel を ID から 1 件読む。手元(保持した他人の record を含む)を先に読み、無ければ channel と topic の
    /// 読み先から読む。検証に通った record は保持し、他の参加者へ提供する(#1395)。見つからなければ `None`。
    /// 作成時の最初の投稿も、作成日(と翌日)の bucket から 1 件読んで反映する(ID から返信ツリーを開けるように)。
    pub async fn get_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<PublicChannelView>> {
        let Some(channel) = self.read_public_channel(topic_id, channel_id).await? else {
            return Ok(None);
        };
        if let Some(first_post_id) = &channel.first_post_id
            && let Err(error) = self
                .ensure_object_projection_near(
                    topic_id,
                    &ChannelRef::PublicChannel {
                        channel_id: ChannelId::new(channel_id),
                    }
                    .timeline_scope(),
                    &EnvelopeId::from(first_post_id.as_str()),
                    DocFetchPolicy::LocalThenRemote,
                    Some(channel.created_at.div_euclid(1_000)),
                )
                .await
        {
            warn!(%error, "failed to read the first post of a public channel");
        }
        Ok(Some(channel))
    }

    async fn read_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<PublicChannelView>> {
        let channel_id = ChannelId::new(channel_id);
        let replica = public_channel_replica(topic_id, &channel_id)?;
        let key = public_channel_key(&channel_id);
        let local: Arc<dyn DocsSync> = Arc::new(LocalSourceReader {
            docs: self.services.docs_sync.clone(),
            replica: replica.clone(),
        });
        read_local_then_remote(
            local,
            async {
                self.remote_post_readers(topic_id, Some(channel_id.as_str()), &replica, None)
                    .await
                    .inspect_err(|error| warn!(%error, "public channel reader selection failed"))
                    .unwrap_or_default()
            },
            |source: Arc<dyn DocsSync>, policy| {
                let (replica, key, channel_id) = (replica.clone(), key.clone(), channel_id.clone());
                let topic_id = topic_id.to_string();
                async move {
                    let records = source
                        .query_replica_exact_bounded(
                            &replica,
                            &key,
                            MAX_ENVELOPE_RECORDS_PER_OBJECT,
                            policy,
                        )
                        .await?;
                    for record in records {
                        let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value)
                        else {
                            continue;
                        };
                        let Ok(content) = verify_public_channel(&envelope) else {
                            continue;
                        };
                        if content.channel_id != channel_id || content.topic_id.as_str() != topic_id
                        {
                            continue;
                        }
                        if let Err(error) = source
                            .persist_verified_record(
                                &replica,
                                &key,
                                record.docs_author.as_deref(),
                                &[],
                            )
                            .await
                        {
                            warn!(%error, "failed to keep a public channel record");
                        }
                        return Ok(Some(public_channel_view(envelope.pubkey.as_str(), content)));
                    }
                    Ok(None)
                }
            },
        )
        .await
    }
}
