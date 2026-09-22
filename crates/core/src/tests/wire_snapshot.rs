//! wire serde 形式の凍結テスト(WP-S3 T2)。
//!
//! GossipHint は serde_json のまま iroh gossip に流れ、KukuriPostEnvelopeContentV1
//! (payload_ref 含む)は署名済み content と SQLite projection の両方に永続される。
//! つまりここでの JSON 形(externally-tagged のタグ名 = variant 名 PascalCase、
//! フィールド名、null/省略の扱い)はネットワーク・ストレージ契約そのものである。
//!
//! - これらのテストが fail したら、テストではなく変更側(rename・serde 属性・
//!   依存更新)の互換影響を評価すること。受信側(transport)はデコード失敗を
//!   黙殺するため、GossipHint の形式変更は実行時エラーにならず静かに機能停止
//!   する — この snapshot が唯一の検出手段。
//! - PayloadRef のタグが PascalCase、ObjectVisibility 等が snake_case という
//!   混在は「意図的な凍結」である。統一リファクタは 1 バイト互換破壊になる。

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{
    ADULT_CONTENT_LABEL, BlobHash, ChannelId, ChannelRef, DirectMessageAckV1, EnvelopeId,
    GossipHint, HintObjectRef, KukuriEnvelope, KukuriPostEnvelopeContentV1,
    KukuriPostWithdrawalEnvelopeContentV1, ObjectStatus, ObjectVisibility, PayloadRef,
    PostWithdrawalReason, Pubkey, TimelineScope, TopicId, WithdrawalReasonVisibility,
    has_adult_content_label,
};

const PUBKEY_A: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const PUBKEY_B: &str = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

/// serialize が期待 JSON(生リテラル)と一致し、期待 JSON から元の値へ
/// デシリアライズできること(round-trip)を両方向で固定する。
fn assert_wire<T>(value: &T, expected: &str)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(
        serde_json::to_string(value).expect("serialize"),
        expected,
        "serialized form drifted from frozen wire snapshot"
    );
    let parsed: T = serde_json::from_str(expected).expect("frozen wire snapshot deserializes");
    assert_eq!(&parsed, value, "deserialized value drifted");
}

fn demo_topic() -> TopicId {
    TopicId::new("kukuri:topic:demo")
}

fn author() -> Pubkey {
    Pubkey::from(PUBKEY_A)
}

#[test]
fn gossip_hint_topic_objects_changed_snapshot() {
    assert_wire(
        &GossipHint::TopicObjectsChanged {
            topic_id: demo_topic(),
            objects: vec![HintObjectRef {
                object_id: "obj-1".to_string(),
                object_kind: "post".to_string(),
                docs_author: None,
            }],
        },
        r#"{"TopicObjectsChanged":{"topic_id":"kukuri:topic:demo","objects":[{"object_id":"obj-1","object_kind":"post"}]}}"#,
    );
}

