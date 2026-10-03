//! #1211 AC-1（1a・1f）: 招待の wire の形式・上限・期限と、秘密の非混入。AC-2（2a）: 必須 bundle の frame。
//! AC-3（3a）: 履歴の frame。

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;

use crate::{
    ACCOUNT_TRANSFER_INVITE_TTL_MS, ACCOUNT_TRANSFER_LINK_PREFIX, AccountHistoryCursor,
    AccountHistoryRecord, AccountSyncItem, AccountSyncItemKey, AccountTransferFrame,
    AccountTransferHistory, AccountTransferInvite, AccountTransferItem, ChannelId, KukuriKeys,
    MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES, MAX_ACCOUNT_SYNC_ITEM_BYTES,
    MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS, MAX_ACCOUNT_TRANSFER_FRAME_BYTES,
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

// #1211 AC-2（2a）: 確認済みの接続で送る必須 bundle の frame の上限と、item の検証。

const SECRET: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const OTHER_SECRET: &str = "101112131415161718191a1b1c1d1e1f000102030405060708090a0b0c0d0e0f";

fn bundle_item(
    keys: &KukuriKeys,
    key: AccountSyncItemKey,
    value_bytes: usize,
) -> AccountTransferItem {
    let item = AccountSyncItem {
        key,
        op_id: "0123456789abcdef0123456789abcdef".to_string(),
        updated_at: NOW,
        value: Some(serde_json::Value::String("x".repeat(value_bytes))),
    };
    AccountTransferItem {
        key: item.key.docs_key(),
        sealed: keys
            .derive_account_sync()
            .seal(&keys.public_key(), &item)
            .unwrap(),
    }
}

fn membership(index: usize) -> AccountSyncItemKey {
    AccountSyncItemKey::ChannelMembership {
        channel_id: ChannelId(format!("channel-{index}")),
    }
}

#[test]
fn bundle_frames_stay_within_the_item_and_byte_limits() {
    let keys = KukuriKeys::parse(SECRET).unwrap();
    let small = (0..130)
        .map(|i| bundle_item(&keys, membership(i), 8))
        .collect();
    let frames = AccountTransferFrame::chunks(small).unwrap();
    let sizes: Vec<_> = frames
        .iter()
        .map(|frame| match frame {
            AccountTransferFrame::Items { items } => items.len(),
            _ => panic!("not an items frame"),
        })
        .collect();
    assert_eq!(sizes, [64, 64, 2]);

    // 1 件が 16 KiB 近い item は、64 件より前に 1 MiB で分かれる。
    let large: Vec<_> = (0..70)
        .map(|i| bundle_item(&keys, membership(i), MAX_ACCOUNT_SYNC_ITEM_BYTES - 200))
        .collect();
    let total = large.len();
    let frames = AccountTransferFrame::chunks(large).unwrap();
    assert!(frames.len() > 2, "{}", frames.len());
    let mut count = 0;
    for frame in &frames {
        let bytes = frame.encode().unwrap();
        assert!(bytes.len() <= MAX_ACCOUNT_TRANSFER_FRAME_BYTES);
        assert_eq!(&AccountTransferFrame::decode(&bytes).unwrap(), frame);
        if let AccountTransferFrame::Items { items } = frame {
            assert!(items.len() < MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS);
            count += items.len();
        }
    }
    assert_eq!(count, total);
}

#[test]
fn malformed_and_oversized_bundle_frames_are_rejected() {
    let keys = KukuriKeys::parse(SECRET).unwrap();
    let item = serde_json::to_value(bundle_item(&keys, membership(0), 8)).unwrap();
    let items = |n: usize| serde_json::json!({ "frame": "items", "items": vec![item.clone(); n] });
    for bad in [
        items(0),
        items(MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS + 1),
        serde_json::json!({ "frame": "end", "count": 1, "extra": 1 }),
        serde_json::json!({ "frame": "rewind" }),
    ] {
        let bytes = serde_json::to_vec(&bad).unwrap();
        assert!(
            AccountTransferFrame::decode(&bytes).is_err(),
            "accepted {bad}"
        );
    }
    let oversized = vec![b' '; MAX_ACCOUNT_TRANSFER_FRAME_BYTES + 1];
    assert!(AccountTransferFrame::decode(&oversized).is_err());
    assert!(
        AccountTransferFrame::decode(
            &serde_json::to_vec(&items(MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS)).unwrap()
        )
        .is_ok()
    );
}

#[test]
fn bundle_items_open_only_for_the_account_and_a_transferred_kind() {
    let keys = KukuriKeys::parse(SECRET).unwrap();
    let derived = keys.derive_account_sync();
    let account = keys.public_key();
    let item = bundle_item(&keys, membership(1), 8);
    assert_eq!(item.open(&derived, &account).unwrap().key, membership(1));

    // 別のアカウントの鍵・公開鍵、置き換えた docs の key では開けない。
    let other = KukuriKeys::parse(OTHER_SECRET).unwrap();
    assert!(item.open(&other.derive_account_sync(), &account).is_err());
    assert!(item.open(&derived, &other.public_key()).is_err());
    let moved = AccountTransferItem {
        key: membership(2).docs_key(),
        sealed: item.sealed.clone(),
    };
    assert!(moved.open(&derived, &account).is_err());

    // 変更の窓は移行の bundle に入れない（封が正しくても拒否する）。
    let slot = bundle_item(&keys, AccountSyncItemKey::change_slot("device-a", 3), 8);
    assert!(slot.open(&derived, &account).is_err());
}

#[test]
fn the_key_frame_does_not_debug_the_secret() {
    let frame = AccountTransferFrame::Key {
        secret: SECRET.to_string(),
    };
    let debug = format!("{frame:?}");
    assert!(!debug.contains(SECRET), "{debug}");
}

// #1211 AC-3（3a）: 履歴の stream の frame の上限と形。

fn history_record(index: usize, value_bytes: usize) -> AccountHistoryRecord {
    AccountHistoryRecord {
        replica: format!("bucket::v1::topic::74::{}", 20_000 + index),
        key: format!("objects/{index:064x}/envelope"),
        docs_author: "a".repeat(64),
        value: vec![b'v'; value_bytes],
    }
}

#[test]
fn history_frames_stay_within_the_record_and_byte_limits() {
    let frames =
        AccountTransferFrame::record_chunks((0..130).map(|i| history_record(i, 8)).collect())
            .unwrap();
    let sizes: Vec<_> = frames
        .iter()
        .map(|frame| match frame {
            AccountTransferFrame::Records { records } => records.len(),
            _ => panic!("not a records frame"),
        })
        .collect();
    assert_eq!(sizes, [64, 64, 2]);

    // 1 件が 64 KiB の record は、64 件より前に 1 MiB で分かれ、値はそのまま戻る。
    let large: Vec<_> = (0..20).map(|i| history_record(i, 64 * 1024)).collect();
    let frames = AccountTransferFrame::record_chunks(large.clone()).unwrap();
    assert!(frames.len() > 1, "{}", frames.len());
    let mut decoded = Vec::new();
    for frame in &frames {
        let bytes = frame.encode().unwrap();
        assert!(bytes.len() <= MAX_ACCOUNT_TRANSFER_FRAME_BYTES);
        match AccountTransferFrame::decode(&bytes).unwrap() {
            AccountTransferFrame::Records { records } => decoded.extend(records),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(decoded, large);

    // 範囲・続きの位置・page の終わりと、512 KiB の blob の一部は 1 frame に収まって戻る。
    let cursor = AccountHistoryCursor {
        reference: "own_docs".into(),
        replica: "bucket::v1::topic::74::20000".into(),
        key: "objects/x/envelope".into(),
        author: "a".repeat(64),
    };
    for frame in [
        AccountTransferFrame::History {
            since: Some(20_000),
            cursor: Some(cursor.clone()),
        },
        AccountTransferFrame::History {
            since: None,
            cursor: None,
        },
        AccountTransferFrame::Page {
            next: Some(cursor),
            posts: 3,
            unavailable: 2,
        },
        AccountTransferFrame::Blob {
            hash: "b".repeat(64),
            len: 2 * MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES as u64,
            offset: MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES as u64,
            bytes: vec![7; MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES],
        },
    ] {
        let bytes = frame.encode().unwrap();
        assert!(bytes.len() <= MAX_ACCOUNT_TRANSFER_FRAME_BYTES);
        assert_eq!(AccountTransferFrame::decode(&bytes).unwrap(), frame);
    }
}

#[test]
fn malformed_history_frames_are_rejected() {
    let record = serde_json::to_value(history_record(0, 8)).unwrap();
    let records =
        |n: usize| serde_json::json!({ "frame": "records", "records": vec![record.clone(); n] });
    let blob = |len: u64, offset: u64, bytes: usize| {
        serde_json::json!({
            "frame": "blob",
            "hash": "b".repeat(64),
            "len": len,
            "offset": offset,
            "bytes": base64::engine::general_purpose::STANDARD.encode(vec![1u8; bytes]),
        })
    };
    let mut unknown_field = record.clone();
    unknown_field["content_hash"] = serde_json::json!("x");
    for bad in [
        records(0),
        records(MAX_ACCOUNT_TRANSFER_CHUNK_ITEMS + 1),
        serde_json::json!({ "frame": "records", "records": [unknown_field] }),
        blob(10, 0, 0),
        blob(10, 5, 6),
        blob(u64::MAX, u64::MAX, 1),
        blob(u64::MAX, 0, MAX_ACCOUNT_HISTORY_BLOB_PART_BYTES + 1),
        serde_json::json!({ "frame": "page", "next": null, "posts": 0 }),
        serde_json::json!({ "frame": "history", "since": "1" }),
    ] {
        let bytes = serde_json::to_vec(&bad).unwrap();
        assert!(
            AccountTransferFrame::decode(&bytes).is_err(),
            "accepted {bad}"
        );
    }
    assert!(AccountTransferFrame::decode(&serde_json::to_vec(&blob(10, 5, 5)).unwrap()).is_ok());
}

#[test]
fn history_frames_do_not_debug_the_content() {
    let record = history_record(0, 8);
    let value = String::from_utf8(record.value.clone()).unwrap();
    for debug in [
        format!("{record:?}"),
        format!(
            "{:?}",
            AccountTransferFrame::Records {
                records: vec![record.clone()]
            }
        ),
        format!(
            "{:?}",
            AccountTransferFrame::Blob {
                hash: "b".repeat(64),
                len: 8,
                offset: 0,
                bytes: record.value.clone(),
            }
        ),
    ] {
        assert!(!debug.contains(&value), "{debug}");
    }
}

#[test]
fn the_history_ranges_have_fixed_days() {
    assert_eq!(AccountTransferHistory::Month.days(), Some(30));
    assert_eq!(AccountTransferHistory::Year.days(), Some(365));
    assert_eq!(AccountTransferHistory::All.days(), None);
    assert_eq!(
        serde_json::to_value(AccountTransferHistory::Month).unwrap(),
        "month"
    );
}
