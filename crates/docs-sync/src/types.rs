use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::Stream;
use kukuri_core::ReplicaId;
use kukuri_transport::SeedPeer;
use serde::{Deserialize, Serialize};

pub type DocEventStream = Pin<Box<dyn Stream<Item = Result<DocEvent>> + Send>>;
pub type ReplicaNoticeStream = Pin<Box<dyn Stream<Item = Result<ReplicaNotice>> + Send>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocOp {
    SetJson {
        key: String,
        value: serde_json::Value,
    },
    SetBytes {
        key: String,
        value: Vec<u8>,
    },
    DeletePrefix {
        prefix: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocQuery {
    Exact(String),
    Prefix(String),
    All,
}

/// key だけを返す上限つきの読み出し(#1239)。entry の本体は読まない。
///
/// replica の総 entry 数に依存しない読み出しの基本形。全件を読む `DocQuery::Prefix` の代わりに使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocKeyQuery {
    pub prefix: String,
    pub order: DocKeyOrder,
    /// 返す entry の上限。0 なら何も返さない。
    pub limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKeyOrder {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocKeyEntry {
    pub key: String,
    pub content_hash: String,
    pub content_len: u64,
    /// その entry を書いた docs author の id(ADR 0053)。docs author を持たない実装は `None`。
    pub docs_author: Option<String>,
}

/// 上限つきの key の一覧(`DocsSync::query_replica_keys`)の結果(#1257)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocKeyPage {
    /// 返せた entry。件数は `limit` 以下。
    pub entries: Vec<DocKeyEntry>,
    /// query が `limit` 件の entry を読んで打ち切られたか。返さずに飛ばした entry(UTF-8 でない key)も数える。
    ///
    /// `false` なら、その prefix の entry は今回読んだもので尽きている。`true` なら、続きがありうる。
    /// 呼び出し側は「尽きたか」の判定に、`entries` の件数ではなくこの値を使う。飛ばした entry があると、
    /// 打ち切られた読み出しでも `entries` の件数は `limit` に満たない。
    pub reached_limit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocFetchPolicy {
    LocalOnly,
    LocalThenRemote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRecord {
    pub key: String,
    pub value: Vec<u8>,
    pub content_hash: String,
    pub content_len: u64,
    /// その entry を書いた docs author の id(ADR 0053)。docs author を持たない実装は `None`。
    #[serde(default)]
    pub docs_author: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocEvent {
    pub replica_id: ReplicaId,
    pub key: String,
    pub content_hash: String,
    pub source_peer: Option<String>,
    /// その entry を書いた docs author の id(ADR 0053)。docs author を持たない実装は `None`。
    #[serde(default)]
    pub docs_author: Option<String>,
}

/// replica の購読が受け取る通知(#1239)。
///
/// entry の event は上限つきの buffer を通るので、まとまった同期では取りこぼす。取りこぼしを黙って捨てると、
/// 購読側は「全件を読み直す」以外に追いつく手段が無くなる。取りこぼしと同期の区切りを通知として流し、
/// 購読側が上限つきの窓の追いつきを予約できるようにする。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplicaNotice {
    /// entry が 1 件、手元の replica に入った。
    Entry(DocEvent),
    /// 相手との同期が 1 回終わった(成功・失敗を問わない)。
    SyncFinished,
    /// 同期で受け取った entry の本体が、手元にそろった。
    ContentReady,
    /// 購読側が遅れて、`missed` 件の通知を受け取れなかった。
    Lagged { missed: u64 },
}

#[async_trait]
pub trait DocsSync: Send + Sync {
    /// End one remote object's read lease before advancing a bounded page.
    async fn finish_remote_object(&self) {}
    /// Commit the exact record already read by a demand lease after the caller
    /// verifies its signed content and scope under its save guard.
    async fn persist_verified_record(
        &self,
        _replica: &ReplicaId,
        _key: &str,
        _author: Option<&str>,
    ) -> Result<()> {
        Ok(())
    }
    /// 保存済みpublic replicaのexact keyだけを読む。namespaceの作成、sync、subscribeを行わない。
    /// author指定は1名の索引、結果は最大8件。未対応adapterは通常queryへfallbackしない。
    async fn query_local_source(
        &self,
        _replica: &ReplicaId,
        _key: &str,
        _author: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<DocRecord>> {
        anyhow::bail!("this DocsSync implementation does not support local source reads")
    }
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()>;
    /// 同期とevent転送を停止しhandleを解放する。永続entryとcapabilityは削除しない。
    /// 呼出元は利用中のleaseが無いことを保証する。未対応実装を成功扱いにしない。
    async fn close_replica(&self, _replica_id: &ReplicaId) -> Result<()> {
        anyhow::bail!("this DocsSync implementation does not support closing replicas")
    }
    async fn register_private_replica_secret(
        &self,
        _replica_id: &ReplicaId,
        _namespace_secret_hex: &str,
    ) -> Result<()> {
        Ok(())
    }
    async fn remove_private_replica_secret(&self, _replica_id: &ReplicaId) -> Result<()> {
        Ok(())
    }
    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()>;
    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>>;
    async fn query_replica(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
    ) -> Result<Vec<DocRecord>> {
        self.query_replica_with_policy(replica_id, query, DocFetchPolicy::LocalThenRemote)
            .await
    }
    /// key を 1 つ指定し、返す record 数に上限を置く読み出し(#1248)。
    ///
    /// 同じ key には docs author ごとの entry がありうる(key・docs author の昇順で返る)。その replica に書ける
    /// 誰もが同じ key へ entry を足せるので、先頭の 1 件だけを信用せず、上限つきで複数を調べる読み手が使う。
    /// 既定実装は読んでから切り詰める。1 つの key の entry 数が増えうる本番の実装は、query に上限を渡すこと。
    async fn query_replica_exact_bounded(
        &self,
        replica_id: &ReplicaId,
        key: &str,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        let mut records = self
            .query_replica_with_policy(replica_id, DocQuery::Exact(key.to_string()), policy)
            .await?;
        records.truncate(limit);
        Ok(records)
    }
    /// key の索引だけを使う上限つきの読み出し。
    ///
    /// 既定実装はエラーを返す。全件読みへ黙って落ちると、replica の大きさに比例する経路が戻るため、
    /// 本番の実装と、この読み出しを通る test double は必ず実装する。
    async fn query_replica_keys(
        &self,
        _replica_id: &ReplicaId,
        _query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        anyhow::bail!("this DocsSync implementation does not support bounded key queries")
    }
    /// docs author を指定した、上限つきの key の一覧(#1239、ADR 0053 §6)。`query_replica_keys` と同じ意味で、
    /// その docs author が書いた entry だけを返す。他の名義の entry は、何件あっても読まず、`limit` にも数えない。
    /// `docs_author` が docs author の id として読めない値のときは、空の結果を返す。
    /// 既定実装はエラーを返す(名義を問わない一覧へ黙って落ちると、他の名義の entry で窓が埋まる経路が戻るため)。
    async fn query_replica_keys_by_author(
        &self,
        _replica_id: &ReplicaId,
        _docs_author: &str,
        _query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        anyhow::bail!("this DocsSync implementation does not support key queries by docs author")
    }
    /// この実装が書き込みに使う、アカウントの署名鍵から導出した docs author の id(ADR 0053)。
    ///
    /// 導出した docs author を設定していない実装と、docs author を持たない実装は `None` を返す。`None` のとき、
    /// 呼び出し側は投稿の envelope の `docs_author` の tag と hint の手がかりを付けない。
    async fn local_docs_author(&self) -> Result<Option<String>> {
        Ok(None)
    }
    /// docs author と key の組を指定して 1 件読む(ADR 0053 §3)。
    ///
    /// 同じ key に他の docs author の entry が何件あっても、読む entry は最大 1 件。`docs_author` が docs author の id として
    /// 読めない値のとき(手がかりは信用しない入力)は `Ok(None)` を返す。
    /// 既定実装はエラーを返す。key だけの読み出しへ黙って落ちると、積まれた record 数に影響される経路が戻るため、
    /// 本番の実装と、この読み出しを通る test double は必ず実装する。
    async fn query_replica_by_author(
        &self,
        _replica_id: &ReplicaId,
        _docs_author: &str,
        _key: &str,
        _policy: DocFetchPolicy,
    ) -> Result<Option<DocRecord>> {
        anyhow::bail!("this DocsSync implementation does not support reads by docs author")
    }
    async fn subscribe_replica(&self, replica_id: &ReplicaId) -> Result<DocEventStream>;
    /// entry の event に加えて、取りこぼしと同期の区切りも受け取る購読(#1239)。
    ///
    /// 既定実装は `subscribe_replica` の entry だけを流す(取りこぼしと同期の区切りを知らせない実装として振る舞う)。
    async fn subscribe_replica_notices(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<ReplicaNoticeStream> {
        let events = self.subscribe_replica(replica_id).await?;
        Ok(Box::pin(futures_util::StreamExt::map(events, |event| {
            event.map(ReplicaNotice::Entry)
        })))
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()>;
    async fn learn_peer(&self, _endpoint_id: &str) -> Result<()> {
        Ok(())
    }
    async fn restart_replica_sync(&self, replica_id: &ReplicaId) -> Result<()> {
        self.open_replica(replica_id).await
    }
    async fn set_seed_peers(&self, _peers: Vec<SeedPeer>) -> Result<()> {
        Ok(())
    }
    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// A fresh bounded read lease per selected provider. Private callers pass
    /// the namespace secret and peers for this scope; no global peer fallback.
    async fn remote_readers(
        &self,
        _replica: &ReplicaId,
        _private_secret: Option<[u8; 32]>,
        _scope_peers: Vec<SeedPeer>,
    ) -> Result<Vec<Arc<dyn DocsSync>>> {
        Ok(Vec::new())
    }
}
