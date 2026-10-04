//! #1221 R5-H: owner が受け取った private channel の参加・退出の表。新しい record だけで置き換え、
//! 退出は channel の全 epoch に及ぶ。参加中の pubkey は cursor で重複なく読む(両 backend で同じ結果)。
//! #1219 AC-2: (channel, epoch) の参加中の数と資格喪失(owner と mutual でない)の数を、行と follow の edge の
//! 書込みで保つ(SQLite は書込みで保ち、試験用の Memory は読むときに求める。両者が同じ数を返す)。

use super::*;
use crate::PrivateChannelRow;
use anyhow::Result;
use kukuri_core::FollowEdge;

fn row(
    epoch: &str,
    pubkey: &str,
    left_at: Option<i64>,
    updated_at: i64,
) -> PrivateChannelParticipantRow {
    PrivateChannelParticipantRow {
        channel_id: "channel".into(),
        epoch_id: epoch.into(),
        participant_pubkey: pubkey.into(),
        left_at,
        updated_at,
    }
}

fn follow(subject: &str, target: &str, status: FollowEdgeStatus, updated_at: i64) -> FollowEdge {
    FollowEdge {
        subject_pubkey: subject.into(),
        target_pubkey: target.into(),
        status,
        updated_at,
        envelope_id: EnvelopeId::from(format!("follow-{subject}-{target}-{updated_at}")),
    }
}

