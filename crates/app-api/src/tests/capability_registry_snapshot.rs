use crate::service::projection_support::joined_private_channel_state_from_capability;
use crate::{PrivateChannelCapability, PrivateChannelController, PrivateChannelEpochCapability};
use kukuri_core::ChannelAudienceKind;

// private channel capability registry の永続 JSON(desktop-runtime が identity storage に
// purpose="private-channel-capabilities" / key="registry" で保存する
// `Vec<PrivateChannelCapability>` の serde 表現)は、既存ユーザーのディスク上に存在する
// 凍結境界である。フィールド名・形状の変更は migration なしに不可。
// このテストはその形状を固定し、意図しない serde 変更を CI で検出する。

fn full_capability() -> PrivateChannelCapability {
    PrivateChannelCapability {
        topic_id: "kukuri:topic:snapshot".into(),
        channel_id: "channel-1".into(),
        label: "snapshot".into(),
        creator_pubkey: "11".repeat(32),
        owner_pubkey: "22".repeat(32),
        joined_via_pubkey: Some("33".repeat(32)),
        audience_kind: ChannelAudienceKind::FriendPlus,
        current_epoch_id: "epoch-1700000000000-aabbccdd".into(),
        current_epoch_secret_hex: "44".repeat(32),
        archived_epochs: vec![PrivateChannelEpochCapability {
            epoch_id: "epoch-1600000000000-aabbccdd".into(),
            namespace_secret_hex: "55".repeat(32),
        }],
        rotation_required: true,
        participant_count: 3,
        stale_participant_count: 1,
        namespace_secret_hex: "44".repeat(32),
        controller: Some(Some(PrivateChannelController {
            device_id: "66".repeat(32),
            generation: 1,
            transfer_to: None,
        })),
    }
}

#[test]
fn capability_registry_json_shape_is_frozen() {
    let registry = vec![full_capability()];
    let encoded = serde_json::to_string(&registry).expect("encode registry");
    let expected = format!(
        concat!(
            "[{{",
            "\"topic_id\":\"kukuri:topic:snapshot\",",
            "\"channel_id\":\"channel-1\",",
            "\"label\":\"snapshot\",",
            "\"creator_pubkey\":\"{c}\",",
            "\"owner_pubkey\":\"{o}\",",
            "\"joined_via_pubkey\":\"{j}\",",
            "\"audience_kind\":\"friend_plus\",",
            "\"current_epoch_id\":\"epoch-1700000000000-aabbccdd\",",
            "\"current_epoch_secret_hex\":\"{s}\",",
            "\"archived_epochs\":[{{",
            "\"epoch_id\":\"epoch-1600000000000-aabbccdd\",",
            "\"namespace_secret_hex\":\"{a}\"",
            "}}],",
            "\"rotation_required\":true,",
            "\"participant_count\":3,",
            "\"stale_participant_count\":1,",
            "\"namespace_secret_hex\":\"{s}\",",
            "\"controller\":{{\"device_id\":\"{d}\",\"generation\":1,\"transfer_to\":null}}",
            "}}]"
        ),
        c = "11".repeat(32),
        o = "22".repeat(32),
        j = "33".repeat(32),
        s = "44".repeat(32),
        a = "55".repeat(32),
        d = "66".repeat(32),
    );
    assert_eq!(encoded, expected);

    let decoded: Vec<PrivateChannelCapability> =
        serde_json::from_str(&encoded).expect("decode registry");
    assert_eq!(decoded, registry);
}

#[test]
fn capability_registry_accepts_minimal_legacy_json() {
    // 過去バージョンが書いた最小形(必須 4 フィールドのみ)と、legacy 世代の
    // 「namespace_secret_hex のみ・current_epoch_* 空」形の両方が読めること。
    let minimal = r#"[{
        "topic_id": "kukuri:topic:legacy",
        "channel_id": "channel-legacy",
        "label": "legacy",
        "creator_pubkey": "aa"
    }]"#;
    let decoded: Vec<PrivateChannelCapability> =
        serde_json::from_str(minimal).expect("decode minimal registry");
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].owner_pubkey, "");
    assert_eq!(decoded[0].audience_kind, ChannelAudienceKind::InviteOnly);
    assert!(decoded[0].archived_epochs.is_empty());
    // #1219 W6: 担当の欄が無い保存(本変更前)と、担当が不明(null)を区別して読む。
    assert_eq!(decoded[0].controller, None);
    let unknown = minimal.replace("\"aa\"", "\"aa\", \"controller\": null");
    let decoded: Vec<PrivateChannelCapability> =
        serde_json::from_str(&unknown).expect("decode unknown controller");
    assert_eq!(decoded[0].controller, Some(None));

    let legacy = r#"[{
        "topic_id": "kukuri:topic:legacy",
        "channel_id": "channel-legacy",
        "label": "legacy",
        "creator_pubkey": "aa",
        "namespace_secret_hex": "bb"
    }]"#;
    let decoded: Vec<PrivateChannelCapability> =
        serde_json::from_str(legacy).expect("decode legacy registry");
    let state = joined_private_channel_state_from_capability(decoded[0].clone())
        .expect("restore legacy capability");
    // legacy 形: current_epoch_id は legacy epoch、secret は namespace_secret_hex へ
    // フォールバック、owner は creator へフォールバックする。
    assert_eq!(state.current_epoch_secret_hex, "bb");
    assert_eq!(state.owner_pubkey, "aa");
}

#[test]
fn diagnostics_fields_do_not_affect_restored_state() {
    // rotation_required / participant_count / stale_participant_count は復元時に
    // 一切読まれない(= 永続化時に既定値へ落としても復元同値)。boundary PR の
    // state-only スナップショット化が凍結境界を壊さないことの固定。
    let populated = full_capability();
    let mut defaulted = populated.clone();
    defaulted.rotation_required = false;
    defaulted.participant_count = 0;
    defaulted.stale_participant_count = 0;

    let state_populated =
        joined_private_channel_state_from_capability(populated).expect("restore populated");
    let state_defaulted =
        joined_private_channel_state_from_capability(defaulted).expect("restore defaulted");
    assert_eq!(state_populated, state_defaulted);
}
