//! 自分の端末で hosting する Dome の heartbeat と参加の holder の所有(#1221 R2-C)。
//! heartbeat の task は registry が持ち、hosting の終了と account の shutdown で止める。

use super::*;
use kukuri_core::SpatialContextV1;

impl AppService {
    /// 自分の端末の hosting を終える。session・heartbeat・参加の holder を外す。
    pub(crate) async fn stop_owner_dome_hosting(&self, instance_id: &str) {
        self.dome_host_sessions.lock().await.remove(instance_id);
        let heartbeat = self
            .subscription_registry
            .dome_heartbeats
            .lock()
            .await
            .remove(instance_id);
        drop(heartbeat);
        self.release_scope_holder(&dome_holder(instance_id)).await;
    }

    pub(crate) async fn spawn_owner_dome_heartbeat_task(
        &self,
        context: SpatialContextV1,
        instance_id: String,
        session_id: String,
    ) {
        let sessions = Arc::clone(&self.dome_host_sessions);
        let hint_transport = Arc::clone(&self.services.hint_transport);
        let key = instance_id.clone();
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(
                kukuri_core::DOME_HOST_HEARTBEAT_INTERVAL_MILLIS as u64,
            ));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let signed = {
                    let sessions = sessions.lock().await;
                    let Some(runtime) = sessions.get(&instance_id) else {
                        break;
                    };
                    if runtime.session_id() != session_id {
                        break;
                    }
                    match runtime.signed_heartbeat(Utc::now().timestamp_millis()) {
                        Ok(signed) => signed,
                        Err(_) => break,
                    }
                };
                let hint = GossipHint::DomeHostHeartbeat {
                    topic_id: context.topic_id().clone(),
                    instance_id: instance_id.clone(),
                    heartbeat: Box::new(signed),
                };
                if hint_transport
                    .publish_hint(
                        &channel_hint_topic_for(context.topic_id().as_str(), context.channel_id()),
                        hint,
                    )
                    .await
                    .is_err()
                {
                    continue;
                }
            }
        });
        self.subscription_registry
            .dome_heartbeats
            .lock()
            .await
            .insert(key, AbortOnDropTask::new(handle));
    }
}
