use crate::*;

#[test]
fn host_liveness_uses_five_second_offline_and_fifteen_second_close_boundaries() {
    assert_eq!(
        resolve_dome_host_liveness(Some(1_000), 6_000),
        DomeHostLivenessV1::Online
    );
    assert_eq!(
        resolve_dome_host_liveness(Some(1_000), 6_001),
        DomeHostLivenessV1::OfflineGrace
    );
    assert_eq!(
        resolve_dome_host_liveness(Some(1_000), 16_000),
        DomeHostLivenessV1::OfflineGrace
    );
    assert_eq!(
        resolve_dome_host_liveness(Some(1_000), 16_001),
        DomeHostLivenessV1::Closed
    );
    assert_eq!(
        resolve_dome_host_liveness(None, 1_000),
        DomeHostLivenessV1::OfflineGrace
    );
}

// 所有者の端末が送る間隔どおりに送る heartbeat が遅れて届いても、参加者は動いている host を offline と読まない
// (#1803。送る間隔が判定の閾値と同じだと、遅れの分だけ毎周期 offline になっていた)。
#[test]
fn a_host_sending_on_schedule_stays_online_for_a_late_receiver() {
    for delay in [0, 100, 1_000, 2_000] {
        for now in (delay..60_000).step_by(10) {
            // 間隔ごとに署名した heartbeat のうち、`delay` 遅れて届いた最後のもの。
            let received = (now - delay) / DOME_HOST_HEARTBEAT_SEND_INTERVAL_MILLIS
                * DOME_HOST_HEARTBEAT_SEND_INTERVAL_MILLIS;
            assert_eq!(
                resolve_dome_host_liveness(Some(received), now),
                DomeHostLivenessV1::Online,
                "delay {delay} ms, {now} ms"
            );
        }
    }
}

#[test]
fn evacuation_candidates_are_safe_ranked_stable_and_exclude_current() {
    let candidates = vec![
        DomeEvacuationCandidateV1 {
            instance_id: "current".into(),
            kind: DomeEvacuationCandidateKindV1::OwnHosted,
            available: true,
        },
        DomeEvacuationCandidateV1 {
            instance_id: "fallback-b".into(),
            kind: DomeEvacuationCandidateKindV1::StableFallback,
            available: true,
        },
        DomeEvacuationCandidateV1 {
            instance_id: "adjacent".into(),
            kind: DomeEvacuationCandidateKindV1::ActiveAdjacent,
            available: true,
        },
        DomeEvacuationCandidateV1 {
            instance_id: "adjacent".into(),
            kind: DomeEvacuationCandidateKindV1::LastVisited,
            available: true,
        },
        DomeEvacuationCandidateV1 {
            instance_id: "unavailable".into(),
            kind: DomeEvacuationCandidateKindV1::EstablishedTransition,
            available: false,
        },
        DomeEvacuationCandidateV1 {
            instance_id: "home".into(),
            kind: DomeEvacuationCandidateKindV1::OwnHosted,
            available: true,
        },
        DomeEvacuationCandidateV1 {
            instance_id: "fallback-a".into(),
            kind: DomeEvacuationCandidateKindV1::StableFallback,
            available: true,
        },
    ];
    let ids = order_dome_evacuation_candidates("current", &candidates)
        .into_iter()
        .map(|candidate| candidate.instance_id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["adjacent", "home", "fallback-a", "fallback-b"]);
}

#[test]
fn recovery_reason_codes_are_stable() {
    assert_eq!(
        DomeEvacuationReasonV1::HostOffline.code(),
        "DOME_EVACUATION_HOST_OFFLINE"
    );
    assert_eq!(
        DomeEvacuationReasonV1::AccessRevoked.code(),
        "DOME_EVACUATION_ACCESS_REVOKED"
    );
    assert_eq!(
        DomeEvacuationReasonV1::Blocked.code(),
        "DOME_EVACUATION_BLOCKED"
    );
    assert_eq!(
        DomeEvacuationReasonV1::TopologyInvalid.code(),
        "DOME_EVACUATION_TOPOLOGY_INVALID"
    );
    assert_eq!(
        DomeEvacuationReasonV1::UserRequested.code(),
        "DOME_EVACUATION_USER_REQUESTED"
    );
}
