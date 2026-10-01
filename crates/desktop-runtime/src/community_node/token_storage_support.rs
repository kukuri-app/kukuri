use super::*;

pub(crate) async fn load_community_node_token(
    db_path: &Path,
    mode: IdentityStorageMode,
    base_url: &str,
) -> Result<Option<StoredCommunityNodeToken>> {
    let Some(raw) =
        load_optional_secret(db_path, mode, COMMUNITY_NODE_TOKEN_PURPOSE, base_url).await?
    else {
        return Ok(None);
    };
    let token = serde_json::from_str::<StoredCommunityNodeToken>(&raw)
        .context("failed to decode persisted community-node token")?;
    Ok(Some(token))
}

pub(crate) async fn persist_community_node_token(
    db_path: &Path,
    mode: IdentityStorageMode,
    base_url: &str,
    token: &StoredCommunityNodeToken,
) -> Result<()> {
    let encoded = serde_json::to_string(token).context("failed to encode community-node token")?;
    persist_optional_secret(
        db_path,
        mode,
        COMMUNITY_NODE_TOKEN_PURPOSE,
        base_url,
        encoded.as_str(),
    )
    .await
}
