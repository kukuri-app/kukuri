use serde_json::{Value, json};

use super::{
    media,
    schema::{array, channel_ref, cursor, object, timeline_scope},
};

pub(super) fn input(name: &str) -> Value {
    match name {
        "create_post" => object(
            json!({"topic": {"type": "string"}, "content": {"type": "string"},
            "reply_to": {"type": "string"}, "channel_ref": channel_ref(), "attachments": array(media::input_schema()),
            "content_labels": array(json!({"type": "string"}))}),
            &["topic", "content"],
        ),
        "withdraw_post" => object(
            json!({"topic": {"type": "string"}, "object_id": {"type": "string"},
            "channel_ref": channel_ref(), "replacement_object_id": {"type": "string"},
            "reason_visibility": {"enum": ["public", "private"]}, "reason": {"enum": ["author_request", "correction", "privacy", "other"]}}),
            &["topic", "object_id", "reason_visibility"],
        ),
        "create_repost" => object(
            json!({"topic": {"type": "string"}, "source_topic": {"type": "string"},
            "source_object_id": {"type": "string"}, "commentary": {"type": "string"}}),
            &["topic", "source_topic", "source_object_id"],
        ),
        "resolve_community_index_posts" => object(
            json!({"entries": array(object(json!({
            "key": {"type": "string"}, "topic": {"type": "string"}, "object_id": {"type": "string"},
            "author_pubkey": {"type": "string"}, "channel_ref": channel_ref()
        }), &["key", "topic", "object_id", "author_pubkey", "channel_ref"]))}),
            &["entries"],
        ),
        "bookmark_post" => object(
            json!({"topic": {"type": "string"}, "object_id": {"type": "string"}, "channel_ref": channel_ref()}),
            &["topic", "object_id"],
        ),
        "remove_bookmarked_post" => {
            object(json!({"object_id": {"type": "string"}}), &["object_id"])
        }
        "list_bookmarked_posts" => object(json!({"cursor": bookmark_cursor()}), &[]),
        "list_timeline" => object(
            json!({"topic": {"type": "string"}, "scope": timeline_scope(), "cursor": cursor(), "limit": unsigned()}),
            &["topic"],
        ),
        "list_thread" => object(
            json!({"topic": {"type": "string"}, "thread_id": {"type": "string"}, "cursor": cursor(), "limit": unsigned()}),
            &["topic", "thread_id"],
        ),
        "list_profile_timeline" => object(
            json!({"pubkey": {"type": "string"}, "cursor": cursor(), "limit": unsigned()}),
            &["pubkey"],
        ),
        "get_blob_preview_url" | "get_blob_media_payload" => {
            let mut fields = json!({"hash": {"type": "string"}, "mime": {"type": "string"},
                "output_path": {"type": "string", "description": "新規出力ファイルの絶対path。既存ファイルは上書きしない。"}});
            if name == "get_blob_preview_url" {
                fields["metaverse_kind"] = json!({"enum": ["vrm", "glb", "texture", "other"]});
            } else {
                fields["source_object_id"] = json!({"type": "string"});
            }
            object(fields, &["hash", "mime", "output_path"])
        }
        "set_adult_content_display_enabled" => {
            object(json!({"enabled": {"type": "boolean"}}), &["enabled"])
        }
        "toggle_reaction" => object(
            json!({"target_topic_id": {"type": "string"}, "target_object_id": {"type": "string"},
            "reaction_key": reaction_key(), "channel_ref": channel_ref()}),
            &["target_topic_id", "target_object_id", "reaction_key"],
        ),
        "list_recent_reactions" => object(json!({"limit": unsigned()}), &[]),
        "create_custom_reaction_asset" => object(
            json!({"upload": media::input_schema(), "search_key": {"type": "string"},
            "crop_rect": object(json!({"x": unsigned(), "y": unsigned(), "size": unsigned()}), &["x", "y", "size"])}),
            &["upload", "crop_rect", "search_key"],
        ),
        "bookmark_custom_reaction" => object(
            custom_reaction_fields(),
            &[
                "asset_id",
                "owner_pubkey",
                "blob_hash",
                "search_key",
                "mime",
                "bytes",
                "width",
                "height",
            ],
        ),
        "remove_bookmarked_custom_reaction" => {
            object(json!({"asset_id": {"type": "string"}}), &["asset_id"])
        }
        "set_my_profile" => object(
            json!({"name": {"type": "string"}, "display_name": {"type": "string"},
            "about": {"type": "string"}, "picture_upload": media::input_schema(), "clear_picture": {"type": "boolean", "default": false}}),
            &[],
        ),
        "follow_author"
        | "unfollow_author"
        | "get_author_social_view"
        | "mute_author"
        | "unmute_author"
        | "block_author"
        | "unblock_author" => object(json!({"pubkey": {"type": "string"}}), &["pubkey"]),
        "list_social_connections" => object(
            json!({"kind": {"enum": ["following", "followed", "muted", "blocking", "blocked_by"]}}),
            &["kind"],
        ),
        "mark_notification_read" => object(
            json!({"notification_id": {"type": "string"}}),
            &["notification_id"],
        ),
        "get_content_display_settings"
        | "list_my_custom_reaction_assets"
        | "list_bookmarked_custom_reactions"
        | "get_my_profile"
        | "list_notifications"
        | "mark_all_notifications_read"
        | "get_notification_status" => object(json!({}), &[]),
        _ => panic!("content commandのinput schemaが未定義: {name}"),
    }
}

pub(super) fn bookmark_cursor() -> Value {
    object(
        json!({"bookmarked_at": {"type": "integer"}, "source_object_id": {"type": "string"}}),
        &["bookmarked_at", "source_object_id"],
    )
}

pub(super) fn unsigned() -> Value {
    json!({"type": "integer", "minimum": 0})
}

pub(super) fn custom_reaction_fields() -> Value {
    json!({"asset_id": {"type": "string"}, "owner_pubkey": {"type": "string"}, "blob_hash": {"type": "string"},
        "search_key": {"type": "string"}, "mime": {"type": "string"}, "bytes": unsigned(), "width": unsigned(), "height": unsigned()})
}

pub(super) fn reaction_key() -> Value {
    let mut fields = custom_reaction_fields();
    fields["kind"] = json!({"enum": ["emoji", "custom_asset"]});
    fields["emoji"] = json!({"type": "string"});
    let mut schema = object(fields, &["kind"]);
    schema["description"] = json!(
        "emojiはemoji必須。custom_assetはasset_id/owner_pubkey/blob_hash/search_key/mime/bytes/width/height必須。共有DTOで条件を検証する。"
    );
    schema
}
