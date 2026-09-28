//! #1407: 読取りが作っていた空の namespace の回収。
//!
//! 更新前の版は、表示した他人の author などを読むだけで、中身の無い namespace を作っていた。空の namespace を持つ端末は、
//! 他の端末の読取りに「持っていない」ではなく「0 件」と答える。namespace の id はハッシュで、どの replica のものかを
//! 逆算できないので、中身が 0 件で開いていない namespace を消す(失うデータは無く、書込みが要れば作り直される)。
//! 一度きりの回収で、呼出元が位置と終端を保存する。

use super::*;
use iroh_docs::NamespaceId;

impl IrohDocsSync {
    /// namespace を id の順に、`cursor`(前回の最後に調べた id)より後ろから最大 `limit` 件調べ、中身が 0 件で開いて
    /// いないものを消す。戻り値は次の位置と、最後まで調べたか。
    ///
    /// iroh-docs の列挙には開始位置の指定が無いので、`cursor` までは読み飛ばす(再開のときだけ)。
    pub async fn drop_empty_namespaces_step(
        &self,
        cursor: &str,
        limit: usize,
    ) -> Result<(String, bool)> {
        let mut namespaces = self.node.docs().list().await?;
        let mut last = cursor.to_owned();
        let mut checked = 0;
        while let Some(item) = namespaces.next().await {
            let (namespace, _) = item?;
            let id = namespace.to_string();
            if id.as_str() <= cursor {
                continue;
            }
            if checked == limit {
                return Ok((last, false));
            }
            checked += 1;
            self.drop_if_empty(namespace).await?;
            last = id;
        }
        Ok((last, true))
    }

    async fn drop_if_empty(&self, namespace: NamespaceId) -> Result<()> {
        // handle の登録(読取り・書込みの open)と直列にし、確かめてから消すまでの間に開かれないようにする。
        let replicas = self.replicas.lock().await;
        if replicas.values().any(|handle| handle.doc.id() == namespace) {
            return Ok(());
        }
        let Some(doc) = self.open_existing(namespace).await? else {
            return Ok(());
        };
        let entries = doc.get_many(Query::all().limit(1).build()).await;
        let empty = match entries {
            Ok(stream) => {
                tokio::pin!(stream);
                stream.next().await.is_none()
            }
            Err(error) => {
                warn!(%namespace, %error, "failed to read a namespace before reclaiming it");
                false
            }
        };
        doc.close().await?;
        if empty && let Err(error) = self.node.docs().drop_doc(namespace).await {
            // 読取りの提供(`DocReadProtocol`)が開いている間は消せない。次の回収の対象にはしない(一度きり)。
            warn!(%namespace, %error, "an empty namespace was not reclaimed");
        }
        drop(replicas);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn empty_namespace(node: &IrohDocsNode, seed: u8) -> NamespaceId {
        let doc = node
            .docs()
            .import_namespace(Capability::Write(NamespaceSecret::from_bytes(&[seed; 32])))
            .await
            .expect("empty namespace");
        let id = doc.id();
        doc.close().await.expect("close");
        id
    }

    async fn namespaces(node: &IrohDocsNode) -> Vec<NamespaceId> {
        let mut ids = Vec::new();
        let mut list = node.docs().list().await.expect("list");
        while let Some(item) = list.next().await {
            ids.push(item.expect("namespace").0);
        }
        ids
    }

    #[tokio::test]
    async fn only_empty_and_closed_namespaces_are_reclaimed() {
        let node = IrohDocsNode::memory().await.expect("docs node");
        let docs = IrohDocsSync::new(node.clone());
        let empty = empty_namespace(&node, 1).await;
        let written = ReplicaId::new("author::written");
        docs.apply_doc_op(
            &written,
            DocOp::SetBytes {
                key: "profile/latest".into(),
                value: b"kept".to_vec(),
            },
        )
        .await
        .expect("write");
        docs.close_replica(&written).await.expect("close");
        let open = ReplicaId::new("author::open");
        let open_id = docs.ensure_replica(&open).await.expect("open").id();

        let (_, done) = docs
            .drop_empty_namespaces_step("", 128)
            .await
            .expect("reclaim");

        assert!(done);
        let left = namespaces(&node).await;
        assert!(
            !left.contains(&empty),
            "the empty closed namespace is reclaimed"
        );
        assert!(left.contains(&open_id), "an open namespace is kept");
        assert_eq!(left.len(), 2, "the written namespace is kept");
        let records = docs
            .query_replica_with_policy(
                &written,
                DocQuery::Exact("profile/latest".into()),
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("read");
        assert_eq!(records.len(), 1);
        docs.shutdown().await;
        node.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn one_step_checks_at_most_the_limit_and_resumes_from_the_cursor() {
        let node = IrohDocsNode::memory().await.expect("docs node");
        let docs = IrohDocsSync::new(node.clone());
        for seed in 0..5 {
            empty_namespace(&node, seed).await;
        }

        let (cursor, done) = docs
            .drop_empty_namespaces_step("", 3)
            .await
            .expect("first step");
        assert!(!done);
        assert_eq!(namespaces(&node).await.len(), 2);
        let (_, done) = docs
            .drop_empty_namespaces_step(&cursor, 3)
            .await
            .expect("second step");
        assert!(done);
        assert!(namespaces(&node).await.is_empty());
        docs.shutdown().await;
        node.shutdown().await.expect("shutdown");
    }
}
