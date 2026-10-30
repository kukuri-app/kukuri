//! ゲームの公式アセットの発行・一覧・取得と自動の更新(ADR 0070 §5.3、#1980 AC-1)。
//!
//! release は発行者のアカウント鍵で署名した envelope で、発行者の制御領域の application ID と asset ID から決まる key に
//! 置く。読み手は手元 → 発行者本人 → topic の参加者の順に 1 件を読み、検証できたもののうち revision が最大のものを採る。
//! 適用は revision が大きいときだけで、古い release では戻さない。適用中の版の blob と記録は保護して、ほかの端末へ提供する。
//! 更新の知らせ(hint)を受けたら、この端末で適用中のアセットは背景で新しい版を取得して適用する。

use std::path::PathBuf;

use kukuri_core::GameAssetFormat;
use serde::{Deserialize, Serialize};

use crate::AppService;

/// 更新の知らせの hint の object_kind(`SessionChanged`。session_id は `<発行者>/<asset ID>/<application ID>`)。
pub const GAME_ASSET_HINT_KIND: &str = "game-asset";
/// `list_game_assets` が 1 回で読む asset ID の上限(呼出元が分けて読む)。
pub const GAME_ASSET_LIST_LIMIT: usize = 32;
/// 取得できなかった理由の code。
pub const GAME_ASSET_UNAVAILABLE: &str = "unavailable";
pub const GAME_ASSET_NOT_FOUND: &str = "not_found";

/// アセットを取得できなかった(理由の code)。
#[derive(Debug)]
pub struct GameAssetUnavailable(pub &'static str);

impl std::fmt::Display for GameAssetUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the game asset could not be obtained: {}",
            self.0
        )
    }
}

impl std::error::Error for GameAssetUnavailable {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameAssetReleaseView {
    pub publisher_pubkey: String,
    pub application_id: String,
    pub asset_id: String,
    pub revision: u64,
    pub bytes: u64,
    pub format: GameAssetFormat,
    pub published_at: i64,
    /// この端末で適用中の版(無ければ None)。
    pub applied_revision: Option<u64>,
}

/// 検証した本体の file。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameAssetFileView {
    #[serde(flatten)]
    pub release: GameAssetReleaseView,
    pub path: String,
}

/// 検証できる新しい release が届いた。この端末で適用中のアセットは取得・適用した後に `path` 付きで、まだ適用していない
/// アセットは `path` 無しで知らせる。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameAssetUpdate {
    pub publisher_pubkey: String,
    pub application_id: String,
    pub asset_id: String,
    pub revision: u64,
    pub path: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PublishGameAssetInput {
    pub application_id: String,
    pub asset_id: String,
    pub format: GameAssetFormat,
    pub path: PathBuf,
}

/// アセットごとの取得の排他(前面の取得と、知らせを受けた背景の取得を 1 つずつ走らせる)。
#[derive(Default)]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) struct GameAssetUpdates {
    fetches: crate::service::GameRoomProjectionLocks,
    /// 取得を待っている背景の task(アセットごとに 1 つ。続けて届いた知らせは、待っている task が待った後に読み直して取る)。
    waiting: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl AppService {
    pub fn subscribe_game_asset_updates(
        &self,
    ) -> tokio::sync::broadcast::Receiver<GameAssetUpdate> {
        self.services.game_asset_updates.subscribe()
    }
}

impl crate::ServiceHandles {
    /// 検証したアセットの file を置く dir。native の runtime が入れる(cache と同じ volume に置く)。
    pub fn with_game_assets(mut self, dir: PathBuf) -> Self {
        self.game_assets_dir = Some(dir);
        self
    }
}

/// 発行・一覧・取得は file と blob の file の取り込みを使うので native だけ(Web は SDK を持たない)。
#[cfg(not(target_family = "wasm"))]
mod native {
    use std::path::Path;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use chrono::Utc;
    use kukuri_core::{
        GameAssetReleaseV1, GossipHint, KukuriEnvelope, Pubkey, TopicId,
        build_game_asset_release_envelope, validate_application_id, validate_asset_id,
        verify_game_asset_release,
    };
    use kukuri_docs_sync::{DocFetchPolicy, DocOp, DocsSync, author_replica_id, stable_key};
    use kukuri_store::AppliedGameAssetRow;

