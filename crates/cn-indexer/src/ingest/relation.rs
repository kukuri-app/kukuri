//! 2 者間のアクションの観測（#1221 R5-E、2026-09-26 ユーザー決定）。
//!
//! public topic で索引に入れた投稿の返信・repost・引用と、索引済みの投稿へのリアクションを
//! `cn_core::relation_actions` へ保存する。新しいアクション（actor → target）を保存したときだけ、target の
//! author replica の `graph/follows/<actor>` を 1 key 読み、target → actor のフォローを保存・削除する。
//! 読み取りは有界（reaction 一覧は 1 投稿あたり最大 [`REACTIONS_PER_OBJECT`] 件、envelope は key ごとに
//! [`RECORDS_PER_EXACT_KEY`] 件）。author replica は remote の reader からだけ読み、namespace を import しない。
//! 観測の失敗は索引を止めない（warn して次の機会に委ねる）。

use kukuri_cn_core::{
    RelationAction, RelationActionKind, indexed_public_author, record_relation_action,
    relation_action_exists, remove_relation_action,
};
use kukuri_core::{
    FollowEdgeDocV1, FollowEdgeStatus, ReactionDocV1, deterministic_reaction_id, parse_follow_edge,
    parse_reaction,
};
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery, author_replica_id, stable_key};

use super::*;

/// 1 投稿あたりに読む reaction の key の上限（state と envelope で 2 key ずつ）。
pub(super) const REACTIONS_PER_OBJECT: usize = 16;

/// 変更通知の reaction key（`reactions/<投稿>/<reaction id>/{state,envelope}`）が指す投稿と reaction id。
/// custom reaction の asset（`reactions/assets/...`）は投稿を指さないため含めない。
pub fn changed_reactions<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<(String, String)> {
    let mut found = keys
        .into_iter()
        .filter_map(|key| {
            let rest = key.strip_prefix(SharedReplicaKeyFamily::Reaction.prefix())?;
            let rest = rest
                .strip_suffix("/state")
                .or_else(|| rest.strip_suffix("/envelope"))?;
            let (target, reaction) = rest.split_once('/')?;
            (is_object_id(target) && !reaction.is_empty() && !reaction.contains('/'))
                .then(|| (target.to_string(), reaction.to_string()))
        })
        .collect::<Vec<_>>();
    found.sort();
    found.dedup();
    found
}

