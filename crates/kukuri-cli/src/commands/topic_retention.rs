//! 公開 topic の保持(#1909)。CLI daemon だけの command で、GUI には無い。

use super::{
    command, command_error, decode, encode, host_guards, runtime,
    schema::{array, object},
};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};
use async_trait::async_trait;
use kukuri_desktop_runtime::{SetTopicRetentionLimitsRequest, SetTopicRetentionRequest};
use serde_json::{Value, json};
use std::sync::Arc;

struct Handler(&'static str);

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
            "set_topic_retention" => encode(
                runtime
                    .set_topic_retention(decode::<SetTopicRetentionRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "set_topic_retention_limits" => encode(
                runtime
                    .set_topic_retention_limits(decode::<SetTopicRetentionLimitsRequest>(payload)?)
                    .await
                    .map_err(command_error)?,
            ),
            "get_topic_retention" => {
                encode(runtime.get_topic_retention().await.map_err(command_error)?)
            }
            _ => unreachable!("登録済みのtopic retention command"),
        }
    }
}

fn string() -> Value {
    json!({"type": "string"})
}

fn integer() -> Value {
    json!({"type": "integer"})
}

fn count() -> Value {
    json!({"type": "integer", "minimum": 0})
}

fn input(name: &str) -> Value {
    match name {
        "set_topic_retention" => object(
            json!({"topic": string(), "retained": {"type": "boolean"}}),
            &["topic", "retained"],
        ),
        // 期間は 1〜365 日、容量は 1 GiB 以上(判断 1・2)。
        "set_topic_retention_limits" => object(
            json!({
                "retention_days": {"type": "integer", "minimum": 1, "maximum": 365},
                "capacity_bytes": {"type": "integer", "minimum": 1_u64 << 30},
            }),
            &["retention_days", "capacity_bytes"],
        ),
        "get_topic_retention" => object(json!({}), &[]),
        _ => unreachable!("topic retention input schema"),
    }
}

fn output(name: &str) -> Value {
    match name {
        "set_topic_retention" | "set_topic_retention_limits" => json!({"type": "null"}),
        "get_topic_retention" => object(
            json!({
                "retention_days": count(),
                "capacity_bytes": count(),
                "used_bytes": count(),
                "topics": array(object(
                json!({
                    "topic": string(),
                    "retained_at": integer(),
                    "progress": array(object(
                        json!({"index": string(), "bucket": count(),
                            "created_at": integer(), "object_id": string()}),
                        &["index", "bucket", "created_at", "object_id"],
                    )),
                    "pending_retries": count(),
                    "given_up": count(),
                    "used_bytes": count(),
                }),
                &["topic", "retained_at", "progress", "pending_retries", "given_up", "used_bytes"],
            ))}),
            &["retention_days", "capacity_bytes", "used_bytes", "topics"],
        ),
        _ => unreachable!("topic retention output schema"),
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Read, Write};
    [
        ("set_topic_retention", Write),
        ("set_topic_retention_limits", Write),
        ("get_topic_retention", Read),
    ]
    .into_iter()
    .map(|(name, effect)| {
        command(
            name,
            effect,
            false,
            false,
            host_guards(),
            (input(name), output(name)),
            Arc::new(Handler(name)),
        )
    })
    .collect()
}