    use super::*;
    use crate::service::MAX_ENVELOPE_RECORDS_PER_OBJECT;

    /// 取得の期限: 1 端末の転送を、大きさを 1 MiB/s で送る時間(15 秒より短くしない)にする(2026-10-30 のユーザー判断)。
    const TRANSFER_BYTES_PER_SEC: u64 = 1 << 20;
    const MIN_TRANSFER: Duration = Duration::from_secs(15);

    fn record_key(application_id: &str, asset_id: &str) -> String {
        stable_key("game-assets", &format!("{application_id}/{asset_id}"))
    }

    fn transfer_deadline(bytes: u64) -> Duration {
        MIN_TRANSFER.max(Duration::from_secs(bytes / TRANSFER_BYTES_PER_SEC))
    }

    fn view(
        release: &GameAssetReleaseV1,
        publisher: &str,
        applied: Option<u64>,
    ) -> GameAssetReleaseView {
        GameAssetReleaseView {
            publisher_pubkey: publisher.into(),
            application_id: release.application_id.clone(),
            asset_id: release.asset_id.clone(),
            revision: release.revision,
            bytes: release.bytes,
            format: release.format.clone(),
            published_at: release.published_at,
            applied_revision: applied,
        }
    }

    /// 読み先が返した record のうち、検証できて revision が最大の release と、その record を書いた docs author。
    async fn newest_release(
        docs: &dyn DocsSync,
        policy: DocFetchPolicy,
        publisher: &str,
        application_id: &str,
        asset_id: &str,
    ) -> Result<Option<(GameAssetReleaseV1, Option<String>)>> {
        let records = docs
            .query_replica_exact_bounded(
                &author_replica_id(publisher),
                &record_key(application_id, asset_id),
                MAX_ENVELOPE_RECORDS_PER_OBJECT,
                policy,
            )
            .await?;
        let trusted = Pubkey::from(publisher);
        let mut newest: Option<(GameAssetReleaseV1, Option<String>)> = None;
        for record in records {
            let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
                continue;
            };
            let Ok(release) =
                verify_game_asset_release(&envelope, &trusted, application_id, asset_id)
            else {
                continue;
            };
            if newest
                .as_ref()
                .is_none_or(|(current, _)| release.revision > current.revision)
            {
                newest = Some((release, record.docs_author));
            }
        }
        Ok(newest)
    }

    impl AppService {
        fn game_assets_dir(&self) -> Result<&Path> {
            self.services
                .game_assets_dir
                .as_deref()
                .context("game assets are not available on this client")
        }

        /// 発行者の profile で release に署名して公開する(CLI と開発用の Sidecar だけ)。revision は前の版の次。
        pub async fn publish_game_asset(
            &self,
            topic_id: &str,
            input: PublishGameAssetInput,
        ) -> Result<GameAssetReleaseView> {
            validate_application_id(&input.application_id)?;
            validate_asset_id(&input.asset_id)?;
            let mut head = [0; 16];
            let read = {
                use std::io::Read;
                std::fs::File::open(&input.path)
                    .with_context(|| format!("cannot open {}", input.path.display()))?
                    .read(&mut head)?
            };
            anyhow::ensure!(
                input.format.matches_header(&head[..read]),
                "the file is not a {} asset",
                input.format.extension()
            );
            let publisher = self.current_author_pubkey();
            let previous = newest_release(
                self.services.docs_sync.as_ref(),
                DocFetchPolicy::LocalOnly,
                publisher.as_str(),
                &input.application_id,
                &input.asset_id,
            )
            .await?;
            let stored = self
                .services
                .blob_service
                .put_owned_blob_file(&input.path, input.format.mime())
                .await?;
            let release = GameAssetReleaseV1 {
                application_id: input.application_id,
                asset_id: input.asset_id,
                revision: previous.map_or(1, |(release, _)| release.revision + 1),
                blob_hash: stored.hash,
                bytes: stored.bytes,
                format: input.format,
                published_at: Utc::now().timestamp_millis(),
            };
            let envelope =
                build_game_asset_release_envelope(self.services.keys.as_ref(), &release)?;
            let replica = author_replica_id(publisher.as_str());
            self.services.docs_sync.open_replica(&replica).await?;
            self.services
                .docs_sync
                .apply_doc_op(
                    &replica,
                    DocOp::SetJson {
                        key: record_key(&release.application_id, &release.asset_id),
                        value: serde_json::to_value(&envelope)?,
                    },
                )
                .await?;
            self.services
                .hint_transport
                .publish_hint(
                    &TopicId::new(topic_id),
                    GossipHint::SessionChanged {
                        topic_id: TopicId::new(topic_id),
                        session_id: format!(
                            "{}/{}/{}",
                            publisher.as_str(),
                            release.asset_id,
                            release.application_id
                        ),
                        object_kind: GAME_ASSET_HINT_KIND.into(),
                        sent_at: Some(Utc::now().timestamp_millis()),
                    },
                )
                .await?;
            Ok(view(&release, publisher.as_str(), None))
        }

        /// release を手元 → 発行者本人 → topic の参加者の順に読む。読めた読み先は blob の取得元の候補として覚える。
        /// `keep_newer_than` があれば、その版より新しい record だけを保持してほかの端末へ提供する(保持した新しい版を古い版で
        /// 上書きしない)。
        async fn read_game_asset(
            &self,
            topic_id: &str,
            publisher: &str,
            application_id: &str,
            asset_id: &str,
            keep_newer_than: Option<u64>,
        ) -> Result<Option<(GameAssetReleaseV1, Option<String>)>> {
            let blobs = self.services.blob_service.clone();
            self.read_author_object(publisher, Some(topic_id), |docs, policy| {
                let blobs = blobs.clone();
                async move {
                    let Some((release, author)) =
                        newest_release(docs.as_ref(), policy, publisher, application_id, asset_id)
                            .await?
                    else {
                        return Ok(None);
                    };
                    if let Some(provider) = docs.remote_reader_id() {
                        let _ = blobs
                            .learn_content_source(&release.blob_hash, &provider)
                            .await;
                    }
                    if keep_newer_than.is_some_and(|applied| release.revision > applied)
                        && policy != DocFetchPolicy::LocalOnly
                    {
                        docs.persist_verified_record(
                            &author_replica_id(publisher),
                            &record_key(application_id, asset_id),
                            author.as_deref(),
                            &[],
                        )
                        .await?;
                    }
                    Ok(Some((release, author)))
                }
            })
            .await
        }

        /// `asset_ids`(最大 `GAME_ASSET_LIST_LIMIT` 件)の最新の release を返す。読めなかったアセットは含めない。
        pub async fn list_game_assets(
            &self,
            topic_id: &str,
            publisher: &str,
            application_id: &str,
            asset_ids: &[String],
        ) -> Result<Vec<GameAssetReleaseView>> {
            validate_application_id(application_id)?;
            anyhow::ensure!(
                !asset_ids.is_empty() && asset_ids.len() <= GAME_ASSET_LIST_LIMIT,
                "asset_ids must list 1-{GAME_ASSET_LIST_LIMIT} assets"
            );
            for asset_id in asset_ids {
                validate_asset_id(asset_id)?;
            }
            let store = &self.services.projection_store;
            // 1 件の読みは読み先の期限まで待ちうるので、件の数だけ待たないよう並べて読む。
            let read =
                futures_util::future::try_join_all(asset_ids.iter().map(|asset_id| async move {
                    let Some((release, _)) = self
                        .read_game_asset(topic_id, publisher, application_id, asset_id, None)
                        .await?
                    else {
                        return anyhow::Ok(None);
                    };
                    let applied = store
                        .get_applied_game_asset(publisher, application_id, asset_id)
                        .await?
                        .map(|row| row.revision);
                    Ok(Some(view(&release, publisher, applied)))
                }))
                .await?;
            Ok(read.into_iter().flatten().collect())
        }

        /// 最新の release の検証済みの file を返す。適用中の版が最新ならそれを、新しければ取得・検証・適用して返す。
        /// 古い release では戻さない。release も適用中の版も無い・取得できないときは [`GameAssetUnavailable`]。
        pub async fn fetch_game_asset(
            &self,
            topic_id: &str,
            publisher: &str,
            application_id: &str,
            asset_id: &str,
        ) -> Result<GameAssetFileView> {
            validate_application_id(application_id)?;
            validate_asset_id(asset_id)?;
            let _fetching = self
                .lock_game_asset(publisher, application_id, asset_id)
                .await;
            self.fetch_game_asset_locked(topic_id, publisher, application_id, asset_id)
                .await
        }

        async fn lock_game_asset(
            &self,
            publisher: &str,
            application_id: &str,
            asset_id: &str,
        ) -> tokio::sync::OwnedMutexGuard<()> {
            self.services
                .game_asset_running
                .fetches
                .lock(&format!("{publisher}\0{application_id}\0{asset_id}"))
                .await
        }

        async fn fetch_game_asset_locked(
            &self,
            topic_id: &str,
            publisher: &str,
            application_id: &str,
            asset_id: &str,
        ) -> Result<GameAssetFileView> {
            let store = &self.services.projection_store;
            let applied = store
                .get_applied_game_asset(publisher, application_id, asset_id)
                .await?;
            let current = |row: &AppliedGameAssetRow| -> Result<GameAssetFileView> {
                let release: GameAssetReleaseV1 = serde_json::from_str(&row.release_json)?;
                Ok(GameAssetFileView {
                    release: view(&release, publisher, Some(row.revision)),
                    path: row.file_path.clone(),
                })
            };
            let read = self
                .read_game_asset(
                    topic_id,
                    publisher,
                    application_id,
                    asset_id,
                    Some(applied.as_ref().map_or(0, |row| row.revision)),
                )
                .await?;
            let (release, author) = match (read, &applied) {
                (Some((release, _)), Some(row)) if row.revision >= release.revision => {
                    return current(row);
                }
                (None, Some(row)) => return current(row),
                (None, None) => return Err(GameAssetUnavailable(GAME_ASSET_NOT_FOUND).into()),
                (Some(read), _) => read,
            };
            let dir = self.game_assets_dir()?.join(
                &blake3::hash(format!("{publisher}\0{application_id}\0{asset_id}").as_bytes())
                    .to_hex()[..32],
            );
            tokio::fs::create_dir_all(&dir).await?;
            let partial = dir.join(format!("{}.partial", release.revision));
            let path = dir.join(format!(
                "{}.{}",
                release.revision,
                release.format.extension()
            ));
            // 検証(hash は転送で、長さはここで)が済むまで、使える名前の file にしない。
            let fetched = self
                .services
                .blob_service
                .fetch_blob_ephemeral_to_file(
                    &release.blob_hash,
                    &partial,
                    Some(transfer_deadline(release.bytes)),
                )
                .await;
            if !matches!(fetched, Ok(Some(length)) if length == release.bytes) {
                let _ = tokio::fs::remove_file(&partial).await;
                // 新しい版を取れなくても、適用中の版はそのまま使える(更新の失敗で今のスキンを失わない)。
                if let Some(row) = &applied {
                    return current(row);
                }
                fetched?;
                return Err(GameAssetUnavailable(GAME_ASSET_UNAVAILABLE).into());
            }
            tokio::fs::rename(&partial, &path).await?;
            let blobs = &self.services.blob_service;
            if blobs.local_blob_status(&release.blob_hash).await?
                == kukuri_blob_service::BlobStatus::Missing
            {
                blobs
                    .put_remote_blob_file(&path, &release.blob_hash, false)
                    .await?;
            }
            let replica = author_replica_id(publisher);
            let mut refs = vec![("blob".to_string(), release.blob_hash.as_str().to_string())];
            if let Some(author) = &author {
                // remote cache の記録の key(`SqliteStore::remote_record_cache_key` と同じ形)。
                refs.push((
                    "record".into(),
                    format!(
                        "{}\0{}\0{author}",
                        replica.as_str(),
                        record_key(application_id, asset_id)
                    ),
                ));
            }
            let row = AppliedGameAssetRow {
                publisher_pubkey: publisher.into(),
                application_id: application_id.into(),
                asset_id: asset_id.into(),
                revision: release.revision,
                release_json: serde_json::to_string(&release)?,
                file_path: path.to_string_lossy().into_owned(),
                applied_at: Utc::now().timestamp_millis(),
            };
            if !store.apply_game_asset(&row, &refs).await? {
                // 同時に取得した別の要求が、同じか新しい版を先に適用した。
                let _ = tokio::fs::remove_file(&path).await;
                let latest = store
                    .get_applied_game_asset(publisher, application_id, asset_id)
                    .await?
                    .context("the applied game asset disappeared")?;
                return current(&latest);
            }
            // 前の版の file を消す(行の数で抑えられる。失敗しても続ける)。
            if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if entry.path() != path {
                        let _ = tokio::fs::remove_file(entry.path()).await;
                    }
                }
            }
            Ok(GameAssetFileView {
                release: view(&release, publisher, Some(release.revision)),
                path: row.file_path,
            })
        }

        /// 更新の知らせを受けた。新しい release を読めたら、適用中のアセットは背景で取得・適用して、まだ適用していない
        /// アセットはそのまま知らせる。
        pub(crate) async fn receive_game_asset_hint(
            &self,
            topic_id: &str,
            id: &str,
        ) -> Result<bool> {
            let mut parts = id.splitn(3, '/');
            let (Some(publisher), Some(asset_id), Some(application_id)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Ok(false);
            };
            if validate_application_id(application_id).is_err()
                || validate_asset_id(asset_id).is_err()
            {
                return Ok(false);
            }
            let Some((release, _)) = self
                .read_game_asset(topic_id, publisher, application_id, asset_id, None)
                .await?
            else {
                return Ok(false);
            };
            let applied = self
                .services
                .projection_store
                .get_applied_game_asset(publisher, application_id, asset_id)
                .await?;
            let update = GameAssetUpdate {
                publisher_pubkey: publisher.into(),
                application_id: application_id.into(),
                asset_id: asset_id.into(),
                revision: release.revision,
                path: None,
            };
            match applied {
                Some(row) if row.revision >= release.revision => return Ok(false),
                Some(_) => {
                    let key = format!("{publisher} {application_id} {asset_id}");
                    let waiting = &self.services.game_asset_running.waiting;
                    if !waiting
                        .lock()
                        .expect("game asset waiting")
                        .insert(key.clone())
                    {
                        return Ok(true);
                    }
                    let (app, topic) = (self.account_handle(), topic_id.to_owned());
                    n0_future::task::spawn(async move {
                        let (publisher, application_id, asset_id) = (
                            &update.publisher_pubkey,
                            &update.application_id,
                            &update.asset_id,
                        );
                        // 前面の取得か、先に届いた知らせの取得が済ませていれば知らせない。取得の間に届いた次の版の知らせは、
                        // 待った後に読み直して取る。
                        let _fetching = app
                            .lock_game_asset(publisher, application_id, asset_id)
                            .await;
                        // 取得を始めたら、次の知らせは新しい task を待たせる。
                        app.services
                            .game_asset_running
                            .waiting
                            .lock()
                            .expect("game asset waiting")
                            .remove(&key);
                        let before = app
                            .services
                            .projection_store
                            .get_applied_game_asset(publisher, application_id, asset_id)
                            .await
                            .ok()
                            .flatten()
                            .map_or(0, |row| row.revision);
                        if before >= update.revision {
                            return;
                        }
                        let fetched = app
                            .fetch_game_asset_locked(&topic, publisher, application_id, asset_id)
                            .await;
                        match fetched {
                            Ok(file) if file.release.revision > before => {
                                let _ = app.services.game_asset_updates.send(GameAssetUpdate {
                                    revision: file.release.revision,
                                    path: Some(file.path),
                                    ..update
                                });
                            }
                            Ok(_) => tracing::warn!(
                                revision = update.revision,
                                "the newer game asset could not be obtained; keeping the applied one"
                            ),
                            Err(error) => tracing::warn!(%error, "failed to update a game asset"),
                        }
                    });
                }
                None => {
                    let _ = self.services.game_asset_updates.send(update);
                }
            }
            Ok(true)
        }
    }
}