async fn participant_table_contract<S: Store + ProjectionStore>(store: &S) -> Result<()> {
    store
        .put_private_channel(
            &PrivateChannelRow {
                channel_key: "topic::channel".into(),
                topic_id: "topic".into(),
                channel_id: "channel".into(),
                label: "channel".into(),
                creator_pubkey: "owner".into(),
                owner_pubkey: "owner".into(),
                joined_via_pubkey: None,
                audience_kind: "friend_only".into(),
                current_epoch_id: "e1".into(),
                controller: None,
                joined: true,
                updated_at: 1,
                op_id: String::new(),
            },
            &[],
        )
        .await?;
    // a・c・d は owner と mutual、b は owner からの follow だけ(資格喪失)。
    for pubkey in ["a", "b", "c", "d"] {
        store
            .upsert_follow_edge(follow("owner", pubkey, FollowEdgeStatus::Active, 1))
            .await?;
        if pubkey != "b" {
            store
                .upsert_follow_edge(follow(pubkey, "owner", FollowEdgeStatus::Active, 1))
                .await?;
        }
    }
    for (epoch, pubkey) in [
        ("e1", "owner"),
        ("e1", "a"),
        ("e1", "b"),
        ("e1", "c"),
        ("e2", "a"),
        ("e2", "d"),
    ] {
        assert!(
            store
                .put_private_channel_participant(row(epoch, pubkey, None, 10))
                .await?
        );
    }
    // 古い record は置き換えない。
    assert!(
        !store
            .put_private_channel_participant(row("e1", "b", Some(5), 5))
            .await?
    );
    assert_eq!(
        store
            .private_channel_participant_counts("channel", "e1")
            .await?,
        (4, 1)
    );
    // 全 epoch の参加中の pubkey を重複なく、cursor で読む。
    let first = store
        .list_private_channel_participants("channel", "", 2)
        .await?;
    assert_eq!(first, vec!["a".to_string(), "b".to_string()]);
    let rest = store
        .list_private_channel_participants("channel", "b", 2)
        .await?;
    assert_eq!(rest, vec!["c".to_string(), "d".to_string()]);
    // follow を外すと、その公開鍵の現在の世代の行が資格喪失になり、戻すと外れる(双方が外した関係も資格喪失)。
    store
        .upsert_follow_edge(follow("c", "owner", FollowEdgeStatus::Revoked, 2))
        .await?;
    store
        .upsert_follow_edge(follow("owner", "c", FollowEdgeStatus::Revoked, 2))
        .await?;
    // #1219 AC-5: edge は向きごとに 1 件で読める。
    assert_eq!(
        store.get_follow_edge("owner", "c").await?,
        Some(follow("owner", "c", FollowEdgeStatus::Revoked, 2))
    );
    assert_eq!(store.get_follow_edge("c", "z").await?, None);
    assert_eq!(
        store
            .private_channel_participant_counts("channel", "e1")
            .await?,
        (4, 2)
    );
    store
        .upsert_follow_edge(follow("b", "owner", FollowEdgeStatus::Active, 3))
        .await?;
    assert_eq!(
        store
            .private_channel_participant_counts("channel", "e1")
            .await?,
        (4, 1)
    );
    // 現 epoch での退出は channel からの退出で、古い epoch の行も退出にする。
    assert!(
        store
            .put_private_channel_participant(row("e2", "a", Some(20), 20))
            .await?
    );
    assert_eq!(
        store
            .list_private_channel_participants("channel", "", 10)
            .await?,
        vec![
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "owner".to_string()
        ]
    );
    assert_eq!(
        store
            .private_channel_participant_counts("channel", "e1")
            .await?,
        (3, 1)
    );
    // 資格喪失の参加者の退出は、資格喪失の数からも外れる。
    assert!(
        store
            .put_private_channel_participant(row("e1", "c", Some(21), 21))
            .await?
    );
    assert_eq!(
        store
            .private_channel_participant_counts("channel", "e1")
            .await?,
        (2, 0)
    );
    // 退出した相手も含め、channel で一度でも行を持った相手は既知(B7。回転後の遅れた参加 record を受け付けない)。
    assert!(
        store
            .has_private_channel_participant("channel", "a")
            .await?
    );
    assert!(
        store
            .has_private_channel_participant("channel", "d")
            .await?
    );
    assert!(
        !store
            .has_private_channel_participant("channel", "z")
            .await?
    );
    assert!(!store.has_private_channel_participant("other", "a").await?);
    // #1219 AC-5: いずれかの channel に行のある相手（退出の行だけの相手を含む）と、channel で参加中の相手。
    assert!(store.has_private_channel_member("d").await?);
    assert!(store.has_private_channel_member("c").await?);
    assert!(!store.has_private_channel_member("z").await?);
    assert!(
        !store
            .is_active_private_channel_participant("other", "b")
            .await?
    );
    assert!(
        store
            .is_active_private_channel_participant("channel", "b")
            .await?
    );
    assert!(
        !store
            .is_active_private_channel_participant("channel", "a")
            .await?
    );
    // 退出より後の参加(再参加)は戻す。
    assert!(
        store
            .put_private_channel_participant(row("e2", "a", None, 30))
            .await?
    );
    assert_eq!(
        store
            .private_channel_participant_counts("channel", "e2")
            .await?,
        (2, 0)
    );
    assert_eq!(
        store
            .private_channel_participant_counts("other", "e1")
            .await?,
        (0, 0)
    );
    assert!(
        store
            .list_private_channel_participants("other", "", 10)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn private_channel_participants_keep_the_newest_record_and_page_by_pubkey() -> Result<()> {
    participant_table_contract(&SqliteStore::connect_memory().await?).await?;
    participant_table_contract(&MemoryStore::default()).await
}

/// 数は 1 行で読み、参加中の pubkey の page と follow の edge の書込みは、次の pubkey・channel を索引で 1 件ずつ引く
/// (参加者の表の件数にも、参加者が持つ過去の世代の行の数にも比例しない)。
#[tokio::test]
async fn participant_counts_and_stale_marks_use_bounded_index_ranges() -> Result<()> {
    use crate::sqlite::private_channel_keys::LIST_ROTATIONS;
    use crate::sqlite::social::{LIST_PARTICIPANTS, RESTALE_PARTICIPANT};
    use sqlx::Row;

    let store = SqliteStore::connect_memory().await?;
    for (query, index, binds) in [
        (
            LIST_PARTICIPANTS,
            "idx_private_channel_participants_active",
            3,
        ),
        (
            RESTALE_PARTICIPANT,
            "idx_private_channel_participants_member",
            1,
        ),
        (
            "SELECT active, stale FROM private_channel_participant_counts              WHERE channel_id = 'c' AND epoch_id = 'e'",
            "sqlite_autoindex_private_channel_participant_counts_1",
            0,
        ),
        (LIST_ROTATIONS, "idx_private_channel_epochs_rotation", 3),
    ] {
        let mut explain = sqlx::QueryBuilder::<sqlx::Sqlite>::new("EXPLAIN QUERY PLAN ");
        explain.push(query);
        let mut plan = explain.build();
        if binds > 0 {
            plan = plan.bind("");
        }
        if binds == 3 {
            plan = plan.bind("").bind(1_i64);
        }
        let plan = plan
            .fetch_all(store.pool())
            .await?
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(plan.contains(index), "{query}: {plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{query}: {plan}");
    }
    Ok(())
}
