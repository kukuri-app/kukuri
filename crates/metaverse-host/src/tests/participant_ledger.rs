//! 参加者ごとの台帳が退出・拒否の後に残らず、件数が参加者の上限で抑えられること。
//! 退出した署名者の古い署名済み input の再送を、同じ session の間は拒否し続けること。

use super::*;

/// その署名者の行を持つ、参加者ごとの台帳の名前。
fn ledger_rows(runtime: &DomeSessionRuntime, participant: &KukuriKeys) -> Vec<&'static str> {
    let pubkey = participant.public_key();
    let id = pubkey.as_str();
    let owns =
        |ticket: &DomeTransitionAdmissionTicketV1| ticket.request.participant_pubkey == pubkey;
    [
        ("participants", runtime.participants.contains(id)),
        ("player_budgets", runtime.player_budgets.contains_key(id)),
        ("prepared_exits", runtime.prepared_exits.contains_key(id)),
        (
            "transition_entries",
            runtime.transition_entries.contains_key(id),
        ),
        ("seated_on", runtime.seated_on.contains_key(id)),
        (
            "last_seen_at",
            runtime.participant_last_seen_at.contains_key(id),
        ),
        (
            "reservations",
            runtime.transition_reservations.values().any(owns),
        ),
        (
            "committed",
            runtime.committed_transitions.values().any(owns),
        ),
        ("sequence", runtime.inputs.sequence(id).is_some()),
    ]
    .into_iter()
    .filter_map(|(name, present)| present.then_some(name))
    .collect()
}

fn runtime_with_participant_limit(limit: u32) -> (KukuriKeys, DomeSessionRuntime) {
    let (owner, lease, instance, preset) = fixture();
    let mut budget = MetaverseResourceBudgetConfig::default();
    budget.host.max_participants = limit;
    let runtime = DomeSessionRuntime::start_with_budget(
        lease,
        owner.clone(),
        &instance,
        &preset,
        "session-1",
        1_000,
        budget,
    )
    .unwrap();
    (owner, runtime)
}

fn join(participant: &KukuriKeys, sequence: u64) -> SignedDomeSessionInputV1 {
    signed_input(
        participant,
        sequence,
        DomeSessionInputKindV1::Join {
            avatar_collider: None,
        },
    )
}

/// 署名時刻だけを変えて署名し直す。
fn resigned_at(
    participant: &KukuriKeys,
    signed: SignedDomeSessionInputV1,
    sent_at: i64,
) -> SignedDomeSessionInputV1 {
    let input = DomeSessionInputV1 {
        sent_at,
        ..signed.input
    };
    kukuri_core::build_signed_dome_session_input(participant, input).unwrap()
}

#[test]
fn leave_keeps_only_the_replay_mark_until_the_last_input_expires() {
    let (owner, mut runtime) = runtime_with_participant_limit(8);
    let participant = KukuriKeys::generate();
    let seat = format!("avatar:{}", participant.public_key().as_str());
    runtime.apply_signed_input(&join(&participant, 1)).unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            2,
            DomeSessionInputKindV1::Sit { prop_id: seat },
        ))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            3,
            DomeSessionInputKindV1::Leave,
        ))
        .unwrap();
    assert_eq!(ledger_rows(&runtime, &participant), ["sequence"]);
    // 最後の input（署名 1_003）から 10 秒の間は、古い Join の再送を sequence で拒否する。
    assert!(
        runtime
            .apply_signed_input_at(&join(&participant, 1), 5_000)
            .is_err()
    );

    // 10 秒を過ぎた後の input で記録を消す。
    runtime
        .apply_signed_input_at(&resigned_at(&owner, join(&owner, 1), 11_003), 11_003)
        .unwrap();
    assert!(ledger_rows(&runtime, &participant).is_empty());
}

#[test]
fn completing_a_transition_exit_forgets_the_participant_and_allows_a_later_join() {
    let (_owner, mut runtime) = runtime_with_participant_limit(8);
    let participant = KukuriKeys::generate();
    let complete = || DomeSessionInputKindV1::CompleteTransition {
        transition_id: "exit-1".into(),
    };
    runtime.apply_signed_input(&join(&participant, 1)).unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &participant,
            2,
            DomeSessionInputKindV1::PrepareTransition {
                transition_id: "exit-1".into(),
                direction: DomeDirection::North,
            },
        ))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(&participant, 3, complete()))
        .unwrap();
    assert_eq!(ledger_rows(&runtime, &participant), ["sequence"]);

    // 応答を失った完了の再送は、退出済みでも成功する。
    runtime
        .apply_signed_input(&signed_input(&participant, 4, complete()))
        .unwrap();
    // 遷移で出た Dome へ、後から Join で入り直せる。
    runtime.apply_signed_input(&join(&participant, 5)).unwrap();
    assert_eq!(runtime.participant_count(), 1);
}

#[test]
fn rejected_joins_and_inputs_from_non_participants_leave_no_rows() {
    let (_owner, mut runtime) = runtime_with_participant_limit(1);
    let first = KukuriKeys::generate();
    let second = KukuriKeys::generate();
    let stranger = KukuriKeys::generate();
    runtime.apply_signed_input(&join(&first, 1)).unwrap();
    assert!(runtime.apply_signed_input(&join(&second, 1)).is_err());
    let walk = DomeSessionInputKindV1::Move {
        position: [0, 0, 0],
        rotation: [0, 0, 0],
        animation: "walk".into(),
    };
    assert!(
        runtime
            .apply_signed_input(&signed_input(&second, 2, walk))
            .is_err()
    );
    runtime
        .apply_signed_input(&signed_input(&stranger, 1, DomeSessionInputKindV1::Leave))
        .unwrap();
    runtime
        .apply_signed_input(&signed_input(
            &stranger,
            2,
            DomeSessionInputKindV1::AbortTransition {
                transition_id: "none".into(),
            },
        ))
        .unwrap();

    assert!(ledger_rows(&runtime, &second).is_empty());
    assert!(ledger_rows(&runtime, &stranger).is_empty());
}

