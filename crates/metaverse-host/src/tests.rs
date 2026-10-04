use kukuri_core::{
    DomeDirection, DomeHostTargetV1, DomeHostingLeaseV1, DomeInstanceStatusV1, DomePresetRefV1,
    DomeTransitionAccessDecisionV1, DomeTransitionAdmissionRequestV1, MetaverseColliderV1,
    MetaverseDomeV1, MetaversePersistentPropV1, MetaversePrimitive, MetaverseRoomSpawnV1,
    SpatialContextV1, TopicId, build_signed_dome_hosting_lease,
};

use super::*;

mod participant_ledger;

fn fixture() -> (
    KukuriKeys,
    SignedDomeHostingLeaseV1,
    DomeInstanceManifestV1,
    DomePresetManifestV1,
) {
    let owner = KukuriKeys::generate();
    let context = SpatialContextV1::Topic {
        topic_id: TopicId("kukuri:topic:runtime".into()),
    };
    let preset = DomePresetManifestV1 {
        preset_id: "preset-1".into(),
        owner_pubkey: owner.public_key(),
        revision: 1,
        dome: MetaverseDomeV1::default(),
        asset_refs: Vec::new(),
        updated_at: 1_000,
    };
    let preset_ref = DomePresetRefV1 {
        preset_id: preset.preset_id.clone(),
        owner_pubkey: owner.public_key(),
        revision: preset.revision,
        manifest_blob_hash: "manifest-hash".into(),
        manifest_mime: "application/vnd.kukuri.dome-preset+json".into(),
        manifest_bytes: 100,
    };
    let instance = DomeInstanceManifestV1 {
        instance_id: "dome-1".into(),
        spatial_context: context.clone(),
        owner_pubkey: owner.public_key(),
        preset_ref,
        title: "Dome".into(),
        description: String::new(),
        max_peers: Some(8),
        default_spawn: MetaverseRoomSpawnV1 {
            position: [0, 0, 0],
            rotation: [0, 0, 0],
        },
        generation: 1,
        status: DomeInstanceStatusV1::Active,
        relationship_detach: None,
        replacement_instance_id: None,
        chat_history: Vec::new(),
        updated_at: 1_000,
    };
    let lease = build_signed_dome_hosting_lease(
        &owner,
        DomeHostingLeaseV1 {
            lease_id: "lease-1".into(),
            spatial_context: context,
            instance_id: instance.instance_id.clone(),
            instance_generation: 1,
            owner_pubkey: owner.public_key(),
            host: DomeHostTargetV1::OwnerDevice {
                endpoint_id: "endpoint-1".into(),
                host_pubkey: owner.public_key(),
            },
            manifest_blob_hash: "manifest-hash".into(),
            manifest_version: 1,
            epoch: 1,
            issued_at: 1_000,
            expires_at: 20_000,
        },
    )
    .unwrap();
    (owner, lease, instance, preset)
}

fn signed_input(
    participant: &KukuriKeys,
    sequence: u64,
    input: DomeSessionInputKindV1,
) -> SignedDomeSessionInputV1 {
    kukuri_core::build_signed_dome_session_input(
        participant,
        DomeSessionInputV1 {
            input_id: format!("input-{sequence}"),
            instance_id: "dome-1".into(),
            instance_generation: 1,
            lease_epoch: 1,
            session_id: "session-1".into(),
            participant_pubkey: participant.public_key(),
            sequence,
            sent_at: 1_000 + sequence as i64,
            input,
        },
    )
    .unwrap()
}

#[test]
fn keepalive_preserves_participant_and_timeout_evicts_all_session_state() {
    let (owner, lease, instance, preset) = fixture();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let participant = KukuriKeys::generate();
    runtime
        .apply_signed_input_at(
            &signed_input(
                &participant,
                1,
                DomeSessionInputKindV1::Join {
                    avatar_collider: None,
                },
            ),
            1_001,
        )
        .unwrap();
    runtime
        .apply_signed_input_at(
            &signed_input(&participant, 19_000, DomeSessionInputKindV1::KeepAlive),
            20_000,
        )
        .unwrap();

    runtime.advance_to(50_000).unwrap();
    assert_eq!(runtime.participant_count(), 1);
    runtime.advance_to(50_001).unwrap();
    assert_eq!(runtime.participant_count(), 0);
    assert!(
        runtime
            .signed_snapshot(50_001)
            .unwrap()
            .snapshot
            .bodies
            .iter()
            .all(|body| body.entity_id != format!("avatar:{}", participant.public_key().as_str()))
    );
}

