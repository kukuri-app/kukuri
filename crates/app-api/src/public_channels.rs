//! 公開 channel(公開スレッド。#1840、ADR 0068)の作成と、ID からの取得。
//!
//! metadata は作成時刻の公開 topic の bucket の `channels/list/<作成時刻の秒 20 桁>-<channel ID>/<channel ID>` に置く
//! (タイムラインの索引と同じ形なので、作成の新しい順に有界に読める)。channel の投稿は既存の投稿の作成
//! (`ChannelRef::PublicChannel`)で同じ topic の bucket に置く。検証した metadata は、一覧の行(`PublicChannelStore`)
//! にも置く。
use crate::service::*;
use kukuri_core::{
    KukuriPublicChannelContentV1, PUBLIC_CHANNEL_KIND, build_public_channel_envelope,
    verify_public_channel,
};
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};
use kukuri_store::PublicChannelRow;

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
    /// この端末が知っている最後の投稿の時刻(ミリ秒)。投稿を知らなければ作成時刻。
    pub last_post_at: i64,
    /// 作成者の profile の名前(手元に profile が無ければ無い。一覧の行の表示。#1840 AC-3)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub creator_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub creator_display_name: Option<String>,
    /// 配信中の Live がある(手元の Live の行から。非表示の主催者の Live は数えない。一覧の行の印。#1840 AC-4)。
    #[serde(default)]
    pub live_now: bool,
}

impl From<PublicChannelRow> for PublicChannelView {
    fn from(row: PublicChannelRow) -> Self {
        Self {
            channel_id: row.channel_id,
            topic_id: row.topic_id,
            title: row.title,
            creator_pubkey: row.creator_pubkey,
            created_at: row.created_at,
            first_post_id: row.first_post_id,
            last_post_at: row.last_post_at,
            creator_name: None,
            creator_display_name: None,
            live_now: false,
        }
    }
}

/// 一覧の行の配信中の印のために、行ごとに読む配信中の Live の数の上限(#1840 AC-4)。非表示の主催者の Live だけがこの数を
/// 占めると、印は出ない。
const LIVE_MARK_READ_LIMIT: usize = 8;

/// metadata を置く bucket(ID の作成時刻の、公開 topic の bucket)と key。
pub(crate) fn public_channel_location(
    topic_id: &str,
    channel_id: &ChannelId,
) -> Result<(ReplicaId, String)> {
    let seconds = channel_id
        .public_channel_created_at_ms()
        .ok_or_else(|| anyhow::anyhow!("public channel id is invalid"))?
        .div_euclid(1_000);
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic_id.to_owned(),
        },
        TimeBucket::from_unix_seconds(seconds)?,
    )?
    .replica_id();
    let id = channel_id.as_str();
    Ok((
        replica,
        format!("{PUBLIC_CHANNEL_LIST_PREFIX}{seconds:020}-{id}/{id}"),
    ))
}