fn is_object_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl IngestPipeline {
    /// 索引に入れた public 投稿の返信・repost・引用を保存する。
    pub(super) async fn observe_post_actions(&self, scope_id: &str, object: &PostObjectView) {
        let Some(pool) = &self.relation_pool else {
            return;
        };
        let reply_target = match &object.reply_to {
            Some(parent) => indexed_public_author(pool, scope_id, parent.as_str())
                .await
                .unwrap_or_else(|error| {
                    warn!(%error, "failed to resolve the reply target author");
                    None
                }),
            None => None,
        };
        let repost_target = object
            .repost_of
            .as_ref()
            .map(|source| source.source_author_pubkey.as_str().to_string());
        for (kind, target) in [
            (RelationActionKind::Reply, reply_target),
            (RelationActionKind::Repost, repost_target),
        ] {
            if let Some(target) = target {
                self.observe_action(RelationAction {
                    kind,
                    source_id: object.object_id.as_str().to_string(),
                    actor_pubkey: object.author.as_str().to_string(),
                    target_pubkey: target,
                    scope_id: Some(scope_id.to_string()),
                    anchor_object_id: Some(object.object_id.as_str().to_string()),
                })
                .await;
            }
        }
    }

    /// 投稿へのリアクションを保存する。`reactions` が None なら投稿ごとに一覧の窓を読み、まだ保存していない
    /// reaction だけを読む。Some なら変更通知で届いた reaction を読み直す（取り消しも反映する）。
    pub(super) async fn observe_reactions(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        replica_id: &ReplicaId,
        targets: &[String],
        reactions: Option<&[(String, String)]>,
    ) {
        let Some(pool) = &self.relation_pool else {
            return;
        };
        if scope_kind != IndexScopeKind::PublicTopic {
            return;
        }
        let pairs = match reactions {
            Some(reactions) => reactions.to_vec(),
            None => {
                let mut pairs = Vec::new();
                for target in targets {
                    match self.unrecorded_reactions(pool, replica_id, target).await {
                        Ok(found) => pairs.extend(found),
                        Err(error) => warn!(%error, "failed to list reactions of an indexed post"),
                    }
                }
                pairs
            }
        };
        for (target, reaction_id) in pairs {
            if let Err(error) = self
                .observe_reaction(pool, scope_id, replica_id, &target, &reaction_id)
                .await
            {
                warn!(%error, "failed to observe a reaction");
            }
        }
    }

    async fn unrecorded_reactions(
        &self,
        pool: &sqlx::PgPool,
        replica_id: &ReplicaId,
        target: &str,
    ) -> Result<Vec<(String, String)>> {
        let page = self
            .docs_sync
            .query_replica_keys(
                replica_id,
                DocKeyQuery {
                    prefix: format!("{}{target}/", SharedReplicaKeyFamily::Reaction.prefix()),
                    order: DocKeyOrder::Ascending,
                    limit: REACTIONS_PER_OBJECT * 2,
                },
            )
            .await?;
        let mut found = Vec::new();
        for (target, reaction_id) in
            changed_reactions(page.entries.iter().map(|entry| entry.key.as_str()))
        {
            if !relation_action_exists(pool, RelationActionKind::Reaction, &reaction_id).await? {
                found.push((target, reaction_id));
            }
        }
        Ok(found)
    }

    async fn observe_reaction(
        &self,
        pool: &sqlx::PgPool,
        scope_id: &str,
        replica_id: &ReplicaId,
        target: &str,
        reaction_id: &str,
    ) -> Result<()> {
        let key = format!(
            "{}{target}/{reaction_id}/envelope",
            SharedReplicaKeyFamily::Reaction.prefix()
        );
        let mut newest: Option<ReactionDocV1> = None;
        for record in self
            .docs_sync
            .query_replica_exact_bounded(
                replica_id,
                &key,
                RECORDS_PER_EXACT_KEY,
                DocFetchPolicy::LocalThenRemote,
            )
            .await?
        {
            let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
                continue;
            };
            if envelope.verify().is_err() {
                continue;
            }
            let Ok(Some(doc)) = parse_reaction(&envelope) else {
                continue;
            };
            let expected = deterministic_reaction_id(
                replica_id,
                &doc.target_object_id,
                &doc.author_pubkey,
                doc.normalized_reaction_key.as_str(),
            );
            let matches = doc.target_object_id.as_str() == target
                && doc.reaction_id.as_str() == reaction_id
                && doc.reaction_id == expected
                && doc.target_topic_id.as_str() == scope_id
                && doc.channel_id.is_none();
            if matches
                && newest
                    .as_ref()
                    .is_none_or(|current| doc.updated_at > current.updated_at)
            {
                newest = Some(doc);
            }
        }
        let Some(doc) = newest else {
            return Ok(());
        };
        if doc.status != ObjectStatus::Active {
            return remove_relation_action(pool, RelationActionKind::Reaction, reaction_id).await;
        }
        let Some(target_author) = indexed_public_author(pool, scope_id, target).await? else {
            return Ok(());
        };
        self.observe_action(RelationAction {
            kind: RelationActionKind::Reaction,
            source_id: reaction_id.to_string(),
            actor_pubkey: doc.author_pubkey.as_str().to_string(),
            target_pubkey: target_author,
            scope_id: Some(scope_id.to_string()),
            anchor_object_id: Some(target.to_string()),
        })
        .await;
        Ok(())
    }

    /// アクションを保存し、新しく保存したときだけ target → actor のフォローを読む。
    async fn observe_action(&self, action: RelationAction) {
        let Some(pool) = &self.relation_pool else {
            return;
        };
        match record_relation_action(pool, &action).await {
            Ok(true) => {
                if let Err(error) = self
                    .observe_follow(pool, &action.target_pubkey, &action.actor_pubkey)
                    .await
                {
                    warn!(%error, "failed to read the reverse follow edge");
                }
            }
            Ok(false) => {}
            Err(error) => warn!(%error, "failed to record a relation action"),
        }
    }

    /// `subject` の author replica の `graph/follows/<target>` を読み、署名つき envelope で確かめて保存・削除する。
    /// remote の reader からだけ読む（手元の同期経路では author replica を開かない）。
    async fn observe_follow(&self, pool: &sqlx::PgPool, subject: &str, target: &str) -> Result<()> {
        if self.docs_sync.remote_reader_id().is_none() {
            return Ok(());
        }
        let replica = author_replica_id(subject);
        let Some(doc) = self
            .docs_sync
            .query_replica_exact_bounded(
                &replica,
                &stable_key("graph/follows", target),
                RECORDS_PER_EXACT_KEY,
                DocFetchPolicy::LocalThenRemote,
            )
            .await?
            .iter()
            .filter_map(|record| serde_json::from_slice::<FollowEdgeDocV1>(&record.value).ok())
            .max_by_key(|doc| doc.updated_at)
        else {
            return Ok(());
        };
        let mut edge = None;
        for record in self
            .docs_sync
            .query_replica_exact_bounded(
                &replica,
                &stable_key("envelopes", doc.envelope_id.as_str()),
                RECORDS_PER_EXACT_KEY,
                DocFetchPolicy::LocalThenRemote,
            )
            .await?
        {
            let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
                continue;
            };
            if envelope.verify().is_err() || envelope.id != doc.envelope_id {
                continue;
            }
            if let Ok(Some(parsed)) = parse_follow_edge(&envelope)
                && parsed.subject_pubkey.as_str() == subject
                && parsed.target_pubkey.as_str() == target
            {
                edge = Some(parsed);
                break;
            }
        }
        let action = RelationAction::follow(subject, target);
        match edge.map(|edge| edge.status) {
            Some(FollowEdgeStatus::Active) => {
                record_relation_action(pool, &action).await?;
            }
            Some(FollowEdgeStatus::Revoked) => {
                remove_relation_action(pool, RelationActionKind::Follow, &action.source_id).await?;
            }
            None => {}
        }
        Ok(())
    }
}