#[test]
fn join_uses_default_spawn_then_deterministically_evacuates_from_overlap() {
    let (owner, lease, instance, mut preset) = fixture();
    preset.dome.customization.persistent_props = vec![MetaversePersistentPropV1 {
        prop_id: "spawn-blocker".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: instance.default_spawn.position,
        rotation: [0, 0, 0],
        scale: [100, 180, 100],
        visual_only: false,
        interactions: Vec::new(),
        collider: Some(MetaverseColliderV1::Cuboid {
            center: [0, 90, 0],
            half_extents: [50, 90, 50],
        }),
    }];
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let participant = KukuriKeys::generate();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    let snapshot = runtime.signed_snapshot(1_100).unwrap();
    let avatar = snapshot
        .snapshot
        .bodies
        .iter()
        .find(|body| body.entity_id == format!("avatar:{}", participant.public_key().as_str()))
        .expect("admitted avatar");
    assert_eq!(avatar.position, [150, 0, 0]);
}

#[test]
fn admission_snapshot_never_reuses_a_throttled_pre_join_snapshot() {
    let (owner, lease, instance, preset) = fixture();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let participant = KukuriKeys::generate();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    let first = runtime.signed_snapshot(1_001).unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            2,
            DomeSessionInputKindV1::Leave,
        ))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            3,
            DomeSessionInputKindV1::Join {
                avatar_collider: Some(MetaverseColliderV1::Capsule {
                    center: [1_950, 90, 0],
                    radius: 25,
                    half_height: 65,
                }),
            },
        ))
        .unwrap();

    let admission = runtime.signed_admission_snapshot(1_003).unwrap();
    assert!(admission.snapshot.sequence > first.snapshot.sequence);
    let avatar = admission
        .snapshot
        .bodies
        .iter()
        .find(|body| body.entity_id == format!("avatar:{}", participant.public_key().as_str()))
        .expect("fresh admission avatar");
    assert_ne!(avatar.position, instance.default_spawn.position);
}

#[test]
fn join_with_no_safe_spawn_is_atomic() {
    let (owner, lease, instance, mut preset) = fixture();
    preset.dome.customization.persistent_props = vec![MetaversePersistentPropV1 {
        prop_id: "spawn-area-blocker".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: instance.default_spawn.position,
        rotation: [0, 0, 0],
        scale: [500, 400, 500],
        visual_only: false,
        interactions: Vec::new(),
        collider: Some(MetaverseColliderV1::Cuboid {
            center: [0, 200, 0],
            half_extents: [1_000, 200, 1_000],
        }),
    }];
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let participant = KukuriKeys::generate();
    let error = runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .expect_err("blocked spawn must reject entry");
    assert!(error.to_string().contains("DOME_ENTRY_NO_SAFE_SPAWN"));
    assert_eq!(runtime.participant_count(), 0);
    assert!(
        !runtime
            .bodies_by_id
            .contains_key(&format!("avatar:{}", participant.public_key().as_str()))
    );
}

fn transition_request(
    participant: &KukuriKeys,
    transition_id: &str,
) -> DomeTransitionAdmissionRequestV1 {
    DomeTransitionAdmissionRequestV1 {
        transition_id: transition_id.into(),
        connection_id: "connection-1".into(),
        topology_digest: "topology-1".into(),
        spatial_context: SpatialContextV1::Topic {
            topic_id: TopicId("kukuri:topic:runtime".into()),
        },
        source_instance_id: "dome-0".into(),
        source_instance_generation: 1,
        target_instance_id: "dome-1".into(),
        target_instance_generation: 1,
        participant_pubkey: participant.public_key(),
        direction: DomeDirection::North,
        requested_at: 1_100,
    }
}

