use super::*;

pub(crate) async fn load_community_node_invite_code(
    db_path: &Path,
    mode: IdentityStorageMode,
    base_url: &str,
) -> Result<Option<String>> {
    load_optional_secret(db_path, mode, COMMUNITY_NODE_INVITE_CODE_PURPOSE, base_url).await
}

pub(crate) async fn persist_community_node_invite_code(
    db_path: &Path,
    mode: IdentityStorageMode,
    base_url: &str,
    invite_code: &str,
) -> Result<()> {
    persist_optional_secret(
        db_path,
        mode,
        COMMUNITY_NODE_INVITE_CODE_PURPOSE,
        base_url,
        invite_code,
    )
    .await
}

pub(crate) async fn delete_community_node_invite_code(
    db_path: &Path,
    mode: IdentityStorageMode,
    base_url: &str,
) -> Result<()> {
    crate::identity::delete_optional_secret(
        db_path,
        mode,
        COMMUNITY_NODE_INVITE_CODE_PURPOSE,
        base_url,
    )
    .await
}