// ADR 0053 §2: hint の docs author は任意の field。無ければ出力せず(旧 client と同じ wire)、旧 client の hint も読める。
#[test]
fn gossip_hint_object_ref_docs_author_is_optional_on_the_wire() {
    assert_wire(
        &GossipHint::TopicObjectsChanged {
            topic_id: demo_topic(),
            objects: vec![HintObjectRef {
                object_id: "obj-1".to_string(),
                object_kind: "post".to_string(),
                docs_author: Some("ab".repeat(32)),
            }],
        },
        r#"{"TopicObjectsChanged":{"topic_id":"kukuri:topic:demo","objects":[{"object_id":"obj-1","object_kind":"post","docs_author":"abababababababababababababababababababababababababababababababab"}]}}"#,
    );
    // field を知らない読み手(旧 client)と同じく、未知の field は無視して読める。
    #[derive(serde::Deserialize)]
    struct LegacyHintObjectRef {
        object_id: String,
        object_kind: String,
    }
    let legacy: LegacyHintObjectRef =
        serde_json::from_str(r#"{"object_id":"obj-1","object_kind":"post","docs_author":"ab"}"#)
            .expect("a reader that does not know the field");
    assert_eq!(
        (legacy.object_id.as_str(), legacy.object_kind.as_str()),
        ("obj-1", "post")
    );
}

#[test]
fn gossip_hint_thread_updated_snapshot() {
    assert_wire(
        &GossipHint::ThreadUpdated {
            root_id: EnvelopeId("root-1".to_string()),
            object_ids: vec![
                EnvelopeId("obj-1".to_string()),
                EnvelopeId("obj-2".to_string()),
            ],
        },
        r#"{"ThreadUpdated":{"root_id":"root-1","object_ids":["obj-1","obj-2"]}}"#,
    );
}

#[test]
fn gossip_hint_profile_updated_snapshot() {
    assert_wire(
        &GossipHint::ProfileUpdated { author: author() },
        r#"{"ProfileUpdated":{"author":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"}}"#,
    );
}

#[test]
fn gossip_hint_presence_snapshot() {
    assert_wire(
        &GossipHint::Presence {
            topic_id: demo_topic(),
            author: author(),
            ttl_ms: 30000,
        },
        r#"{"Presence":{"topic_id":"kukuri:topic:demo","author":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","ttl_ms":30000}}"#,
    );
}

#[test]
fn gossip_hint_typing_snapshot_with_and_without_root() {
    assert_wire(
        &GossipHint::Typing {
            topic_id: demo_topic(),
            root_id: Some(EnvelopeId("root-1".to_string())),
            author: author(),
            ttl_ms: 5000,
        },
        r#"{"Typing":{"topic_id":"kukuri:topic:demo","root_id":"root-1","author":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","ttl_ms":5000}}"#,
    );
    // None は省略ではなく null として出力される(古いピアとの互換もこの形)。
    assert_wire(
        &GossipHint::Typing {
            topic_id: demo_topic(),
            root_id: None,
            author: author(),
            ttl_ms: 5000,
        },
        r#"{"Typing":{"topic_id":"kukuri:topic:demo","root_id":null,"author":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","ttl_ms":5000}}"#,
    );
}

#[test]
fn gossip_hint_session_changed_snapshot() {
    assert_wire(
        &GossipHint::SessionChanged {
            topic_id: demo_topic(),
            session_id: "session-1".to_string(),
            object_kind: "live_session".to_string(),
        },
        r#"{"SessionChanged":{"topic_id":"kukuri:topic:demo","session_id":"session-1","object_kind":"live_session"}}"#,
    );
}

#[test]
fn gossip_hint_live_presence_snapshot() {
    assert_wire(
        &GossipHint::LivePresence {
            topic_id: demo_topic(),
            session_id: "session-1".to_string(),
            author: author(),
            ttl_ms: 15000,
        },
        r#"{"LivePresence":{"topic_id":"kukuri:topic:demo","session_id":"session-1","author":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","ttl_ms":15000}}"#,
    );
}

#[test]
fn gossip_hint_metaverse_room_event_snapshot() {
    assert_wire(
        &GossipHint::MetaverseRoomEvent {
            topic_id: demo_topic(),
            room_id: "room-1".to_string(),
            event: Box::new(KukuriEnvelope {
                id: EnvelopeId("id-1".to_string()),
                pubkey: author(),
                created_at: 1_700_000_000,
                kind: "kukuri:metaverse-room-event".to_string(),
                tags: vec![vec!["room".to_string(), "room-1".to_string()]],
                content: "{}".to_string(),
                sig: "sig-1".to_string(),
            }),
        },
        r#"{"MetaverseRoomEvent":{"topic_id":"kukuri:topic:demo","room_id":"room-1","event":{"id":"id-1","pubkey":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","created_at":1700000000,"kind":"kukuri:metaverse-room-event","tags":[["room","room-1"]],"content":"{}","sig":"sig-1"}}}"#,
    );
}

#[test]
fn gossip_hint_direct_message_frame_snapshot() {
    assert_wire(
        &GossipHint::DirectMessageFrame {
            topic_id: TopicId::new("kukuri:dm:abc"),
            dm_id: "dm-1".to_string(),
            message_id: "msg-1".to_string(),
            frame_hash: BlobHash::new("hash-1"),
        },
        r#"{"DirectMessageFrame":{"topic_id":"kukuri:dm:abc","dm_id":"dm-1","message_id":"msg-1","frame_hash":"hash-1"}}"#,
    );
}

#[test]
fn gossip_hint_direct_message_ack_snapshot() {
    assert_wire(
        &GossipHint::DirectMessageAck {
            topic_id: TopicId::new("kukuri:dm:abc"),
            ack: DirectMessageAckV1 {
                dm_id: "dm-1".to_string(),
                message_id: "msg-1".to_string(),
                sender: author(),
                recipient: Pubkey::from(PUBKEY_B),
                acked_at: 1_700_000_100,
                signature: "sig-1".to_string(),
            },
        },
        r#"{"DirectMessageAck":{"topic_id":"kukuri:dm:abc","ack":{"dm_id":"dm-1","message_id":"msg-1","sender":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","recipient":"c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5","acked_at":1700000100,"signature":"sig-1"}}}"#,
    );
}

#[test]
fn payload_ref_snapshot_is_intentionally_pascal_case() {
    // PayloadRef のタグ(InlineText / BlobText)は周囲の snake_case enum と
    // 異なる PascalCase のまま署名済み content と SQLite に永続済み。
    // 「snake_case に統一」する変更は既存データ・既存署名を全て壊す。
    assert_wire(
        &PayloadRef::InlineText {
            text: "hello".to_string(),
        },
        r#"{"InlineText":{"text":"hello"}}"#,
    );
    assert_wire(
        &PayloadRef::BlobText {
            hash: BlobHash::new("blake3-hash"),
            mime: "text/markdown".to_string(),
            bytes: 1234,
        },
        r#"{"BlobText":{"hash":"blake3-hash","mime":"text/markdown","bytes":1234}}"#,
    );
}

#[test]
fn object_enums_snapshot_snake_case() {
    assert_wire(
        &vec![
            ObjectVisibility::Public,
            ObjectVisibility::Community,
            ObjectVisibility::Room,
            ObjectVisibility::Private,
        ],
        r#"["public","community","room","private"]"#,
    );
    assert_wire(
        &vec![
            ObjectStatus::Active,
            ObjectStatus::Edited,
            ObjectStatus::Deleted,
            ObjectStatus::Tombstoned,
        ],
        r#"["active","edited","deleted","tombstoned"]"#,
    );
}

#[test]
fn channel_ref_and_timeline_scope_snapshot_internally_tagged() {
    assert_wire(&ChannelRef::Public, r#"{"kind":"public"}"#);
    assert_wire(
        &ChannelRef::PrivateChannel {
            channel_id: ChannelId::new("chan-1"),
        },
        r#"{"kind":"private_channel","channel_id":"chan-1"}"#,
    );
    assert_wire(&TimelineScope::Public, r#"{"kind":"public"}"#);
    // #1280: 複数の channel をまたぐ scope は API から閉じた。渡されたら読み込みで失敗する。
    assert!(serde_json::from_str::<TimelineScope>(r#"{"kind":"all_joined"}"#).is_err());
    assert_wire(
        &TimelineScope::Channel {
            channel_id: ChannelId::new("chan-1"),
        },
        r#"{"kind":"channel","channel_id":"chan-1"}"#,
    );
}

#[test]
fn post_envelope_content_snapshot() {
    assert_wire(
        &KukuriPostEnvelopeContentV1 {
            object_kind: "post".to_string(),
            topic_id: demo_topic(),
            channel_id: None,
            payload_ref: PayloadRef::InlineText {
                text: "hello".to_string(),
            },
            attachments: vec![],
            media_manifest_refs: vec![],
            visibility: ObjectVisibility::Public,
            reply_to: None,
            root_id: None,
            repost_of: None,
            content_labels: Vec::new(),
        },
        r#"{"object_kind":"post","topic_id":"kukuri:topic:demo","channel_id":null,"payload_ref":{"InlineText":{"text":"hello"}},"attachments":[],"media_manifest_refs":[],"visibility":"public","reply_to":null,"root_id":null,"repost_of":null}"#,
    );
}

// #858: 成人向け自己申告ラベルは content_labels として署名対象 content に載る。
// ラベルなしは省略され(skip_serializing_if)、旧 wire 形状と一致し続ける。
#[test]
fn post_envelope_content_labels_snapshot() {
    let labeled = KukuriPostEnvelopeContentV1 {
        object_kind: "post".to_string(),
        topic_id: demo_topic(),
        channel_id: None,
        payload_ref: PayloadRef::InlineText {
            text: "hello".to_string(),
        },
        attachments: vec![],
        media_manifest_refs: vec![],
        visibility: ObjectVisibility::Public,
        reply_to: None,
        root_id: None,
        repost_of: None,
        content_labels: vec![ADULT_CONTENT_LABEL.to_string()],
    };
    assert_wire(
        &labeled,
        r#"{"object_kind":"post","topic_id":"kukuri:topic:demo","channel_id":null,"payload_ref":{"InlineText":{"text":"hello"}},"attachments":[],"media_manifest_refs":[],"visibility":"public","reply_to":null,"root_id":null,"repost_of":null,"content_labels":["adult"]}"#,
    );
    assert!(has_adult_content_label(&labeled.content_labels));
    assert!(!has_adult_content_label(&[]));
}

#[test]
fn post_envelope_content_accepts_legacy_json_without_defaulted_fields() {
    // #[serde(default)] フィールドを持たない古い署名済み content が
    // 読み続けられること(後方互換)の凍結。
    let legacy = r#"{"object_kind":"post","topic_id":"kukuri:topic:demo","payload_ref":{"InlineText":{"text":"hello"}},"reply_to":null,"root_id":null}"#;
    let parsed: KukuriPostEnvelopeContentV1 =
        serde_json::from_str(legacy).expect("legacy content deserializes");
    assert_eq!(parsed.channel_id, None);
    assert_eq!(parsed.attachments, vec![]);
    assert_eq!(parsed.media_manifest_refs, Vec::<String>::new());
    assert_eq!(parsed.visibility, ObjectVisibility::Public);
    assert_eq!(parsed.repost_of, None);
}

#[test]
fn post_withdrawal_content_snapshot() {
    assert_wire(
        &KukuriPostWithdrawalEnvelopeContentV1 {
            target_object_id: EnvelopeId::from("object-1"),
            target_author: author(),
            topic_id: demo_topic(),
            channel_id: None,
            generation: 1,
            replacement_object_id: None,
            reason_visibility: WithdrawalReasonVisibility::Public,
            reason: Some(PostWithdrawalReason::AuthorRequest),
        },
        r#"{"target_object_id":"object-1","target_author":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","topic_id":"kukuri:topic:demo","channel_id":null,"generation":1,"replacement_object_id":null,"reason_visibility":"public","reason":"author_request"}"#,
    );
}
