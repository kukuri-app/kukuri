//! 本人の書込みの record を保護所有先へも置く(#1221 R5-I)。

use super::*;

impl IrohDocsSync {
    /// 本人の書込みの record を、保護参照を付けて保護所有先へも置く(#1221 R5-I)。backup・restore と相手への
    /// 再提供(`DocReadProtocol` の exact)に使う。他人の author の領域(`author::<他人>` とその author bucket)へ
    /// 置く行は、読み直し・hydration で手元へ置いた他人の内容なので置かない。
    pub(super) async fn protect_own_record(
        &self,
        replica_id: &ReplicaId,
        key: &str,
        author: AuthorId,
        value: Vec<u8>,
        content_hash: iroh_blobs::Hash,
    ) -> Result<()> {
        let Some(cache) = self.remote_cache.as_ref() else {
            return Ok(());
        };
        let Some(owner) = self
            .account_docs_author
            .lock()
            .await
            .as_ref()
            .map(|account| account.owner.clone())
        else {
            return Ok(());
        };
        let author_scope = match replica_id.as_str().strip_prefix("author::") {
            Some(pubkey) => Some(pubkey.to_string()),
            None if replica_id.as_str().starts_with("bucket::") => {
                match crate::BucketReplica::parse(replica_id)?.scope() {
                    crate::BucketScope::Author { author_pubkey } => Some(author_pubkey.clone()),
                    _ => None,
                }
            }
            None => None,
        };
        if author_scope.is_some_and(|pubkey| pubkey != owner) {
            return Ok(());
        }
        let author = author.to_string();
        let payload = serde_json::to_vec(&kukuri_iroh_node::DocReadRecord {
            key: key.to_string(),
            content_len: value.len() as u64,
            value,
            content_hash: content_hash.to_string(),
            docs_author: author.clone(),
        })?;
        cache
            .put_owned_record(replica_id.as_str(), key, &author, &payload)
            .await
    }
}