#[test]
fn transition_reservation_counts_capacity_and_commit_is_idempotent() {
    let (owner, lease, instance, preset) = fixture();
    let mut budget = kukuri_core::MetaverseResourceBudgetConfig::default();
    budget.host.max_participants = 1;
    let mut runtime = DomeSessionRuntime::start_with_budget(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
        budget,
    )
    .unwrap();
    let first = KukuriKeys::generate();
    let second = KukuriKeys::generate();
    let ticket = runtime
        .prepare_transition_admission(
            transition_request(&first, "transition-1"),
            DomeTransitionAccessDecisionV1::Allowed,
            1_100,
        )
        .unwrap();
    assert_eq!(runtime.transition_reservation_count(), 1);
    assert!(
        runtime
            .prepare_transition_admission(
                transition_request(&second, "transition-2"),
                DomeTransitionAccessDecisionV1::Allowed,
                1_100,
            )
            .unwrap_err()
            .to_string()
            .contains("DOME_TRANSITION_CAPACITY_FULL")
    );
    runtime
        .commit_transition_admission(&ticket, [0, 90, 2_840], [0, 0, 0], 1_200)
        .unwrap();
    let avatar_id = format!("avatar:{}", first.public_key().as_str());
    let avatar = runtime.rigid_bodies[runtime.bodies_by_id[&avatar_id].handle].translation();
    assert_eq!(
        meters_to_centimeters([avatar.x, avatar.y, avatar.z])[2],
        2_840
    );
    runtime
        .commit_transition_admission(&ticket, [0, 90, 2_840], [0, 0, 0], 30_000)
        .unwrap();
    assert_eq!(runtime.participant_count(), 1);
    assert_eq!(runtime.transition_reservation_count(), 0);
}

#[test]
fn transition_reservation_expires_and_access_denial_does_not_mutate_capacity() {
    let (owner, lease, instance, preset) = fixture();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let participant = KukuriKeys::generate();
    let denied = runtime.prepare_transition_admission(
        transition_request(&participant, "transition-denied"),
        DomeTransitionAccessDecisionV1::Denied {
            reason: kukuri_core::DomeTransitionDenialReasonV1::VisitorBlocked,
        },
        1_100,
    );
    assert!(denied.unwrap_err().to_string().contains("VISITOR_BLOCKED"));
    assert_eq!(runtime.transition_reservation_count(), 0);

    let ticket = runtime
        .prepare_transition_admission(
            transition_request(&participant, "transition-expired"),
            DomeTransitionAccessDecisionV1::Allowed,
            1_100,
        )
        .unwrap();
    assert!(
        runtime
            .commit_transition_admission(&ticket, [0, 90, 0], [0, 0, 0], ticket.expires_at)
            .is_err()
    );
    assert_eq!(runtime.transition_reservation_count(), 0);
    assert_eq!(runtime.participant_count(), 0);
}

#[test]
fn access_revocation_cancels_reservation_and_evicts_only_target_participant() {
    let (owner, lease, instance, preset) = fixture();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let blocked = KukuriKeys::generate();
    let allowed = KukuriKeys::generate();
    let blocked_ticket = runtime
        .prepare_transition_admission(
            transition_request(&blocked, "transition-blocked"),
            DomeTransitionAccessDecisionV1::Allowed,
            1_100,
        )
        .unwrap();
    assert_eq!(runtime.transition_reservation_count(), 1);
    assert_eq!(
        runtime.revoke_transition_access(&blocked.public_key(), Some("connection-1")),
        1
    );
    assert!(
        runtime
            .commit_transition_admission(&blocked_ticket, [0, 90, 0], [0, 0, 0], 1_200)
            .is_err()
    );

    runtime
        .apply_signed_input_at(
            &signed_input(
                &blocked,
                1,
                DomeSessionInputKindV1::Join {
                    avatar_collider: None,
                },
            ),
            1_101,
        )
        .unwrap();
    runtime
        .apply_signed_input_at(
            &signed_input(
                &allowed,
                1,
                DomeSessionInputKindV1::Join {
                    avatar_collider: None,
                },
            ),
            1_101,
        )
        .unwrap();
    assert_eq!(runtime.participant_count(), 2);
    assert!(runtime.evict_participant(&blocked.public_key()));
    assert_eq!(runtime.participant_count(), 1);
    assert!(!runtime.evict_participant(&blocked.public_key()));
}

