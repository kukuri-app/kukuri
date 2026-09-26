//! WP-Q7 T5 の lock 分類 contract。
//!
//! desktop-runtime のテストが取得する共有資源 lock(kukuri-test-support の
//! `TestResource`)の全取得箇所を、ファイル別・資源別の分類表として固定する。
//! 本表に載らないファイルは「lock 不要」分類を意味し、取得 0 件であることを
//! 同時に検証する。
//!
//! テストの lock 取得を追加・削除・移動した場合は、本表を更新して分類を
//! 宣言すること(未分類のままの追加はこのテストが fail する)。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// (tests/ からの相対パス, TestResource variant 名, 取得数)。
const LOCK_CLASSIFICATION: &[(&str, &str, usize)] = &[
    ("account_logout.rs", "IdentityStorage", 2),
    ("community_node/admission.rs", "CommunityNodeServer", 6),
    ("community_node/config.rs", "ProcessEnvironment", 5),
    ("community_node/connectivity.rs", "IrohNetwork", 6),
    ("community_node/dome_hosting.rs", "CommunityNodeServer", 6),
    (
        "community_node/dome_hosting/transfer_contract.rs",
        "CommunityNodeServer",
        7,
    ),
    ("community_node/index_query.rs", "CommunityNodeServer", 14),
    // #1055 で content advisory の合成・issuer 照合・無効化の 4 test を追加。
    // #1056 で採用 OFF と既定有効の 2 test を追加。
    // oversized-files の上限に合わせ、index_query.rs から子モジュールへ分けている。
    (
        "community_node/index_query/advisory_lookup.rs",
        "CommunityNodeServer",
        7,
    ),
    (
        "community_node/index_query/content_advisory.rs",
        "CommunityNodeServer",
        6,
    ),
    (
        "community_node/indexing_status.rs",
        "CommunityNodeServer",
        4,
    ),
    ("community_node/metadata.rs", "CommunityNodeServer", 10),
    (
        "community_node/report_submission.rs",
        "CommunityNodeServer",
        9,
    ),
    ("community_node/scheduler.rs", "CommunityNodeServer", 6),
    ("community_node/session.rs", "CommunityNodeServer", 7),
    (
        "community_node/tester_feedback_submission.rs",
        "CommunityNodeServer",
        4,
    ),
    ("community_node/trust_gates.rs", "CommunityNodeServer", 8),
    (
        "community_node/trust_observations.rs",
        "CommunityNodeServer",
        11,
    ),
    ("community_node/trust_relation.rs", "CommunityNodeServer", 5),
    ("device_backup.rs", "IdentityStorage", 8),
    ("device_backup/recovery.rs", "IdentityStorage", 13),
    ("identity_restart.rs", "IdentityStorage", 2),
    ("media_blob_restore.rs", "IrohNetwork", 11),
    ("private_channels/friend_only.rs", "IrohNetwork", 1),
    ("private_channels/friend_plus.rs", "IrohNetwork", 1),
    ("private_channels/invite.rs", "IrohNetwork", 3),
    ("private_channels/persistence.rs", "IrohNetwork", 2),
    ("protected_migration.rs", "IdentityStorage", 3),
    ("receive_binding.rs", "IdentityStorage", 1),
    ("runtime_events.rs", "CommunityNodeServer", 2),
    ("runtime_events.rs", "IdentityStorage", 2),
    ("seeded_dht.rs", "DhtTestnet", 3),
    ("static_peer.rs", "IrohNetwork", 2),
];

/// 取得呼び出しの検出パターン。本ファイル自身がマッチしないよう分割して組み立てる。
fn acquisition_pattern() -> String {
    format!("{}({}::", "lock_test_resource", "TestResource")
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn scan_acquisitions(tests_root: &Path) -> BTreeMap<(String, String), usize> {
    let pattern = acquisition_pattern();
    let mut files = Vec::new();
    collect_rs_files(tests_root, &mut files);
    let mut acquisitions: BTreeMap<(String, String), usize> = BTreeMap::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let relative = path
            .strip_prefix(tests_root)
            .expect("path under tests root")
            .to_string_lossy()
            .replace('\\', "/");
        for (offset, _) in source.match_indices(&pattern) {
            let variant_start = offset + pattern.len();
            let variant_end = source[variant_start..]
                .find(')')
                .map(|end| variant_start + end)
                .unwrap_or_else(|| panic!("unterminated acquisition expression in {relative}"));
            let variant = source[variant_start..variant_end].trim().to_string();
            *acquisitions.entry((relative.clone(), variant)).or_default() += 1;
        }
    }
    acquisitions
}

/// Q7 T5 の受入条件「全取得箇所が分類され、未分類追加で test が fail」を固定する。
#[test]
fn lock_acquisitions_match_declared_classification() {
    let tests_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests");
    let actual = scan_acquisitions(&tests_root);
    let expected: BTreeMap<(String, String), usize> = LOCK_CLASSIFICATION
        .iter()
        .map(|(file, resource, count)| (((*file).to_string(), (*resource).to_string()), *count))
        .collect();
    assert_eq!(
        actual, expected,
        "lock acquisitions drifted from the declared classification. \
         テストの lock 取得を追加・削除・移動した場合は \
         support/lock_contract.rs の LOCK_CLASSIFICATION を更新して分類を宣言すること \
         (表に無いファイルは lock 不要分類 = 取得 0 件を要求する)"
    );
    let total: usize = expected.values().sum();
    assert_eq!(
        total, 177,
        "classification total drifted from the Q7 T6 baseline(#1020 で Dome delete・stale input 試験を各 1 件追加、#711 で index_query 試験を 1 件、\
         #802 で tester_feedback_submission 試験を 3 件、#862 で config 永続化試験を 2 件、\
         #855 で device_backup 試験を 7 件、recovery 試験を 13 件へ拡充、\
         #857 で report consent gate 試験を 3 件、\
         Dome hosting consent gate 試験を 6 件、固定面回帰で session・metadata・report を各 1 件追加、
         transition rerun で index_query 試験を 3 件、tester feedback・trust relation 試験を各 1 件追加、
         #921 で Dome transfer contract の CommunityNodeServer 取得を 5 件追加、\
         #975 で indexing_status 試験の CommunityNodeServer 取得を 4 件追加、\
         #1005 で account_logout 試験の IdentityStorage 取得を 2 件追加、         #1055 で index_query の content advisory 試験の CommunityNodeServer 取得を 4 件追加、\n         #1056 で content advisory 試験を 2 件、一括照会試験を 7 件追加、\n         #1061 で観測提供試験を 11 件、表示判断試験を 8 件追加、\n         #1221 で受信bindingの実account起動試験に IdentityStorage 取得を 1 件、通知event taskのshutdown/drop試験に各 1 件、N70のCN候補二端末試験に CommunityNodeServer 取得を 1 件追加、         #1221 R5-G で未完了の移行の backup 拒否試験と保護データの移行・復元試験に IdentityStorage 取得を各 1 件、追いついた後の pin と依存 record を待つ本人 envelope の試験に各 1 件追加)"
    );
}
