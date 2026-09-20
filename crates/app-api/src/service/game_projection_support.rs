//! ScoreGame の canonical state と projection の書込み順を揃える。

use super::*;
use std::sync::Weak;
use tokio::sync::OwnedMutexGuard;

/// Store の一意キー(room_id)と同じ単位。handles の clone 間でも共有する。
#[derive(Default)]
pub(crate) struct GameRoomProjectionLocks {
    rooms: Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl GameRoomProjectionLocks {
    pub(crate) async fn lock(&self, room_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut rooms = self.rooms.lock().await;
            rooms.retain(|_, lock| lock.strong_count() > 0);
            match rooms.get(room_id).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    rooms.insert(room_id.to_string(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }
}

pub(crate) async fn hydrate_game_room_from_record(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    mut record: DocRecord,
) -> Result<bool> {
    // A moving docs pointer may invalidate more than one fetch. Bound re-resolution;
    // false keeps the existing caller's retry/recovery path, rather than claiming success.
    for _ in 0..session_projection_retry_attempts() {
        // 行は、署名された manifest(metaverse room は Dome の id)と、読んだ replica の topic / channel に照らして
        // 確かめた room から作る(#1252)。検証に通らない record は warn を出して `false` を返し、エラーにしない。
        let Some(verified) = verify_game_room_record(
            services.docs_sync.as_ref(),
            services.blob_service.as_ref(),
            replica,
            topic_id,
            &record,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        else {
            return Ok(false);
        };
        services
            .projection_store
            .mark_blob_status(
                &verified.state().current_manifest.hash,
                BlobCacheStatus::Available,
            )
            .await?;
        let state = verified.state();
        let manifest = verified.manifest();
        // Slow blob I/O precedes the room lock. Other rooms never wait for it.
        // Metaverse keeps its existing projection/lifecycle behavior.
        let projection_guard = if manifest.room_kind == GameRoomKind::ScoreGame {
            Some(services.game_room_projections.lock(&state.room_id).await)
        } else {
            None
        };
        let row = game_projection_row(&verified);
        if projection_guard.is_some() {
            // 同じ key には docs author ごとの record がありうる。先頭の 1 件だけと比べると、不正な record を 1 件置くだけで
            // 正しい room の反映を止められる(#1252)。候補が現在の record のどれかと一致すれば、まだ最新とみなす。
            let mut current = services
                .docs_sync
                .query_replica_exact_bounded(
                    replica,
                    record.key.as_str(),
                    MAX_ENVELOPE_RECORDS_PER_OBJECT,
                    DocFetchPolicy::LocalOnly,
                )
                .await?;
            if current.is_empty() {
                return Ok(false);
            }
            if !current.iter().any(|current| current.value == record.value) {
                // Neither timestamps nor hash ordering decide freshness: docs does.
                // Drop this guard before fetching the current candidate's blob.
                record = current.swap_remove(0);
                continue;
            }
            if let Some(mut cached) = services
                .projection_store
                .list_topic_game_rooms(topic_id)
                .await?
                .into_iter()
                .find(|cached| cached.room_id == row.room_id)
            {
                cached.derived_at = row.derived_at;
                if cached == row {
                    return Ok(true);
                }
            }
        }
        // For ScoreGame, canonical comparison and commit share the local writer lock.
        services
            .projection_store
            .upsert_game_room_cache(row)
            .await?;
        return Ok(true);
    }
    Ok(false)
}