#[test]
fn source_prepare_drops_grab_clears_seat_and_fences_interactions() {
    let (owner, lease, instance, mut preset) = fixture();
    preset.dome.customization.persistent_props = vec![MetaversePersistentPropV1 {
        prop_id: "seat-1".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: [0, 100, 0],
        rotation: [0, 0, 0],
        scale: [100, 100, 100],
        visual_only: false,
        interactions: Vec::new(),
        collider: None,
    }];
    let participant = KukuriKeys::generate();
    let participant_id = participant.public_key().as_str().to_string();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            2,
            DomeSessionInputKindV1::Grab {
                prop_id: "seat-1".into(),
            },
        ))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            3,
            DomeSessionInputKindV1::Sit {
                prop_id: "seat-1".into(),
            },
        ))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            4,
            DomeSessionInputKindV1::PrepareTransition {
                transition_id: "transition-1".into(),
                direction: DomeDirection::North,
            },
        ))
        .unwrap();
    assert_eq!(runtime.bodies_by_id["seat-1"].grabbed_by, None);
    assert!(!runtime.seated_on.contains_key(&participant_id));
    assert!(runtime.prepared_exits.contains_key(&participant_id));
    assert!(
        runtime
            .apply_signed_input(&signed_input(
                &participant,
                5,
                DomeSessionInputKindV1::Grab {
                    prop_id: "seat-1".into(),
                },
            ))
            .unwrap_err()
            .to_string()
            .contains("SOURCE_INPUT_FENCED")
    );
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            5,
            DomeSessionInputKindV1::CompleteTransition {
                transition_id: "transition-1".into(),
            },
        ))
        .unwrap();
    assert_eq!(runtime.participant_count(), 0);
}

#[test]
fn only_a_prepared_avatar_can_enter_the_connection_zone() {
    let (owner, lease, instance, preset) = fixture();
    let participant = KukuriKeys::generate();
    let participant_id = participant.public_key().as_str().to_string();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    runtime
        .add_guest_prop(GuestPropSpec {
            prop_id: "blocked-prop".into(),
            position: [0, 90, -3_000],
            expires_at: 10_000,
        })
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    let prop = runtime.rigid_bodies[runtime.bodies_by_id["blocked-prop"].handle].translation();
    assert!(prop.z > -20.0, "props must remain inside the hemisphere");

    runtime
        .apply_signed_input(&signed_input(
            &participant,
            2,
            DomeSessionInputKindV1::PrepareTransition {
                transition_id: "transition-corridor".into(),
                direction: DomeDirection::North,
            },
        ))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            3,
            DomeSessionInputKindV1::Move {
                position: [0, 90, -3_000],
                rotation: [0, 0, 0],
                animation: "walk".into(),
            },
        ))
        .unwrap();
    let avatar_id = format!("avatar:{participant_id}");
    let avatar = runtime.rigid_bodies[runtime.bodies_by_id[&avatar_id].handle].translation();
    assert_eq!(
        meters_to_centimeters([avatar.x, avatar.y, avatar.z])[2],
        -3_000
    );
}

#[test]
fn zero_participants_sleep_but_wall_clock_ttl_expires() {
    let (owner, lease, instance, preset) = fixture();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    runtime
        .add_guest_prop(GuestPropSpec {
            prop_id: "guest-1".into(),
            position: [0, 100, 0],
            expires_at: 2_000,
        })
        .unwrap();
    assert!(runtime.is_sleeping());
    let snapshot = runtime.signed_snapshot(2_100).unwrap();
    assert!(snapshot.snapshot.sleeping);
    assert!(
        snapshot
            .snapshot
            .bodies
            .iter()
            .all(|body| body.entity_id != "guest-1")
    );
}

#[test]
fn joined_participant_wakes_physics_and_stale_input_is_rejected() {
    let (owner, lease, instance, preset) = fixture();
    let participant = KukuriKeys::generate();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let join = signed_input(
        &participant,
        1,
        DomeSessionInputKindV1::Join {
            avatar_collider: None,
        },
    );
    runtime.apply_signed_input(&join).unwrap();
    assert!(!runtime.is_sleeping());
    assert!(runtime.apply_signed_input(&join).is_err());
    runtime.advance_to(1_100).unwrap();
    assert!(runtime.signed_snapshot(1_200).unwrap().snapshot.sequence > 0);
}

