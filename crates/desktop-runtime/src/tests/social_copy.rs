//! #1998 AC-3: まとめの付け直しが、B の記録を飛ばすか書き直すか（ADR 0070 §4.2）。

use super::*;
use crate::runtime::{SocialCopyKind, SocialCopySource};

const TARGET: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

fn request() -> AuthorRequest {
    AuthorRequest {
        pubkey: TARGET.to_owned(),
    }
}

#[tokio::test]
async fn the_copy_rewrites_only_active_records_made_after_the_merge() -> Result<()> {
    let dir = tempdir()?;
    let a = DesktopRuntime::new(dir.path().join("a.db")).await?;
    a.follow_author(request()).await?;
    let a_key = a.local_author_pubkey();
    a.shutdown().await;
    let source = SocialCopySource::open(&dir.path().join("a.db"), &a_key).await?;
    let b = DesktopRuntime::new(dir.path().join("b.db")).await?;
    let me = b.local_author_pubkey();
    let edge = || async {
        b.store
            .get_follow_edge(&me, TARGET)
            .await
            .unwrap()
            .expect("the follow of B")
    };
    let page = |since: i64| b.copy_social_page(&source, SocialCopyKind::Follows, None, since);

    // 確定より前に B が付けた記録は、そのまま残す。
    b.follow_author(request()).await?;
    let before = edge().await;
    sleep(Duration::from_millis(5)).await;
    assert_eq!(page(before.updated_at + 1).await?.as_deref(), Some(TARGET));
    assert_eq!(edge().await.envelope_id, before.envelope_id);

    // 確定より後の有効な記録は、途中まで書いた付け直しとして書き直す。
    page(before.updated_at).await?;
    let rewritten = edge().await;
    assert_ne!(rewritten.envelope_id, before.envelope_id);

    // 確定より後に外した記録は、付け直さない。
    sleep(Duration::from_millis(5)).await;
    b.unfollow_author(request()).await?;
    let revoked = edge().await;
    page(0).await?;
    assert_eq!(edge().await.envelope_id, revoked.envelope_id);
    b.shutdown().await;
    Ok(())
}
