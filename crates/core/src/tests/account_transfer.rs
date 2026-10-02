//! #1211 AC-1（1a・1f）: 招待の wire の形式・上限・期限と、秘密の非混入。

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;

use crate::{
    ACCOUNT_TRANSFER_INVITE_TTL_MS, ACCOUNT_TRANSFER_LINK_PREFIX, AccountTransferInvite,
    MAX_ACCOUNT_TRANSFER_LINK_BYTES,
};

const NOW: i64 = 1_790_000_000_000;
const ISSUER: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn invite() -> AccountTransferInvite {
    AccountTransferInvite::issue(
        ISSUER.to_string(),
        Some("https://relay.example.test./".to_string()),
        (0..6).map(|port| format!("127.0.0.1:{}", 4000 + port).parse().unwrap()),
        NOW,
    )
    .unwrap()
}

fn json_of(link: &str) -> serde_json::Value {
    let payload = link.strip_prefix(ACCOUNT_TRANSFER_LINK_PREFIX).unwrap();
    serde_json::from_slice(&BASE64_URL.decode(payload).unwrap()).unwrap()
}

fn link_of(json: &serde_json::Value) -> String {
    format!(
        "{ACCOUNT_TRANSFER_LINK_PREFIX}{}",
        BASE64_URL.encode(serde_json::to_vec(json).unwrap())
    )
}

#[test]
fn the_link_round_trips_within_its_limits() {
    let invite = invite();
    assert_eq!(ACCOUNT_TRANSFER_LINK_PREFIX, "kukuri://transfer#v1.");
    assert_eq!(invite.direct_addrs.len(), 4, "addrs are capped");
    assert_eq!(invite.expires_at_ms, NOW + ACCOUNT_TRANSFER_INVITE_TTL_MS);
    let link = invite.to_link();
    assert!(
        link.len() <= MAX_ACCOUNT_TRANSFER_LINK_BYTES,
        "{}",
        link.len()
    );
    assert_eq!(
        AccountTransferInvite::parse_link(&link, NOW).unwrap(),
        invite
    );
    // 2 回作れば秘密は別になる。
    assert_ne!(invite.to_link(), self::invite().to_link());
}

#[test]
fn malformed_oversized_and_expired_links_are_rejected() {
    let link = invite().to_link();
    let expired = AccountTransferInvite::parse_link(&link, NOW + ACCOUNT_TRANSFER_INVITE_TTL_MS);
    assert!(expired.unwrap_err().to_string().contains("expired"));

    let mut unknown = json_of(&link);
    unknown["extra"] = serde_json::json!(1);
    let mut bad_issuer = json_of(&link);
    bad_issuer["issuer"] = serde_json::json!("zz");
    let mut too_many = json_of(&link);
    too_many["direct_addrs"] = serde_json::json!(vec!["127.0.0.1:1"; 5]);
    let mut short_secret = json_of(&link);
    short_secret["secret"] = serde_json::json!("00");
    for bad in [
        String::new(),
        link.replacen("kukuri://transfer#", "kukuri://transfer?", 1),
        link.replacen("#v1.", "#v2.", 1),
        format!("https://example.test/{link}"),
        format!("{link}!"),
        format!("{link}{}", "A".repeat(MAX_ACCOUNT_TRANSFER_LINK_BYTES)),
        link_of(&unknown),
        link_of(&bad_issuer),
        link_of(&too_many),
        link_of(&short_secret),
    ] {
        assert!(
            AccountTransferInvite::parse_link(&bad, NOW).is_err(),
            "accepted {bad}"
        );
    }
}

#[test]
fn the_proof_and_code_bind_both_endpoints_and_the_secret() {
    let invite = invite();
    let target = [2; 32];
    let other_target = [3; 32];
    let proof = invite.hello_proof(&target);
    assert!(invite.verify_hello(&target, &proof));
    assert!(!invite.verify_hello(&other_target, &proof));

    let mut other_issuer = json_of(&invite.to_link());
    other_issuer["issuer"] = serde_json::json!("2".repeat(64));
    let other_issuer = AccountTransferInvite::parse_link(&link_of(&other_issuer), NOW).unwrap();
    assert!(!other_issuer.verify_hello(&target, &proof));
    assert!(
        !self::invite().verify_hello(&target, &proof),
        "another secret"
    );

    let code = invite.verification_code(&target);
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(code, invite.verification_code(&target));
    assert_ne!(code, invite.verification_code(&other_target));
}

#[test]
fn debug_output_does_not_contain_the_secret() {
    let invite = invite();
    let secret = json_of(&invite.to_link())["secret"]
        .as_str()
        .unwrap()
        .to_string();
    let debug = format!("{invite:?}");
    assert!(!debug.contains(&secret), "{debug}");
    assert!(!debug.contains("direct_addrs"), "{debug}");
}