#[test]
fn departed_replay_marks_are_capped_at_the_participant_limit() {
    let (_owner, mut runtime) = runtime_with_participant_limit(2);
    let visitors = (0..5).map(|_| KukuriKeys::generate()).collect::<Vec<_>>();
    for visitor in &visitors {
        runtime.apply_signed_input(&join(visitor, 1)).unwrap();
        runtime
            .apply_signed_input(&signed_input(visitor, 2, DomeSessionInputKindV1::Leave))
            .unwrap();
    }

    // 上限を超えた分は退出の古い順に捨て、最後の 2 人の記録だけを残す。
    let rows = visitors
        .iter()
        .map(|visitor| ledger_rows(&runtime, visitor))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [vec![], vec![], vec![], vec!["sequence"], vec!["sequence"]]
    );
    assert!(runtime.player_budgets.is_empty());
}

#[test]
fn transition_arrivals_keep_one_committed_ticket_and_a_first_leave_is_not_replayable() {
    let (_owner, mut runtime) = runtime_with_participant_limit(8);
    let participant = KukuriKeys::generate();
    let arrive = |runtime: &mut DomeSessionRuntime, transition_id: &str, now: i64| {
        let ticket = runtime
            .prepare_transition_admission(
                transition_request(&participant, transition_id),
                DomeTransitionAccessDecisionV1::Allowed,
                now,
            )
            .unwrap();
        runtime
            .commit_transition_admission(&ticket, [0, 90, 0], [0, 0, 0], now + 100)
            .unwrap();
    };
    arrive(&mut runtime, "arrival-1", 1_100);
    arrive(&mut runtime, "arrival-2", 1_100);
    assert_eq!(runtime.committed_transitions.len(), 1);

    // 到着の後の最初の input が Leave でも、その sequence を残して再送を拒否する。
    let leave = signed_input(&participant, 1, DomeSessionInputKindV1::Leave);
    runtime.apply_signed_input_at(&leave, 1_300).unwrap();
    assert_eq!(ledger_rows(&runtime, &participant), ["sequence"]);
    arrive(&mut runtime, "arrival-3", 1_400);
    assert!(runtime.apply_signed_input_at(&leave, 1_600).is_err());
    assert_eq!(runtime.participant_count(), 1);
}

#[test]
fn old_inputs_stay_rejected_after_eviction_and_after_their_ttl() {
    let (owner, mut runtime) = runtime_with_participant_limit(8);
    let participant = KukuriKeys::generate();
    runtime.apply_signed_input(&join(&participant, 1)).unwrap();
    assert!(runtime.evict_participant(&participant.public_key()));
    // access 失効などで退去した直後の、古い Join の再送を拒否する。
    assert!(
        runtime
            .apply_signed_input_at(&join(&participant, 1), 2_000)
            .is_err()
    );

    // 署名から 10 秒を過ぎた input は、記録が消えた後も拒否する。
    let expired = runtime
        .apply_signed_input_at(&join(&participant, 1), 11_001)
        .unwrap_err();
    assert!(expired.to_string().contains("DOME_SESSION_INPUT_EXPIRED"));
    // 新しく署名した Join は、sequence を振り直していても受け付ける。
    let rejoin = resigned_at(&participant, join(&participant, 1), 11_001);
    runtime.apply_signed_input_at(&rejoin, 11_001).unwrap();
    assert_eq!(runtime.participant_count(), 1);

    // 参加していない所有者の prop の変更も、再送を拒否する。
    let prop = MetaversePersistentPropV1 {
        prop_id: "owner-prop".into(),
        asset_ref: None,
        primitive_fallback: MetaversePrimitive::Cube,
        position: [0, 100, 0],
        rotation: [0, 0, 0],
        scale: [100, 100, 100],
        visual_only: false,
        interactions: Vec::new(),
        collider: None,
    };
    let upsert = DomeSessionInputKindV1::UpsertPersistentProp { prop };
    let upsert = resigned_at(&owner, signed_input(&owner, 1, upsert), 11_002);
    runtime.apply_signed_input_at(&upsert, 11_002).unwrap();
    let delete = DomeSessionInputKindV1::DeletePersistentProp {
        prop_id: "owner-prop".into(),
    };
    let delete = resigned_at(&owner, signed_input(&owner, 2, delete), 11_003);
    runtime.apply_signed_input_at(&delete, 11_003).unwrap();
    assert!(runtime.apply_signed_input_at(&upsert, 11_004).is_err());
}

#[test]
fn inputs_signed_ten_seconds_before_the_host_clock_are_rejected() {
    let (_owner, mut runtime) = runtime_with_participant_limit(8);
    let participant = KukuriKeys::generate();
    runtime.apply_signed_input(&join(&participant, 1)).unwrap();
    let late = runtime
        .apply_signed_input_at(
            &signed_input(&participant, 2, DomeSessionInputKindV1::KeepAlive),
            11_002,
        )
        .unwrap_err();
    assert!(late.to_string().contains("DOME_SESSION_INPUT_EXPIRED"));
    runtime
        .apply_signed_input_at(
            &signed_input(&participant, 3, DomeSessionInputKindV1::KeepAlive),
            11_002,
        )
        .unwrap();
}
