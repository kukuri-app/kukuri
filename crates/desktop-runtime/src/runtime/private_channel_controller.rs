//! private channel の鍵更新の担当（ADR 0018 §8）の引取り（#1219 AC-4）。
//!
//! 担当でない owner の端末は、明示の操作で担当を引き取る。backup から復元した account は、復元の準備が db の隣に
//! 印を置き、その account の最初の起動の背景 task が自分の channel の担当を 64 件ずつ引き取って、終えたら印を消す
//! （途中で止まれば、次の起動でやり直す。既に引き取った channel は飛ばす）。

use std::path::{Path, PathBuf};

use anyhow::Result;
use kukuri_app_api::{AppService, PrivateChannelControllerTake};

use crate::requests::TakePrivateChannelControllerRequest;
use crate::storage::{delete_file, read_file};

use super::DesktopRuntime;

/// 復元の後の引取りの印（`<db>.controller-claim`）。
pub(crate) fn controller_claim_marker(db: &Path) -> PathBuf {
    db.with_extension("controller-claim")
}

/// 印があれば、自分の channel の担当をすべて引き取り、印を消す。
pub(super) async fn claim_restored_controllers(db: &Path, app: &AppService) -> Result<()> {
    let marker = controller_claim_marker(db);
    if read_file(&marker).await?.is_none() {
        return Ok(());
    }
    let mut after = String::new();
    while let Some(next) = app.claim_private_channel_controllers(&after, 64).await? {
        after = next;
    }
    delete_file(&marker).await
}

impl DesktopRuntime {
    pub async fn take_private_channel_controller(
        &self,
        request: TakePrivateChannelControllerRequest,
    ) -> Result<PrivateChannelControllerTake> {
        self.app_service
            .take_private_channel_controller(request.topic.as_str(), request.channel_id.as_str())
            .await
    }
}
