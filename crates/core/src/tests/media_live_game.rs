use crate::*;

#[test]
fn media_manifest_envelope_uses_protocol_object_kind() {
    let keys = generate_keys();
    let envelope = build_media_manifest_envelope(
        &keys,
        &TopicId::new("kukuri:topic:media"),
        &KukuriMediaManifestV1 {
            manifest_id: "manifest-1".into(),
            owner_pubkey: keys.public_key(),
            created_at: 1,
            items: vec![MediaManifestItem {
                blob_hash: BlobHash::new("blob-1"),
                mime: "image/png".into(),
                size: 123,
                width: Some(10),
                height: Some(10),
                duration_ms: None,
                codec: None,
                thumbnail_blob_hash: None,
            }],
        },
    )
    .expect("manifest envelope");

    envelope.verify().expect("signature verification");
    assert_eq!(envelope.kind, "media-manifest");
}

#[test]
fn fixed_dome_v1_geometry_matches_the_product_contract() {
    let spec = fixed_dome_v1();

    assert_eq!(spec.spec_id, FIXED_DOME_SPEC_ID);
    assert_eq!(spec.inner_radius_cm, 2_000);
    assert_eq!(spec.outer_radius_cm, 2_200);
    assert_eq!(spec.apex_height_cm, 2_000);
    assert_eq!(spec.opening_width_cm, 500);
    assert_eq!(spec.opening_height_cm, 1_000);
    assert_eq!(spec.opening_arch_radius_cm, 250);
    assert_eq!(spec.connection_zone_depth_cm, 1_500);
    assert_eq!(spec.connection_boundary_offset_cm, 2_850);
    assert_eq!(spec.adjacent_dome_center_distance_cm, 5_700);
    assert_eq!(spec.endpoints.len(), 4);
    assert!(spec.physics_enabled);
    assert_eq!(spec.gravity_direction, [0, -1, 0]);
}

#[test]
fn fixed_dome_fallback_capsule_contains_the_bounding_box() {
    let collider =
        fallback_capsule_collider([-100, 0, -50], [100, 400, 50]).expect("valid bounding box");

    assert_eq!(
        collider,
        MetaverseColliderV1::Capsule {
            center: [0, 200, 0],
            radius: 100,
            half_height: 100,
        }
    );
}

#[test]
fn fixed_dome_rejects_unbounded_avatar_collider_descriptors() {
    assert!(
        validate_metaverse_collider(&MetaverseColliderV1::Capsule {
            center: [0, 90, 0],
            radius: -1,
            half_height: 90,
        })
        .is_err()
    );
    assert!(
        validate_metaverse_collider(&MetaverseColliderV1::Cuboid {
            center: [i64::MAX, 0, 0],
            half_extents: [25, 90, 25],
        })
        .is_err()
    );
}

#[test]
fn fixed_dome_rejects_the_legacy_scene_payload() {
    let legacy = serde_json::json!({
        "world_version": 1,
        "max_peers": 8,
        "scene": {
            "ground": "default",
            "shared_object": {
                "object_id": "mvp-object-1",
                "asset_ref": null,
                "primitive_fallback": "cube",
                "position": [0, 50, -240],
                "rotation": [0, 0, 0],
                "scale": [100, 100, 100],
                "updated_by": "owner",
                "updated_at": 1
            }
        },
        "default_spawn": { "position": [0, 0, 260], "rotation": [0, 180, 0] },
        "asset_refs": [],
        "chat_history": []
    });

    assert!(serde_json::from_value::<MetaverseRoomStateV1>(legacy).is_err());
}