fn public_channel_row(
    creator_pubkey: &str,
    content: KukuriPublicChannelContentV1,
) -> PublicChannelRow {
    let created_at = content
        .channel_id
        .public_channel_created_at_ms()
        .unwrap_or_default();
    PublicChannelRow {
        channel_id: content.channel_id.0,
        topic_id: content.topic_id.as_str().to_string(),
        title: content.title,
        creator_pubkey: creator_pubkey.to_string(),
        created_at,
        first_post_id: content.first_post_id.map(|id| id.0),
        last_post_at: created_at,
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
        attachments: Vec<PendingAttachment>,
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
        // 本文が空でも添付があれば最初の投稿を作る(タイムラインの投稿と同じ。#1889)。
        if !first_post.trim().is_empty() || !attachments.is_empty() {
            content.first_post_id = Some(EnvelopeId::from(
                self.create_post_object(
                    topic_id,
                    ChannelRef::PublicChannel {
                        channel_id: channel_id.clone(),
                    },
                    first_post,
                    None,
                    attachments,
                    Vec::new(),
                    None,
                )
                .await?,
            ));
        }
        let envelope = build_public_channel_envelope(self.keys(), &content)?;
        let (replica, key) = public_channel_location(topic_id, &channel_id)?;
        let docs = self.docs_sync();
        docs.open_replica(&replica).await?;
        docs.apply_doc_op(
            &replica,
            DocOp::SetJson {
                key,
                value: serde_json::to_value(&envelope)?,
            },
        )
        .await?;
        let row = public_channel_row(creator.as_str(), content);
        self.services
            .projection_store
            .put_public_channel(row.clone(), false)
            .await?;
        // topic の lease を持つ端末に作成を知らせる(受け取った端末は metadata を 1 件読んで一覧の行に置く)。
        // 旧版は未知の種類として読まない。
        if let Err(error) = self
            .services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(topic_id, None),
                GossipHint::SessionChanged {
                    topic_id: TopicId::new(topic_id),
                    session_id: row.channel_id.clone(),
                    object_kind: PUBLIC_CHANNEL_KIND.into(),
                    sent_at: Some(Utc::now().timestamp_millis()),
                },
            )
            .await
        {
            warn!(%error, "failed to announce a public channel");
        }
        Ok(self
            .with_row_details(topic_id, vec![row.into()], &BTreeSet::new())
            .await?
            .remove(0))
    }

    /// 行に作成者の名前と配信中の印を付ける。名前は手元の profile から付け、手元に無い profile は、タイムラインの投稿と
    /// 同じく背景で読む(読めたら次の取得で付く)。配信中の印は、行ごとに手元の配信中の Live の行を上限つきで読む(Live の
    /// 変化は topic の lease で受け取る。#1840 AC-4)。読むのは渡した行の分だけ。
    pub(crate) async fn with_row_details(
        &self,
        topic_id: &str,
        mut views: Vec<PublicChannelView>,
        hidden: &BTreeSet<String>,
    ) -> Result<Vec<PublicChannelView>> {
        let mut creators = views
            .iter()
            .map(|view| view.creator_pubkey.clone())
            .collect::<Vec<_>>();
        creators.sort();
        creators.dedup();
        let profiles = self.services.store.get_profiles(&creators).await?;
        let local = self.current_author_pubkey();
        self.request_missing_profiles(
            creators
                .into_iter()
                .filter(|creator| !profiles.contains_key(creator) && *creator != local.as_str())
                .map(|creator| (creator, Some(topic_id.to_owned()))),
        )
        .await;
        for view in &mut views {
            if let Some(profile) = profiles.get(&view.creator_pubkey) {
                view.creator_name = profile.name.clone();
                view.creator_display_name = profile.display_name.clone();
            }
            view.live_now = self
                .services
                .projection_store
                .list_channel_live_now_sessions(
                    topic_id,
                    view.channel_id.as_str(),
                    LIVE_MARK_READ_LIMIT,
                )
                .await?
                .iter()
                .any(|session| !hidden.contains(&session.host_pubkey));
        }
        Ok(views)
    }

    /// 公開 channel を ID から 1 件読む。手元(保持した他人の record を含む)を先に読み、無ければ channel と topic の
    /// 読み先から読む。検証に通った record は保持し、他の参加者へ提供する(#1395)。見つからなければ `None`。
    /// 作成時の最初の投稿も、作成日(と翌日)の bucket から 1 件読んで反映する(ID から返信ツリーを開けるように)。
    /// 非表示の作者(mute と、どちらの向きの block)の channel は、一覧と同じく返さない(#1840 AC-3)。
    pub async fn get_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<PublicChannelView>> {
        let Some(channel) = self.read_public_channel(topic_id, channel_id).await? else {
            return Ok(None);
        };
        let hidden = self.current_hidden_author_pubkeys().await?;
        if hidden.contains(&channel.creator_pubkey) {
            return Ok(None);
        }
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
        Ok(self
            .with_row_details(topic_id, vec![channel], &hidden)
            .await?
            .pop())
    }

    /// metadata を ID から 1 件読み、一覧の行に置く(手元を先に、無ければ channel と topic の読み先から)。
    pub(crate) async fn read_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<PublicChannelView>> {
        let channel_id = ChannelId::new(channel_id);
        let (replica, _) = public_channel_location(topic_id, &channel_id)?;
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
                self.read_public_channel_from(source, topic_id, &channel_id, policy)
            },
        )
        .await
    }

    /// 1 つの読み先から metadata の record を読む。検証に通れば保持し、一覧の行が無ければ置く。
    pub(crate) async fn read_public_channel_from(
        &self,
        source: Arc<dyn DocsSync>,
        topic_id: &str,
        channel_id: &ChannelId,
        policy: DocFetchPolicy,
    ) -> Result<Option<PublicChannelView>> {
        let (replica, key) = public_channel_location(topic_id, channel_id)?;
        let records = source
            .query_replica_exact_bounded(&replica, &key, MAX_ENVELOPE_RECORDS_PER_OBJECT, policy)
            .await?;
        for record in records {
            let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
                continue;
            };
            let Ok(content) = verify_public_channel(&envelope) else {
                continue;
            };
            if content.channel_id != *channel_id || content.topic_id.as_str() != topic_id {
                continue;
            }
            if let Err(error) = source
                .persist_verified_record(&replica, &key, record.docs_author.as_deref(), &[])
                .await
            {
                warn!(%error, "failed to keep a public channel record");
            }
            let store = &self.services.projection_store;
            if store
                .get_public_channel_row(channel_id.as_str())
                .await?
                .is_none()
            {
                let creator = envelope.pubkey.as_str();
                store
                    .put_public_channel(
                        public_channel_row(creator, content),
                        creator != self.current_author_pubkey().as_str(),
                    )
                    .await?;
            }
            return Ok(store
                .get_public_channel_row(channel_id.as_str())
                .await?
                .map(Into::into));
        }
        Ok(None)
    }
}
