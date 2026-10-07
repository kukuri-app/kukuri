//! 公開 blob の保持端末の検索（#1632 D7、ADR 0063 §7）の capability の開示 contract（`generate.rs` から分離）。

mod generate_support;
use generate_support::doc;

use kukuri_cn_operator::{
    SAMPLE_CONFIG, build_manifest, generate_all, load_and_validate, policy_snapshot_revision,
};

/// #1632 D7: 公開 blob の保持端末の検索は既定で無効。有効にすると manifest・プライバシーポリシー・外部送信表示に
/// 出し（Mainline DHT と補助 index への送信）、同意の revision が変わる。
#[test]
fn public_blob_search_is_opt_in_and_disclosed() {
    let off = load_and_validate(SAMPLE_CONFIG).unwrap();
    let on = load_and_validate(
        &SAMPLE_CONFIG.replace("public_blob_search: false", "public_blob_search: true"),
    )
    .unwrap();
    for (resolved, enabled) in [(&off, false), (&on, true)] {
        assert_eq!(
            build_manifest(resolved).capabilities.public_blob_search,
            enabled
        );
        let files = generate_all(resolved);
        let ext = doc(&files, "external-transmission-notice.md");
        let active_section = ext.split("送信していない").next().unwrap();
        assert_eq!(active_section.contains("### Mainline DHT"), enabled);
        assert_eq!(
            active_section.contains("### kukuri が運用する補助 index"),
            enabled
        );
        assert_eq!(
            doc(&files, "privacy-policy.md").contains("### 公開 blob の保持端末の検索"),
            enabled
        );
    }
    assert_ne!(
        policy_snapshot_revision(&off),
        policy_snapshot_revision(&on)
    );
}
