//! 共有 replica の key 種別表（#1065）の contract。

use crate::keys::SharedReplicaKeyFamily;
use crate::replicas::stable_key;

#[test]
fn prefixes_are_pairwise_disjoint() {
    for a in SharedReplicaKeyFamily::ALL {
        for b in SharedReplicaKeyFamily::ALL {
            if a != b {
                assert!(
                    !a.prefix().starts_with(b.prefix()),
                    "{a:?} prefix overlaps {b:?}"
                );
            }
        }
    }
}

#[test]
fn parse_resolves_client_keys_and_rejects_unknown() {
    let cases = [
        (
            stable_key("objects", "o1/state"),
            SharedReplicaKeyFamily::PostObject,
            "o1/state",
        ),
        (
            stable_key("withdrawals", "o1/state"),
            SharedReplicaKeyFamily::PostWithdrawal,
            "o1/state",
        ),
        (
            stable_key("link-previews", "o1/state"),
            SharedReplicaKeyFamily::LinkPreview,
            "o1/state",
        ),
        (
            stable_key("manifests/media", "m1/envelope"),
            SharedReplicaKeyFamily::MediaManifest,
            "m1/envelope",
        ),
        (
            stable_key("indexes/timeline", "s/o1"),
            SharedReplicaKeyFamily::TimelineIndex,
            "s/o1",
        ),
        (
            stable_key("indexes/thread", "r/s/o1"),
            SharedReplicaKeyFamily::ThreadIndex,
            "r/s/o1",
        ),
        (
            stable_key("reactions", "o1/r1/state"),
            SharedReplicaKeyFamily::Reaction,
            "o1/r1/state",
        ),
        (
            stable_key("envelopes", "e1"),
            SharedReplicaKeyFamily::Envelope,
            "e1",
        ),
        (
            stable_key("sessions/live", "s1/state"),
            SharedReplicaKeyFamily::Session,
            "live/s1/state",
        ),
        (
            stable_key("channels", "policy/envelope"),
            SharedReplicaKeyFamily::Channel,
            "policy/envelope",
        ),
        (
            stable_key("metaverse/dome-hosting", "i1/x"),
            SharedReplicaKeyFamily::Metaverse,
            "dome-hosting/i1/x",
        ),
    ];
    for (key, family, rest) in &cases {
        assert_eq!(
            SharedReplicaKeyFamily::parse(key),
            Some((*family, *rest)),
            "{key}"
        );
    }
    for unknown in [
        "",
        "objects",
        "indexes/other/x",
        "manifests/other/x",
        "unknown/x",
    ] {
        assert_eq!(SharedReplicaKeyFamily::parse(unknown), None, "{unknown}");
    }
}
