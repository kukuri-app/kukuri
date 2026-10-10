use std::fs::OpenOptions;
use std::io::Write;

use anyhow::{Context, Result};
use kukuri_cn_core::{
    LegalDataCipher, export_legal_hold, initialize_database, release_legal_hold, start_legal_hold,
};
use sqlx::PgPool;

use crate::LegalHoldAction;

pub(super) async fn run(pool: &PgPool, action: LegalHoldAction) -> Result<()> {
    initialize_database(pool).await?;
    match action {
        LegalHoldAction::Start {
            target_kind,
            target_id,
            data_categories,
            basis,
            release_condition,
            actor,
        } => {
            let hold = start_legal_hold(
                pool,
                &target_kind,
                &target_id,
                &data_categories,
                &basis,
                &release_condition,
                &actor,
                chrono::Utc::now(),
            )
            .await?;
            println!("legal hold started: {}", hold.id);
        }
        LegalHoldAction::Release { id, actor } => {
            release_legal_hold(pool, &id, &actor, chrono::Utc::now()).await?;
            println!("legal hold released: {id}");
        }
        LegalHoldAction::Export { id, actor, output } => {
            if !output.is_absolute() {
                anyhow::bail!("--output must be an absolute path");
            }
            let cipher = LegalDataCipher::from_key_material(
                &std::env::var("COMMUNITY_NODE_LEGAL_DATA_KEY")
                    .context("COMMUNITY_NODE_LEGAL_DATA_KEY is required")?,
            )?;
            let export = export_legal_hold(pool, &cipher, &id, &actor, chrono::Utc::now()).await?;
            let bytes = serde_json::to_vec_pretty(&export)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .with_context(|| {
                    format!(
                        "failed to create new export {}; existing files are never overwritten",
                        output.display()
                    )
                })?;
            file.write_all(&bytes)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!("legal hold export written: {}", output.display());
        }
    }
    Ok(())
}
