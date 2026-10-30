//! この端末で適用したゲームのアセットの release(#1980)。適用中の版の blob と記録を保護し、ほかの端末へ提供する。

use super::*;
use crate::AppliedGameAssetRow;

impl SqliteStore {
    pub(super) async fn applied_game_asset(
        &self,
        publisher_pubkey: &str,
        application_id: &str,
        asset_id: &str,
    ) -> Result<Option<AppliedGameAssetRow>> {
        let row = sqlx::query_as::<_, (i64, String, String, i64)>(
            "SELECT revision, release_json, file_path, applied_at FROM applied_game_assets \
             WHERE publisher_pubkey = ?1 AND application_id = ?2 AND asset_id = ?3",
        )
        .bind(publisher_pubkey)
        .bind(application_id)
        .bind(asset_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(revision, release_json, file_path, applied_at)| AppliedGameAssetRow {
                publisher_pubkey: publisher_pubkey.into(),
                application_id: application_id.into(),
                asset_id: asset_id.into(),
                revision: revision as u64,
                release_json,
                file_path,
                applied_at,
            },
        ))
    }

    pub(super) async fn apply_game_asset_row(
        &self,
        row: &AppliedGameAssetRow,
        refs: &[(String, String)],
    ) -> Result<bool> {
        let mut update = self.begin_protected_ref_update().await?;
        let applied = sqlx::query(
            "INSERT INTO applied_game_assets \
             (publisher_pubkey, application_id, asset_id, revision, release_json, file_path, applied_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(publisher_pubkey, application_id, asset_id) DO UPDATE SET \
             revision = excluded.revision, release_json = excluded.release_json, \
             file_path = excluded.file_path, applied_at = excluded.applied_at \
             WHERE excluded.revision > applied_game_assets.revision",
        )
        .bind(&row.publisher_pubkey)
        .bind(&row.application_id)
        .bind(&row.asset_id)
        .bind(i64::try_from(row.revision)?)
        .bind(&row.release_json)
        .bind(&row.file_path)
        .bind(row.applied_at)
        .execute(&mut *update.tx)
        .await?
        .rows_affected()
            > 0;
        if applied {
            self.set_refs_in(
                &mut update,
                &format!(
                    "game_asset:{}:{}:{}",
                    row.publisher_pubkey, row.application_id, row.asset_id
                ),
                refs,
            )
            .await?;
        }
        self.commit_protected_ref_update(update).await?;
        Ok(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LiveGameProjectionStore;

    fn row(revision: u64, hash: &str) -> AppliedGameAssetRow {
        AppliedGameAssetRow {
            publisher_pubkey: "p".repeat(64),
            application_id: "com.kukuri.test".into(),
            asset_id: "skin".into(),
            revision,
            release_json: format!("{{\"revision\":{revision}}}"),
            file_path: format!("skin/{hash}.glb"),
            applied_at: revision as i64,
        }
    }

    async fn protected(store: &SqliteStore) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT cache_key FROM remote_content_cache_protected_ref WHERE ref_id LIKE 'game_asset:%' ORDER BY cache_key",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap()
    }

    /// 新しい版だけを適用して保護を付け替え、古い版では行も保護も変えない(#1980)。
    #[tokio::test]
    async fn only_a_newer_release_is_applied_and_takes_over_the_protection() {
        let store = SqliteStore::connect_memory().await.unwrap();
        let refs = |hash: &str| {
            vec![
                ("blob".to_string(), hash.to_string()),
                ("record".to_string(), format!("record-{hash}")),
            ]
        };
        assert!(
            store
                .apply_game_asset(&row(2, "b2"), &refs("b2"))
                .await
                .unwrap()
        );
        assert_eq!(protected(&store).await, vec!["b2", "record-b2"]);

        assert!(
            !store
                .apply_game_asset(&row(1, "b1"), &refs("b1"))
                .await
                .unwrap(),
            "an older release"
        );
        assert!(
            !store
                .apply_game_asset(&row(2, "b1"), &refs("b1"))
                .await
                .unwrap(),
            "the same revision"
        );
        assert_eq!(protected(&store).await, vec!["b2", "record-b2"]);

        assert!(
            store
                .apply_game_asset(&row(3, "b3"), &refs("b3"))
                .await
                .unwrap()
        );
        assert_eq!(
            protected(&store).await,
            vec!["b3", "record-b3"],
            "the old version is no longer protected"
        );
        let applied = store
            .get_applied_game_asset(&"p".repeat(64), "com.kukuri.test", "skin")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (applied.revision, applied.file_path.as_str()),
            (3, "skin/b3.glb")
        );
    }
}
