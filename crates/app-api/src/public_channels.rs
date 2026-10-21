//! 公開 channel(公開スレッド。#1840、ADR 0068)の作成と、ID からの取得。
//!
//! metadata は作成時刻の公開 topic の bucket の `channels/list/<作成時刻の秒 20 桁>-<channel ID>/<channel ID>` に置く
//! (タイムラインの索引と同じ形なので、作成の新しい順に有界に読める)。channel の投稿は既存の投稿の作成
//! (`ChannelRef::PublicChannel`)で同じ topic の bucket に置く。検証した metadata は、一覧の行(`PublicChannelStore`)
//! にも置く。タイトルの変更と削除(#1888)は、作成者が版を進めた metadata で同じ key を上書きし、読む側は行より
//! 新しい版だけを反映する。
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
    /// 削除したスレッドでは空。
    pub title: String,
    pub creator_pubkey: String,
    /// 作成時刻(ミリ秒。channel の ID の時刻)。
    pub created_at: i64,
    /// 作成と同時に書いた最初の投稿。本文を空にして作った channel には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub first_post_id: Option<String>,
    /// 手元にある最初の投稿(取り下げたものは無い。一覧の行のサムネイル。#1900)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub first_post: Option<PostView>,
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
    /// 作成者が削除した(#1888)。一覧には出さず、ID からの取得だけが返す。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub deleted: bool,
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
            first_post: None,
            last_post_at: row.last_post_at,
            creator_name: None,
            creator_display_name: None,
            live_now: false,
            deleted: row.deleted,
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
        revision: content.revision,
        deleted: content.deleted,
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
        content_labels: Vec<String>,
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
            revision: 0,
            deleted: false,
        };
        // 最初の投稿を書く前に、metadata の上限を確かめる(投稿だけが残らないように)。
        build_public_channel_envelope(self.keys(), &content)?;
        // 本文が空でも添付があれば最初の投稿を作る(タイムラインの投稿と同じ。ラベルだけでは作らない。#1889)。
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
                    content_labels,
                    None,
                )
                .await?,
            ));
        }
        self.publish_public_channel(content).await
    }

    /// 作成者がタイトルを変える(#1888)。削除したスレッドは変えられない。
    pub async fn update_public_channel_title(
        &self,
        topic_id: &str,
        channel_id: &str,
        title: &str,
    ) -> Result<PublicChannelView> {
        self.revise_public_channel(topic_id, channel_id, Some(title))
            .await
    }

    /// 作成者がスレッドを削除する(#1888)。取り消せない。投稿は各投稿者のものなので消さない。
    pub async fn delete_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<PublicChannelView> {
        self.revise_public_channel(topic_id, channel_id, None).await
    }

    /// 手元の行(無ければ ID から読んだ行)の版を 1 つ進めた metadata を書く。`title` が無ければ削除する(タイトルを
    /// 空にする)。
    async fn revise_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
        title: Option<&str>,
    ) -> Result<PublicChannelView> {
        let store = &self.services.projection_store;
        if store.get_public_channel_row(channel_id).await?.is_none() {
            self.read_public_channel(topic_id, channel_id).await?;
        }
        let row = store
            .get_public_channel_row(channel_id)
            .await?
            .filter(|row| row.topic_id == topic_id)
            .ok_or_else(|| anyhow::anyhow!("public channel was not found"))?;
        anyhow::ensure!(
            row.creator_pubkey == self.current_author_pubkey().as_str(),
            "only the creator can change a public channel"
        );
        anyhow::ensure!(!row.deleted, "public channel was deleted");
        self.publish_public_channel(KukuriPublicChannelContentV1 {
            channel_id: ChannelId::new(channel_id),
            topic_id: TopicId::new(topic_id),
            title: title.map(str::trim).unwrap_or_default().to_string(),
            first_post_id: row.first_post_id.map(EnvelopeId::from),
            revision: row.revision.saturating_add(1),
            deleted: title.is_none(),
        })
        .await
    }

    /// 自分の metadata を署名して作成の bucket の key に書き(版を進めた記録は同じ key を上書きする)、行に置いて、topic の
    /// lease を持つ端末に知らせる(受け取った端末は metadata を読んで一覧の行に置く。旧版は未知の種類として読まない)。
    async fn publish_public_channel(
        &self,
        content: KukuriPublicChannelContentV1,
    ) -> Result<PublicChannelView> {
        let envelope = build_public_channel_envelope(self.keys(), &content)?;
        let topic_id = content.topic_id.as_str().to_string();
        let (replica, key) = public_channel_location(&topic_id, &content.channel_id)?;
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
        let row = public_channel_row(self.current_author_pubkey().as_str(), content);
        self.services
            .projection_store
            .put_public_channel(row.clone(), false)
            .await?;
        if let Err(error) = self
            .services
            .hint_transport
            .publish_hint(
                &channel_hint_topic_for(&topic_id, None),
                GossipHint::SessionChanged {
                    topic_id: TopicId::new(topic_id.as_str()),
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
            .with_row_details(&topic_id, vec![row.into()], &BTreeSet::new())
            .await?
            .remove(0))
    }

    /// 行に作成者の名前と配信中の印と最初の投稿を付ける。名前は手元の profile から付け、手元に無い profile は、タイムライン
    /// の投稿と同じく背景で読む(読めたら次の取得で付く)。配信中の印は、行ごとに手元の配信中の Live の行を上限つきで読む
    /// (Live の変化は topic の lease で受け取る。#1840 AC-4)。最初の投稿は手元の行だけを付け、無ければ読まない(画面が ID
    /// から読み直す。#1900)。読むのは渡した行の分だけ。
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
        let mut first_posts = Vec::new();
        for view in views.iter().filter(|view| !view.deleted) {
            if let Some(first_post_id) = &view.first_post_id
                && let Some(row) = self
                    .services
                    .projection_store
                    .get_object_projection(&EnvelopeId::from(first_post_id.as_str()))
                    .await?
                && row.channel_id == view.channel_id
            {
                first_posts.push(row);
            }
        }
        let first_posts = self
            .page_to_view_with_policy(
                Page {
                    items: first_posts,
                    next_cursor: None,
                },
                DocFetchPolicy::LocalOnly,
            )
            .await?
            .items;
        for view in &mut views {
            view.first_post = first_posts
                .iter()
                .find(|post| {
                    view.first_post_id.as_ref() == Some(&post.object_id)
                        && post.withdrawal.is_none()
                })
                .cloned();
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

    /// 公開 channel を ID から 1 件読む。手元の行があればそれを返し、背景で変更と削除を読み直す(channel ごとに間隔を
    /// 空ける。#1888)。行が無ければ手元(保持した他人の record を含む)を先に読み、無ければ channel と topic の
    /// 読み先から読む。検証に通った record は保持し、他の参加者へ提供する(#1395)。見つからなければ `None`。
    /// 作成時の最初の投稿も、作成日(と翌日)の bucket から 1 件読んで反映する(ID から返信ツリーを開けるように)。
    /// 非表示の作者(mute と、どちらの向きの block)の channel は、一覧と同じく返さない(#1840 AC-3)。削除した
    /// channel は削除済みとして返す。
    pub async fn get_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<PublicChannelView>> {
        let known = self
            .services
            .projection_store
            .get_public_channel_row(channel_id)
            .await?;
        let channel = match known {
            Some(row) if row.topic_id == topic_id => {
                self.schedule_public_channel_check(topic_id, channel_id);
                row.into()
            }
            _ => match self.read_public_channel(topic_id, channel_id).await? {
                Some(channel) => channel,
                None => return Ok(None),
            },
        };
        let hidden = self.current_hidden_author_pubkeys().await?;
        if hidden.contains(&channel.creator_pubkey) {
            return Ok(None);
        }
        if !channel.deleted
            && let Some(first_post_id) = &channel.first_post_id
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

    /// metadata を ID から読み、一覧の行に置く(手元を先に、無ければ channel と topic の読み先から)。行が既にあれば、
    /// 行より新しい版(変更と削除。#1888)を同じ順に探して反映し、無ければ手元の行を返す。
    pub(crate) async fn read_public_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<Option<PublicChannelView>> {
        let channel_id = ChannelId::new(channel_id);
        let (replica, _) = public_channel_location(topic_id, &channel_id)?;
        let known = self
            .services
            .projection_store
            .get_public_channel_row(channel_id.as_str())
            .await?
            .filter(|row| row.topic_id == topic_id);
        let local: Arc<dyn DocsSync> = Arc::new(LocalSourceReader {
            docs: self.services.docs_sync.clone(),
            replica: replica.clone(),
        });
        let newer = read_local_then_remote(
            local,
            async {
                self.remote_post_readers(topic_id, Some(channel_id.as_str()), &replica, None)
                    .await
                    .inspect_err(|error| warn!(%error, "public channel reader selection failed"))
                    .unwrap_or_default()
            },
            |source: Arc<dyn DocsSync>, policy| {
                self.read_public_channel_from(
                    source,
                    topic_id,
                    &channel_id,
                    policy,
                    known.as_ref().map(|row| row.revision),
                )
            },
        )
        .await?;
        Ok(newer.or_else(|| known.map(Into::into)))
    }

    /// ID から取得した channel の変更と削除を、背景で読み直す(#1888)。channel ごとに間隔を空け、同時に走る数を
    /// 抑える(取り下げの確認と同じ台帳)。反映したら同期の時刻を進め、画面に読み直させる。
    fn schedule_public_channel_check(&self, topic_id: &str, channel_id: &str) {
        let checks = &self.services.withdrawal_checks;
        let Ok(permit) = checks.permits().try_acquire_owned() else {
            return;
        };
        if !checks.try_begin(
            &format!("public-channel\n{channel_id}"),
            Utc::now().timestamp_millis(),
        ) {
            return;
        }
        let app = self.account_handle();
        let (topic_id, channel_id) = (topic_id.to_owned(), channel_id.to_owned());
        n0_future::task::spawn(async move {
            let _permit = permit;
            let store = app.services.projection_store.clone();
            let revision = async || {
                let row = store.get_public_channel_row(&channel_id).await?;
                anyhow::Ok(row.map(|row| row.revision))
            };
            let checked = async {
                let before = revision().await?;
                app.read_public_channel(&topic_id, &channel_id).await?;
                anyhow::Ok(revision().await? != before)
            };
            match checked.await {
                Ok(true) => app.last_sync_ts.set(Utc::now().timestamp_millis()).await,
                Ok(false) => {}
                Err(error) => warn!(%error, "public channel check failed"),
            }
        });
    }

    /// 1 つの読み先から metadata の record を読み、検証に通った最も新しい版を保持して一覧の行に重ねる。`known` (手元の
    /// 行の版)があれば、それより新しい版だけを反映し、無ければ `None` を返す。
    pub(crate) async fn read_public_channel_from(
        &self,
        source: Arc<dyn DocsSync>,
        topic_id: &str,
        channel_id: &ChannelId,
        policy: DocFetchPolicy,
        known: Option<u32>,
    ) -> Result<Option<PublicChannelView>> {
        let (replica, key) = public_channel_location(topic_id, channel_id)?;
        let records = source
            .query_replica_exact_bounded(&replica, &key, MAX_ENVELOPE_RECORDS_PER_OBJECT, policy)
            .await?;
        let mut newest: Option<(KukuriPublicChannelContentV1, String, Option<String>)> = None;
        for record in records {
            let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
                continue;
            };
            let Ok(content) = verify_public_channel(&envelope) else {
                continue;
            };
            if content.channel_id != *channel_id
                || content.topic_id.as_str() != topic_id
                || known.is_some_and(|known| content.revision <= known)
                || newest
                    .as_ref()
                    .is_some_and(|(newest, ..)| content.revision <= newest.revision)
            {
                continue;
            }
            newest = Some((content, envelope.pubkey.0, record.docs_author));
        }
        let Some((content, creator, docs_author)) = newest else {
            return Ok(None);
        };
        if let Err(error) = source
            .persist_verified_record(&replica, &key, docs_author.as_deref(), &[])
            .await
        {
            warn!(%error, "failed to keep a public channel record");
        }
        let store = &self.services.projection_store;
        store
            .put_public_channel(
                public_channel_row(&creator, content),
                creator != self.current_author_pubkey().as_str(),
            )
            .await?;
        Ok(store
            .get_public_channel_row(channel_id.as_str())
            .await?
            .map(Into::into))
    }
}