#[test]
fn fixed_dome_customization_validator_rejects_unsupported_values() {
    let mut customization = DomeCustomizationV1::default();
    customization.environment.gravity_milli = 0;
    assert!(validate_dome_customization(&customization).is_err());

    let mut customization = DomeCustomizationV1::default();
    customization.persistent_props[0].visual_only = true;
    assert!(validate_dome_customization(&customization).is_err());

    let mut customization = DomeCustomizationV1::default();
    customization.persistent_props[0].position = [1_500, 1_500, 0];
    assert!(validate_dome_customization(&customization).is_err());

    let mut customization = DomeCustomizationV1::default();
    customization.surface.wall_texture = Some(MetaverseAssetRef {
        kind: MetaverseAssetKind::Glb,
        blob_hash: "not-a-texture".to_string(),
        mime_type: Some("model/gltf-binary".to_string()),
        size_bytes: Some(1),
        name: Some("mesh.glb".to_string()),
        budget_metadata: None,
    });
    assert!(validate_dome_customization(&customization).is_err());
}

#[test]
fn fixed_dome_environment_interpolation_is_numeric_and_bounded() {
    let from = DomeEnvironmentV1::default();
    let to = DomeEnvironmentV1 {
        key_light_milli: 4_000,
        ambient_light_milli: 1_000,
        fog_density_micros: 20_000,
        gravity_milli: 4_000,
    };

    assert_eq!(interpolate_dome_environment(&from, &to, 0), from);
    assert_eq!(interpolate_dome_environment(&from, &to, 1_000), to);
    assert_eq!(
        interpolate_dome_environment(&from, &to, 500),
        DomeEnvironmentV1 {
            key_light_milli: 3_200,
            ambient_light_milli: 700,
            fog_density_micros: 14_000,
            gravity_milli: 6_900,
        }
    );
    assert_eq!(interpolate_dome_environment(&from, &to, u16::MAX), to);
}

fn dome_preset_ref(owner: &Pubkey) -> DomePresetRefV1 {
    DomePresetRefV1 {
        preset_id: "preset-1".into(),
        owner_pubkey: owner.clone(),
        revision: 1,
        manifest_blob_hash: "a".repeat(64),
        manifest_mime: DOME_PRESET_MANIFEST_MIME.into(),
        manifest_bytes: 512,
    }
}

fn dome_instance(
    owner: &Pubkey,
    instance_id: &str,
    context: SpatialContextV1,
) -> DomeInstanceManifestV1 {
    DomeInstanceManifestV1 {
        instance_id: instance_id.into(),
        spatial_context: context,
        owner_pubkey: owner.clone(),
        preset_ref: dome_preset_ref(owner),
        title: "Dome".into(),
        description: String::new(),
        max_peers: Some(8),
        default_spawn: MetaverseRoomSpawnV1 {
            position: [0, 0, 260],
            rotation: [0, 180, 0],
        },
        generation: 1,
        status: DomeInstanceStatusV1::Active,
        relationship_detach: None,
        replacement_instance_id: None,
        chat_history: Vec::new(),
        updated_at: 1,
    }
}

#[test]
fn spatial_context_distinguishes_topic_and_channel() {
    let topic = SpatialContextV1::Topic {
        topic_id: TopicId::new("kukuri:topic:context"),
    };
    let channel = SpatialContextV1::Channel {
        topic_id: TopicId::new("kukuri:topic:context"),
        channel_id: ChannelId::new("channel-1"),
    };

    assert_eq!(topic.canonical_id(), "topic:kukuri:topic:context");
    assert_eq!(
        channel.canonical_id(),
        "channel:kukuri:topic:context:channel-1"
    );
    assert_ne!(topic, channel);
}

#[test]
fn dome_relationships_require_active_attached_instances_in_one_context() {
    let owner = generate_keys().public_key();
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new("kukuri:topic:context"),
    };
    let left = dome_instance(&owner, "left", context.clone());
    let mut right = dome_instance(&owner, "right", context);
    assert!(validate_dome_relationship_scope(&left, &right).is_ok());

    right.spatial_context = SpatialContextV1::Topic {
        topic_id: TopicId::new("kukuri:topic:other"),
    };
    assert!(validate_dome_relationship_scope(&left, &right).is_err());

    right.spatial_context = left.spatial_context.clone();
    right.relationship_detach = Some(DomeRelationshipDetachV1 {
        move_id: "move-1".into(),
        instance_generation: 1,
        detached_at: 2,
    });
    assert!(validate_dome_relationship_scope(&left, &right).is_err());
}

