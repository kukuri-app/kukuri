use serde_json::{Value, json};

use super::{
    content_schema, media_output,
    schema::{self, array, nullable},
};

fn string() -> Value {
    json!({"type": "string"})
}
fn integer() -> Value {
    json!({"type": "integer"})
}
fn boolean() -> Value {
    json!({"type": "boolean"})
}
fn optional_string() -> Value {
    nullable(string())
}
fn optional_integer() -> Value {
    nullable(integer())
}

/// 共有viewのOptionもnullとしてserializeされるため、出力では全フィールドを宣言する。
fn view(properties: Value) -> Value {
    let required = properties
        .as_object()
        .expect("viewのpropertiesはobjectとして定義する")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

pub(super) fn output(name: &str) -> Value {
    match name {
        "create_post" | "withdraw_post" | "create_repost" => string(),
        "list_timeline" | "list_thread" | "list_profile_timeline" => view(
            json!({"items": array(post()), "next_cursor": nullable(schema::cursor()), "unavailable_count": {"type": "integer", "minimum": 0}}),
        ),
        "get_blob_preview_url" | "get_blob_media_payload" => media_output::output_schema(),
        "bookmark_post" => bookmark(),
        "list_bookmarked_posts" => view(json!({"items": array(bookmark()),
            "newer_cursor": nullable(content_schema::bookmark_cursor()),
            "older_cursor": nullable(content_schema::bookmark_cursor())})),
        "remove_bookmarked_post" | "remove_bookmarked_custom_reaction" => json!({"type": "null"}),
        "resolve_community_index_posts" => view(
            json!({"entries": array(view(json!({"key": string(), "post": nullable(post()),
            "capabilities": view(json!({"open_thread": boolean(), "reply": boolean(), "repost": boolean(), "quote_repost": boolean(),
                "react": boolean(), "copy_link": boolean(), "bookmark": boolean(), "withdraw": boolean()}))})))}),
        ),
        "get_content_display_settings" | "set_adult_content_display_enabled" => {
            view(json!({"adult_content_enabled": boolean()}))
        }
        "toggle_reaction" => view(
            json!({"target_object_id": string(), "source_replica_id": string(),
            "reaction_summary": array(reaction(Some("count"))), "my_reactions": array(reaction(None))}),
        ),
        "list_my_custom_reaction_assets" | "list_bookmarked_custom_reactions" => {
            array(custom_asset())
        }
        "list_recent_reactions" => array(reaction(Some("updated_at"))),
        "create_custom_reaction_asset" | "bookmark_custom_reaction" => custom_asset(),
        "get_my_profile" | "set_my_profile" => profile(),
        "follow_author"
        | "unfollow_author"
        | "get_author_social_view"
        | "mute_author"
        | "unmute_author"
        | "block_author"
        | "unblock_author" => author(),
        "list_social_connections" => array(author()),
        "list_notifications" => array(notification()),
        "mark_notification_read" | "mark_all_notifications_read" | "get_notification_status" => {
            view(json!({"unread_count": content_schema::unsigned()}))
        }
        _ => panic!("content commandのoutput schemaが未定義: {name}"),
    }
}

fn provenance() -> Value {
    view(
        json!({"canonical_source": string(), "observed_via": array(view(json!({
        "node_base_url": string(), "capability": string(), "observed_at": integer()})))}),
    )
}

fn custom_asset() -> Value {
    view(content_schema::custom_reaction_fields())
}

fn reaction(extra: Option<&str>) -> Value {
    let mut fields = json!({"reaction_key_kind": string(), "normalized_reaction_key": string(),
        "emoji": optional_string(), "custom_asset": nullable(custom_asset())});
    if let Some(extra) = extra {
        fields[extra] = integer();
    }
    view(fields)
}

fn profile() -> Value {
    view(
        json!({"pubkey": string(), "name": optional_string(), "display_name": optional_string(),
        "about": optional_string(), "picture_asset": nullable(schema::profile_asset()), "updated_at": integer()}),
    )
}

fn author() -> Value {
    view(
        json!({"author_pubkey": string(), "name": optional_string(), "display_name": optional_string(),
        "about": optional_string(), "picture_asset": nullable(schema::profile_asset()), "updated_at": optional_integer(),
        "following": boolean(), "followed_by": boolean(), "mutual": boolean(), "friend_of_friend": boolean(),
        "friend_of_friend_via_pubkeys": array(string()), "provenance": nullable(provenance()),
        "muted": boolean(), "blocking": boolean(), "blocked_by": boolean()}),
    )
}

fn bookmark() -> Value {
    view(json!({"bookmarked_at": integer(), "post": post()}))
}

fn post() -> Value {
    view(
        json!({"object_id": string(), "envelope_id": string(), "author_pubkey": string(),
        "author_name": optional_string(), "author_display_name": optional_string(),
        "author_picture_asset": nullable(schema::profile_asset()), "following": boolean(), "followed_by": boolean(),
        "mutual": boolean(), "friend_of_friend": boolean(), "provenance": nullable(provenance()),
        "withdrawal": nullable(view(json!({"withdrawn_at": integer(), "replacement_object_id": optional_string(),
            "reason_visibility": string(), "reason": optional_string()}))),
        "content": string(), "content_status": {"enum": ["Missing", "Available", "Pinned"]},
        "attachments": array(schema::attachment()), "content_labels": array(string()), "created_at": integer(),
        "reply_to": optional_string(), "reply_preview": nullable(reply_preview()), "root_id": optional_string(),
        "object_kind": string(), "published_topic_id": optional_string(), "origin_topic_id": optional_string(),
        "repost_of": nullable(repost_source()), "repost_commentary": optional_string(), "is_threadable": boolean(),
        "channel_id": optional_string(), "audience_label": string(), "reaction_summary": array(reaction(Some("count"))),
        "my_reactions": array(reaction(None))}),
    )
}

fn reply_preview() -> Value {
    view(json!({"object_id": string(), "topic": string(),
        "author": view(json!({"pubkey": string(), "name": optional_string(), "display_name": optional_string(),
            "picture_asset": nullable(schema::profile_asset())})),
        "content": string(), "content_status": {"enum": ["Missing", "Available", "Pinned"]},
        "attachments": array(schema::attachment()), "content_labels": array(string()),
        "root_id": optional_string(), "reply_to": optional_string()}))
}

fn repost_source() -> Value {
    view(
        json!({"source_object_id": string(), "source_topic_id": string(), "source_author_pubkey": string(),
        "source_author_name": optional_string(), "source_author_display_name": optional_string(),
        "source_author_picture_asset": nullable(schema::profile_asset()), "source_object_kind": string(),
        "content": string(), "attachments": array(schema::attachment()), "content_labels": array(string()),
        "reply_to": optional_string(), "root_id": optional_string()}),
    )
}

fn notification() -> Value {
    view(
        json!({"notification_id": string(), "kind": {"enum": ["mention", "reply", "repost", "quote_repost", "direct_message", "followed"]},
        "actor_pubkey": string(), "actor_name": optional_string(), "actor_display_name": optional_string(),
        "actor_picture_asset": nullable(schema::profile_asset()), "source_envelope_id": optional_string(),
        "source_replica_id": optional_string(), "topic_id": optional_string(), "channel_id": optional_string(),
        "object_id": optional_string(), "thread_root_object_id": optional_string(), "dm_id": optional_string(),
        "message_id": optional_string(), "preview_text": optional_string(), "content_labels": nullable(array(string())),
        "created_at": integer(), "received_at": integer(), "read_at": optional_integer()}),
    )
}
