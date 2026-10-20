//! 公開 channel（公開スレッド。#1840、ADR 0068）。
//!
//! 作成者が署名した metadata の envelope と、channel の ID の形を決める。channel の投稿は既存の投稿の envelope
//! （`channel_id` に公開 channel の ID、`visibility` に Public）で表し、公開 topic の bucket に置く。
//! タイトルの変更と削除は、作成者が版を 1 つ進めた metadata で同じ key を上書きする（#1888）。

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{ChannelId, EnvelopeId, KukuriEnvelope, KukuriKeys, TopicId};

pub const PUBLIC_CHANNEL_KIND: &str = "public-channel";
pub const PUBLIC_CHANNEL_MAX_TITLE_CHARS: usize = 200;
const PUBLIC_CHANNEL_ID_PREFIX: &str = "pc-";
/// ID の末尾に置く作成者の公開鍵の先頭の桁数（Live の ID と同じ 64 bit）。
const PUBLIC_CHANNEL_OWNER_SUFFIX_LEN: usize = 16;

impl ChannelId {
    /// 作成時刻（ミリ秒）と作成者の公開鍵から、公開 channel の ID を作る。
    pub fn public_channel(created_at_ms: i64, creator_pubkey: &str) -> Self {
        let suffix = creator_pubkey
            .get(..PUBLIC_CHANNEL_OWNER_SUFFIX_LEN)
            .unwrap_or(creator_pubkey);
        Self(format!(
            "{PUBLIC_CHANNEL_ID_PREFIX}{created_at_ms}-{suffix}"
        ))
    }

    /// 公開 channel の ID の形（`pc-<作成時刻のミリ秒>-<作成者の公開鍵の先頭 16 桁>`）か。private channel の ID
    /// （`channel-…`）とは重ならない。
    pub fn is_public_channel(&self) -> bool {
        self.public_channel_created_at_ms().is_some()
    }

    /// 公開 channel の ID が示す作成時刻（ミリ秒）。公開 channel の ID でなければ `None`。
    pub fn public_channel_created_at_ms(&self) -> Option<i64> {
        let (millis, suffix) = self
            .0
            .strip_prefix(PUBLIC_CHANNEL_ID_PREFIX)?
            .split_once('-')?;
        let canonical = !millis.starts_with('0')
            && millis.bytes().all(|byte| byte.is_ascii_digit())
            && suffix.len() == PUBLIC_CHANNEL_OWNER_SUFFIX_LEN
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        canonical
            .then(|| millis.parse::<i64>().ok())
            .flatten()
            .filter(|millis| *millis > 0)
    }

    /// 公開 channel の ID が、この公開鍵の作成者のものか（ID の末尾が公開鍵の先頭と一致する）。
    pub fn is_public_channel_of(&self, creator_pubkey: &str) -> bool {
        self.is_public_channel()
            && self
                .0
                .rsplit_once('-')
                .is_some_and(|(_, suffix)| creator_pubkey.starts_with(suffix))
    }
}

/// 公開 channel の metadata（作成者が署名する）。署名の時刻は版によらず ID の作成時刻にする。ID・topic・最初の投稿は
/// 後から変えない。作成者は版を進めてタイトルを変え、削除する（#1888）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KukuriPublicChannelContentV1 {
    pub channel_id: ChannelId,
    pub topic_id: TopicId,
    /// 削除した版では空（旧版は空のタイトルの記録を読まない）。
    pub title: String,
    /// 作成と同時に書いた最初の投稿。本文を空にして作った channel には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_post_id: Option<EnvelopeId>,
    /// 版。作成は 0 で、変更と削除のたびに 1 つ進める。読む側は大きい版を残す。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub revision: u32,
    /// 削除した版。削除は取り消せない。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn validate(content: &KukuriPublicChannelContentV1) -> Result<i64> {
    let title = content.title.as_str();
    match content.deleted {
        true => ensure!(
            title.is_empty() && content.revision > 0,
            "deleted public channel keeps a title or has no revision"
        ),
        false => ensure!(
            !title.trim().is_empty()
                && title.trim() == title
                && title.chars().count() <= PUBLIC_CHANNEL_MAX_TITLE_CHARS,
            "public channel title is empty or too long"
        ),
    }
    ensure!(
        !content.topic_id.as_str().is_empty(),
        "public channel topic is empty"
    );
    content
        .channel_id
        .public_channel_created_at_ms()
        .ok_or_else(|| anyhow::anyhow!("public channel id is invalid"))
}

/// 作成者の鍵で metadata に署名する。署名の時刻は ID の作成時刻（秒）にする（読む側が bucket を ID から決めるため）。
pub fn build_public_channel_envelope(
    keys: &KukuriKeys,
    content: &KukuriPublicChannelContentV1,
) -> Result<KukuriEnvelope> {
    let created_at_ms = validate(content)?;
    ensure!(
        content
            .channel_id
            .is_public_channel_of(keys.public_key_hex().as_str()),
        "public channel id is not bound to the creator"
    );
    let tags = vec![
        vec!["topic".into(), content.topic_id.as_str().to_string()],
        vec!["object".into(), PUBLIC_CHANNEL_KIND.into()],
        vec!["channel".into(), content.channel_id.as_str().to_string()],
    ];
    crate::sign_envelope_json_at(
        keys,
        PUBLIC_CHANNEL_KIND,
        tags,
        content,
        created_at_ms.div_euclid(1_000),
    )
}

/// metadata の署名・形・上限、ID と作成者・作成時刻の結び付きを確かめる。読んだ bucket との照合は呼び出し側が行う。
pub fn verify_public_channel(envelope: &KukuriEnvelope) -> Result<KukuriPublicChannelContentV1> {
    envelope.verify()?;
    if envelope.kind != PUBLIC_CHANNEL_KIND {
        bail!("envelope is not a public channel");
    }
    let content: KukuriPublicChannelContentV1 = serde_json::from_str(&envelope.content)?;
    let created_at_ms = validate(&content)?;
    ensure!(
        content
            .channel_id
            .is_public_channel_of(envelope.pubkey.as_str())
            && envelope.created_at == created_at_ms.div_euclid(1_000),
        "public channel id is not bound to the signer and its creation time"
    );
    Ok(content)
}
