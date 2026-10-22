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
use kukuri_desktop_runtime::SetTopicRetentionRequest;
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

fn input(name: &str) -> Value {
    match name {
        "set_topic_retention" => object(
            json!({"topic": string(), "retained": {"type": "boolean"}}),
            &["topic", "retained"],
        ),
        "get_topic_retention" => object(json!({}), &[]),
        _ => unreachable!("topic retention input schema"),
    }
}

fn output(name: &str) -> Value {
    match name {
        "set_topic_retention" => json!({"type": "null"}),
        "get_topic_retention" => object(
            json!({"topics": array(object(
                json!({
                    "topic": string(),
                    "retained_at": integer(),
                    "progress": array(object(
                        json!({"index": string(), "bucket": {"type": "integer", "minimum": 0},
                            "created_at": integer(), "object_id": string()}),
                        &["index", "bucket", "created_at", "object_id"],
                    )),
                }),
                &["topic", "retained_at", "progress"],
            ))}),
            &["topics"],
        ),
        _ => unreachable!("topic retention output schema"),
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Read, Write};
    [
        ("set_topic_retention", Write),
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