#[test]
fn heartbeat_sequence_is_monotonic_even_when_the_clock_does_not_advance() {
    let (owner, lease, instance, preset) = fixture();
    let runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();

    let first = runtime.signed_heartbeat(1_000).unwrap();
    let second = runtime.signed_heartbeat(1_000).unwrap();

    assert_eq!(first.heartbeat.sequence, 1);
    assert_eq!(second.heartbeat.sequence, 2);
    assert_eq!(second.heartbeat.sent_at, first.heartbeat.sent_at);
}

#[test]
fn restart_uses_manifest_initial_state_and_new_session() {
    let (owner, lease, instance, mut preset) = fixture();
    preset.dome.customization.persistent_props = vec![MetaversePersistentPropV1 {
        prop_id: "prop-1".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: [100, 200, 300],
        rotation: [0, 0, 0],
        scale: [100, 100, 100],
        visual_only: false,
        interactions: Vec::new(),
        collider: None,
    }];
    let restarted = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-after-restart",
        2_000,
    )
    .unwrap();
    assert_eq!(restarted.session_id(), "session-after-restart");
    assert_eq!(restarted.participant_count(), 0);
    let body = restarted.bodies_by_id.get("prop-1").unwrap();
    let translation = restarted.rigid_bodies[body.handle].translation();
    assert_eq!(
        meters_to_centimeters([translation.x, translation.y, translation.z]),
        [100, 200, 300]
    );
}

#[test]
fn snapshot_ring_is_bounded_and_resync_falls_back_to_latest() {
    let (owner, lease, instance, preset) = fixture();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    for index in 1..=DOME_SNAPSHOT_RING_CAPACITY + 5 {
        runtime.signed_snapshot(1_000 + index as i64 * 100).unwrap();
    }
    assert_eq!(runtime.snapshot_ring_len(), DOME_SNAPSHOT_RING_CAPACITY);
    let latest = runtime.snapshots_after(1);
    assert_eq!(latest.len(), 1);
    assert_eq!(
        latest[0].snapshot.sequence,
        (DOME_SNAPSHOT_RING_CAPACITY + 5) as u64
    );
}

#[test]
fn layout_candidate_contains_only_owner_managed_persistent_props() {
    let (owner, lease, instance, preset) = fixture();
    let participant = KukuriKeys::generate();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner.clone(),
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    let guest = MetaversePersistentPropV1 {
        prop_id: "guest-1".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Sphere,
        position: [0, 100, 0],
        rotation: [0, 0, 0],
        scale: [100, 100, 100],
        visual_only: false,
        interactions: Vec::new(),
        collider: None,
    };
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            2,
            DomeSessionInputKindV1::SpawnGuestProp {
                prop: guest,
                expires_at: 10_000,
            },
        ))
        .unwrap();
    let persistent = MetaversePersistentPropV1 {
        prop_id: "owner-prop".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: [100, 100, 100],
        rotation: [0, 0, 0],
        scale: [120, 120, 120],
        visual_only: false,
        interactions: Vec::new(),
        collider: None,
    };
    runtime
        .apply_signed_input(&signed_input(
            &owner,
            1,
            DomeSessionInputKindV1::UpsertPersistentProp { prop: persistent },
        ))
        .unwrap();
    let candidate = runtime.signed_layout_candidate("layout-1", 1_100).unwrap();
    assert!(
        candidate
            .candidate
            .persistent_props
            .iter()
            .any(|prop| prop.prop_id == "owner-prop")
    );
    assert!(
        candidate
            .candidate
            .persistent_props
            .iter()
            .all(|prop| prop.prop_id != "guest-1")
    );
    assert!(
        candidate
            .candidate
            .persistent_props
            .iter()
            .all(|prop| !prop.prop_id.starts_with("avatar:"))
    );
}

#[test]
fn non_owner_cannot_mutate_persistent_props() {
    let (owner, lease, instance, preset) = fixture();
    let participant = KukuriKeys::generate();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let prop = MetaversePersistentPropV1 {
        prop_id: "unauthorized".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: [0, 100, 0],
        rotation: [0, 0, 0],
        scale: [100, 100, 100],
        visual_only: false,
        interactions: Vec::new(),
        collider: None,
    };
    let input = signed_input(
        &participant,
        1,
        DomeSessionInputKindV1::UpsertPersistentProp { prop },
    );
    assert!(runtime.apply_signed_input(&input).is_err());
}

