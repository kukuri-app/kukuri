use super::{command, command_error, decode, encode, host_guards, network_schema, runtime};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};
use async_trait::async_trait;
use kukuri_desktop_runtime::{
    ConnectivityPeersRequest, DesiredSubscription, DesiredSubscriptionScope,
    ImportPeerTicketRequest, SetChannelGossipEnabledRequest, SetDiscoverySeedsRequest,
    SetPublicBlobDiscoveryRequest, SetTopicGossipEnabledRequest, UnsubscribeTopicRequest,
};
use serde_json::Value;
use std::sync::Arc;

struct Handler(&'static str);

#[derive(serde::Deserialize)]
struct PublicChannelSubscriptionRequest {
    topic: String,
    channel_id: String,
    enabled: bool,
}

#[async_trait]
impl CommandHandler for Handler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        _: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let runtime = runtime(&context)?;
        match self.0 {
            "get_sync_status" => encode(runtime.get_sync_status().await.map_err(command_error)?),
            "list_connectivity_peers" => encode(
                runtime
                    .list_connectivity_peers(decode::<ConnectivityPeersRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "get_discovery_config" => encode(
                runtime
                    .get_discovery_config()
                    .await
                    .map_err(command_error)?,
            ),
            "get_local_peer_ticket" => {
                encode(runtime.local_peer_ticket().await.map_err(command_error)?)
            }
            "import_peer_ticket" => encode(
                runtime
                    .import_peer_ticket(decode::<ImportPeerTicketRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "set_discovery_seeds" => encode(
                runtime
                    .set_discovery_seeds(decode::<SetDiscoverySeedsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "set_public_blob_discovery" => encode(
                runtime
                    .set_public_blob_discovery(decode::<SetPublicBlobDiscoveryRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "set_topic_gossip_enabled" => {
                let request: SetTopicGossipEnabledRequest = decode(payload)?;
                let subscription = request.enabled.then(|| DesiredSubscription {
                    topic: request.topic.clone(),
                    scope: DesiredSubscriptionScope::Public,
                });
                runtime
                    .set_topic_gossip_enabled(request)
                    .await
                    .map_err(command_error)?;
                if let Some(subscription) = subscription {
                    context
                        .host
                        .expect("runtime取得済み")
                        .add_desired_subscription(subscription)
                        .await
                        .map_err(|error| command_error(error.into()))?;
                }
                encode(())
            }
            // #1982: 公開スレッド（公開 channel）の投稿・リアクションの知らせを購読する（desired として保存し、再起動後も
            // 続ける）。topic の購読と同じ上限に数える。
            "set_public_channel_subscription" => {
                let request: PublicChannelSubscriptionRequest = decode(payload)?;
                if !kukuri_core::ChannelId::new(&request.channel_id).is_public_channel() {
                    return Err(ProtocolError::new(
                        crate::protocol::error_code::VALIDATION_FAILED,
                        "channel_id is not a public channel",
                    ));
                }
                let host = context.host.expect("runtime取得済み");
                let subscription = DesiredSubscription {
                    topic: request.topic,
                    scope: DesiredSubscriptionScope::Channel {
                        channel_id: request.channel_id,
                    },
                };
                if request.enabled {
                    host.add_desired_subscription(subscription).await
                } else {
                    host.remove_desired_subscription(&subscription).await
                }
                .map_err(|error| command_error(error.into()))?;
                encode(())
            }
            "set_channel_gossip_enabled" => encode(
                runtime
                    .set_channel_gossip_enabled(decode::<SetChannelGossipEnabledRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "unsubscribe_topic" => {
                let request: UnsubscribeTopicRequest = decode(payload)?;
                let host = context.host.expect("runtime取得済み");
                let desired = host
                    .desired_subscriptions()
                    .await
                    .map_err(|error| command_error(error.into()))?;
                let subscriptions = desired
                    .iter()
                    .filter(|subscription| subscription.topic == request.topic)
                    .collect::<Vec<_>>();
                // 保存済みの購読も解除し、再起動で意図せず再購読しない。
                for subscription in subscriptions {
                    host.remove_desired_subscription(subscription)
                        .await
                        .map_err(|error| command_error(error.into()))?;
                }
                // topic の停止設定も消す(#1221 R2-D)。
                runtime
                    .unsubscribe_topic(request)
                    .await
                    .map_err(command_error)?;
                encode(())
            }
            _ => unreachable!("登録済みのnetwork command"),
        }
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Read, Write};
    [
        ("get_sync_status", Read),
        ("list_connectivity_peers", Read),
        ("get_discovery_config", Read),
        ("get_local_peer_ticket", Read),
        ("import_peer_ticket", Write),
        ("set_discovery_seeds", Write),
        ("set_public_blob_discovery", Write),
        ("unsubscribe_topic", Write),
        ("set_topic_gossip_enabled", Write),
        ("set_channel_gossip_enabled", Write),
        ("set_public_channel_subscription", Write),
    ]
    .into_iter()
    .map(|(name, effect)| {
        command(
            name,
            effect,
            false,
            false,
            host_guards(),
            (network_schema::input(name), network_schema::output(name)),
            Arc::new(Handler(name)),
        )
    })
    .collect()
}
