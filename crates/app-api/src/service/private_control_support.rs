//! private channel の制御 record(metadata・policy・参加 record・handoff grant)の対象参照(#1221 R5-C)。
//!
//! 手元の次に、書き手(owner、token の発行者)の検証済み宛先と参加中の channel の gossip scope の peer から、
//! epoch の capability の証明つきで key 指定で読む。参加者の全件と、sync の再開による待機を使わない。

use super::remote_read_support::{read_local_then_remote, writer_readers};
use super::*;

impl AppService {
    /// private の制御 record を、手元の次に書き手と channel の gossip scope の有界な provider から読む(#1221 R5-C)。
    ///
    /// 要求は epoch の capability の証明つきで、書き手(owner、token の発行者)の検証済み宛先と、参加中の channel の
    /// gossip scope の peer だけへ送る。参加前(取込み中)は書き手だけ。namespace を sync しない。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn read_private_control<T, Fut>(
        &self,
        topic_id: &str,
        channel_id: &str,
        replica: &ReplicaId,
        secret_hex: &str,
        writers: &[&str],
        read: impl Fn(Arc<dyn DocsSync>, DocFetchPolicy) -> Fut,
    ) -> Result<Option<T>>
    where
        Fut: Future<Output = Result<Option<T>>>,
    {
        let local = self.current_author_pubkey();
        let readers = async {
            let mut secret = [0_u8; 32];
            if hex::decode_to_slice(secret_hex, &mut secret).is_err() {
                return Vec::new();
            }
            let scope = if self
                .joined_private_channel_state(topic_id, channel_id)
                .await
                .is_some()
            {
                self.hint_transport()
                    .topic_read_candidates(&private_channel_hint_topic(channel_id))
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let writers = writers
                .iter()
                .copied()
                .filter(|writer| *writer != local)
                .collect::<Vec<_>>();
            writer_readers(&self.services, replica, &writers, Some((secret, scope))).await
        };
        read_local_then_remote(self.services.docs_sync.clone(), readers, read).await
    }

    /// 参加中の channel の現在の epoch の制御 record を、手元の次に owner と channel の peer から読む。
    pub(crate) async fn read_joined_private_control<T, Fut>(
        &self,
        state: &JoinedPrivateChannelState,
        read: impl Fn(Arc<dyn DocsSync>, DocFetchPolicy) -> Fut,
    ) -> Result<Option<T>>
    where
        Fut: Future<Output = Result<Option<T>>>,
    {
        self.read_private_control(
            state.topic_id.as_str(),
            state.channel_id.as_str(),
            &current_private_channel_replica_id(state),
            state.current_epoch_secret_hex.as_str(),
            &[state.owner_pubkey.as_str()],
            read,
        )
        .await
    }

    /// epoch の参加に要る metadata・policy・owner と自分の参加 record を読む(#1221 R5-C)。
    /// 読めるまで 1 秒ごとに読み直し、10 秒で打ち切る。参加者の全件は読まない。
    pub(crate) async fn load_private_epoch_snapshot(
        &self,
        topic_id: &str,
        channel_id: &str,
        replica: &ReplicaId,
        secret_hex: &str,
        writers: &[&str],
        context: PrivateChannelSnapshotWaitContext,
    ) -> Result<PrivateEpochSnapshot> {
        let local = self.current_author_pubkey();
        let local = local.as_str();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(snapshot) = self
                    .read_private_control(
                        topic_id,
                        channel_id,
                        replica,
                        secret_hex,
                        writers,
                        |docs, policy| async move {
                            read_private_epoch_snapshot(docs.as_ref(), replica, local, policy).await
                        },
                    )
                    .await?
                {
                    return Ok::<_, anyhow::Error>(snapshot);
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        })
        .await
        .map_err(|_| context.timeout_error())?
    }

    /// 自分宛の handoff grant。手元に無ければ、手元の policy が rotation を示すときだけ owner と channel の peer から読む。
    pub(crate) async fn own_epoch_handoff_grant(
        &self,
        state: &JoinedPrivateChannelState,
    ) -> Result<Option<PrivateChannelEpochHandoffGrantDocV1>> {
        let local = &self.current_author_pubkey();
        let replica = &current_private_channel_replica_id(state);
        if let Some(grant) = fetch_private_channel_epoch_handoff_grant_from_replica(
            self.docs_sync(),
            replica,
            local,
            DocFetchPolicy::LocalOnly,
        )
        .await?
        {
            return Ok(Some(grant));
        }
        let rotated = fetch_private_channel_policy_from_replica(
            self.docs_sync(),
            replica,
            DocFetchPolicy::LocalOnly,
        )
        .await?
        .is_some_and(|policy| {
            policy.sharing_state == ChannelSharingState::Frozen && policy.rotated_at.is_some()
        });
        if !rotated {
            return Ok(None);
        }
        self.read_joined_private_control(state, |docs, policy| async move {
            fetch_private_channel_epoch_handoff_grant_from_replica(
                docs.as_ref(),
                replica,
                local,
                policy,
            )
            .await
        })
        .await
    }

    pub(crate) async fn private_channel_rotation_is_pending(
        &self,
        state: &JoinedPrivateChannelState,
    ) -> Result<bool> {
        let Some(grant) = self.own_epoch_handoff_grant(state).await? else {
            return Ok(false);
        };
        let payload = decrypt_private_channel_epoch_handoff_grant(self.keys(), &grant)?;
        Ok(payload.new_epoch_id != state.current_epoch_id)
    }
}