#[test]
fn participant_budget_is_enforced_before_join_mutates_state() {
    let (owner, lease, instance, preset) = fixture();
    let mut budget = kukuri_core::MetaverseResourceBudgetConfig::default();
    budget.host.max_participants = 1;
    let mut runtime = DomeSessionRuntime::start_with_budget(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
        budget,
    )
    .unwrap();
    let first = KukuriKeys::generate();
    let second = KukuriKeys::generate();
    runtime
        .apply_signed_input(&signed_input(
            &first,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    let error = runtime
        .apply_signed_input(&signed_input(
            &second,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("METAVERSE_HOST_PARTICIPANTS_LIMIT_EXCEEDED")
    );
    assert_eq!(runtime.participant_count(), 1);
}

#[test]
fn extreme_impulse_is_rejected_without_changing_sequence() {
    let (owner, lease, instance, preset) = fixture();
    let participant = KukuriKeys::generate();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    let excessive = signed_input(
        &participant,
        2,
        DomeSessionInputKindV1::Push {
            prop_id: format!("avatar:{}", participant.public_key().as_str()),
            impulse: [5_001, 0, 0],
        },
    );
    assert!(runtime.apply_signed_input(&excessive).is_err());
    assert_eq!(
        runtime.inputs.sequence(participant.public_key().as_str()),
        Some(1)
    );
}

#[test]
fn interaction_rate_is_enforced_at_the_boundary_without_partial_mutation() {
    let (owner, lease, instance, preset) = fixture();
    let participant = KukuriKeys::generate();
    let mut budget = kukuri_core::MetaverseResourceBudgetConfig::default();
    budget.player.max_interactions_per_second = 1;
    let mut runtime = DomeSessionRuntime::start_with_budget(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
        budget,
    )
    .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            1,
            DomeSessionInputKindV1::Join {
                avatar_collider: None,
            },
        ))
        .unwrap();
    let avatar_id = format!("avatar:{}", participant.public_key().as_str());
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            2,
            DomeSessionInputKindV1::Push {
                prop_id: avatar_id.clone(),
                impulse: [1, 0, 0],
            },
        ))
        .unwrap();
    let error = runtime
        .apply_signed_input(&signed_input(
            &participant,
            3,
            DomeSessionInputKindV1::Push {
                prop_id: avatar_id,
                impulse: [1, 0, 0],
            },
        ))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("METAVERSE_PLAYER_INTERACTION_RATE_RATE_EXCEEDED")
    );
    assert_eq!(
        runtime.inputs.sequence(participant.public_key().as_str()),
        Some(2)
    );
}

#[test]
fn snapshot_frequency_is_capped_without_stopping_session() {
    let (owner, lease, instance, preset) = fixture();
    let mut runtime = DomeSessionRuntime::start_with_session_id(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
    )
    .unwrap();
    let first = runtime.signed_snapshot(1_000).unwrap();
    let throttled = runtime.signed_snapshot(1_001).unwrap();
    assert_eq!(first.snapshot.sequence, throttled.snapshot.sequence);
    let next = runtime.signed_snapshot(1_100).unwrap();
    assert!(next.snapshot.sequence > first.snapshot.sequence);
}

#[test]
fn first_snapshot_over_bandwidth_is_rejected_without_advancing_sequence() {
    let (owner, lease, instance, preset) = fixture();
    let mut budget = kukuri_core::MetaverseResourceBudgetConfig::default();
    budget.host.max_snapshot_bytes_per_second = 1;
    let mut runtime = DomeSessionRuntime::start_with_budget(
        lease,
        owner,
        &instance,
        &preset,
        "session-1",
        1_000,
        budget,
    )
    .unwrap();

    let error = runtime.signed_snapshot(1_000).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("METAVERSE_HOST_SNAPSHOT_BANDWIDTH_LIMIT_EXCEEDED")
    );
    assert_eq!(runtime.snapshot_sequence, 0);
    assert_eq!(runtime.resource_metrics().rejected_total, 1);
}
