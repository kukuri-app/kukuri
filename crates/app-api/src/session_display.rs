//! 表示要求だけがsession manifestのremote取得を開始する。
pub use crate::service::session_projection::SessionCandidateView;
use crate::service::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SessionDisplayRequest {
    pub topic: String,
    pub scope: TimelineScope,
    pub replica_id: String,
    pub session_id: String,
    pub kind: String,
    pub observer: String,
    pub visible: bool,
    #[serde(default)]
    pub retry: bool,
}

impl AppService {
    pub async fn list_session_candidates(
        &self,
        topic: &str,
        scope: TimelineScope,
    ) -> Result<Vec<SessionCandidateView>> {
        let replicas = self.scope_replicas(topic, &scope).await?;
        // 候補は購読event/固定窓から得た作業集合のみ。未検証情報でsession projectionを作らない。
        Ok(self
            .services
            .session_projections
            .candidates(topic, &replicas)
            .await)
    }

    pub async fn set_session_display(&self, request: SessionDisplayRequest) -> Result<()> {
        let _access = self.services.content_save_access.lock().await;
        let replica = ReplicaId::new(request.replica_id.clone());
        let key = format!("sessions/{}/{}/state", request.kind, request.session_id);
        anyhow::ensure!(
            crate::service::hydration_support::is_session_state_key(&key),
            "invalid session key"
        );
        if !request.visible {
            self.services
                .session_projections
                .visibility(
                    &request.topic,
                    &replica,
                    &key,
                    &request.observer,
                    false,
                    false,
                )
                .await?;
            self.services
                .session_projections
                .schedule(&self.services)
                .await;
            return Ok(());
        }
        let allowed = self.scope_replicas(&request.topic, &request.scope).await?;
        let replicas = if request.replica_id.is_empty() {
            allowed.clone()
        } else {
            anyhow::ensure!(
                allowed.contains(&replica),
                "session replica is outside the requested scope"
            );
            vec![replica]
        };
        for replica in replicas {
            self.services
                .session_projections
                .visibility(
                    &request.topic,
                    &replica,
                    &key,
                    &request.observer,
                    true,
                    request.retry,
                )
                .await?;
            crate::service::hydration_support::hydrate_session_key(
                &self.services,
                &request.topic,
                &replica,
                &key,
            )
            .await?;
        }
        self.services
            .session_projections
            .schedule(&self.services)
            .await;
        drop(_access);
        // #1221 R5-H: 表示した session は、手元に無ければ provider から exact に読む(旧 sync で届くのを待たない)。
        let kind = if request.kind == "live" {
            "live-session"
        } else {
            "game-session"
        };
        if let Err(error) = self
            .read_session(&request.topic, &request.session_id, kind)
            .await
        {
            tracing::warn!(%error, "failed to read the displayed session");
        }
        Ok(())
    }
}