#[test]
fn dome_preset_instance_and_move_envelopes_bind_owner_and_context() {
    let keys = generate_keys();
    let owner = keys.public_key();
    let preset = DomePresetManifestV1 {
        preset_id: "preset-1".into(),
        owner_pubkey: owner.clone(),
        revision: 1,
        dome: MetaverseDomeV1::default(),
        asset_refs: Vec::new(),
        updated_at: 1,
    };
    let instance = dome_instance(
        &owner,
        "source",
        SpatialContextV1::Topic {
            topic_id: TopicId::new("kukuri:topic:source"),
        },
    );
    let move_record = DomeMoveRecordV1 {
        move_id: "move-1".into(),
        owner_pubkey: owner,
        source_instance_id: "source".into(),
        source_context: instance.spatial_context.clone(),
        source_generation: 1,
        target_instance_id: "target".into(),
        target_context: SpatialContextV1::Channel {
            topic_id: TopicId::new("kukuri:topic:target"),
            channel_id: ChannelId::new("channel-1"),
        },
        target_generation: 1,
        preset_ref: instance.preset_ref.clone(),
        phase: DomeMovePhaseV1::Preparing,
        failure_reason: None,
        updated_at: 2,
    };

    for envelope in [
        build_dome_preset_envelope(&keys, &preset).expect("preset envelope"),
        build_dome_instance_envelope(&keys, &instance).expect("instance envelope"),
        build_dome_move_envelope(&keys, &move_record).expect("move envelope"),
    ] {
        envelope.verify().expect("signature");
    }
    assert!(validate_dome_move_record(&move_record).is_ok());
}

#[test]
fn metaverse_events_are_bound_to_context_generation_and_active_instance() {
    let owner = generate_keys().public_key();
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new("kukuri:topic:event-context"),
    };
    let mut instance = dome_instance(&owner, "instance-1", context.clone());
    let content = MetaverseRoomEventEnvelopeContentV1 {
        event_id: "event-1".into(),
        topic_id: TopicId::new("kukuri:topic:event-context"),
        channel_id: None,
        room_id: instance.instance_id.clone(),
        spatial_context: context,
        instance_generation: instance.generation,
        session_id: instance.instance_id.clone(),
        peer_id: "peer-1".into(),
        seq: 1,
        sent_at: 1,
        event: MetaverseRoomEventV1::PresenceLeave {
            room_id: instance.instance_id.clone(),
            peer_id: "peer-1".into(),
            left_at: 1,
        },
    };

    assert!(validate_metaverse_room_event_for_instance(&content, &instance).is_ok());

    let mut stale = content.clone();
    stale.instance_generation += 1;
    assert!(validate_metaverse_room_event_for_instance(&stale, &instance).is_err());

    instance.status = DomeInstanceStatusV1::Tombstoned;
    assert!(validate_metaverse_room_event_for_instance(&content, &instance).is_err());
}

#[test]
fn spatial_audio_gain_uses_direct_and_opening_distance() {
    assert_eq!(spatial_audio_gain_milli(50), 1_000);
    assert_eq!(spatial_audio_gain_milli(400), 250);
    assert_eq!(spatial_audio_distance_cm([0, 0, 0], [300, 0, 400]), 500);
    assert_eq!(
        connection_opening_audio_distance_cm([0, 0, 0], [300, 0, 400], [0, 0, 0], [0, 0, 200]),
        700
    );
    assert_eq!(
        metaverse_audio_broadcast_path("instance-1", "session-1"),
        "metaverse-audio/instance-1/session-1"
    );
}
