//! 共有 replica（topic / channel）に書かれる key の種別表（#1065）。
//!
//! 書き込み側（app-api）と読み取り側（cn-indexer の変更通知分類）が同じ語彙で key を扱うための
//! 凍結境界。prefix は互いに素（どの prefix も他の prefix の接頭辞にならない）に保つ。

/// 共有 replica の key 種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SharedReplicaKeyFamily {
    /// 投稿 object（`objects/<object id>/{state,envelope}`）。
    PostObject,
    /// 投稿の撤回（`withdrawals/<object id>/state`）。
    PostWithdrawal,
    /// 投稿者が書いたリンクプレビュー（`link-previews/<object id>/state`。ADR 0051 §7）。
    LinkPreview,
    /// media manifest（`manifests/media/<manifest id>/{state,envelope}`）。
    MediaManifest,
    /// timeline 索引（`indexes/timeline/<sort key>/<object id>`）。
    TimelineIndex,
    /// thread 索引（`indexes/thread/<root id>/<sort key>/<object id>`）。
    ThreadIndex,
    /// reaction（`reactions/<target object id>/<reaction id>/{state,envelope}`）。
    Reaction,
    /// object を持たない署名済み envelope（`envelopes/<envelope id>`。reaction / session 等）。
    Envelope,
    /// live / game session（`sessions/{live,game}/<id>/state`）。
    Session,
    /// private channel の metadata / policy / participant 等（`channels/…`）。
    Channel,
    /// dome 等の metaverse 状態（`metaverse/…`）。
    Metaverse,
}

impl SharedReplicaKeyFamily {
    /// 全種別。`parse` はこの順に照合する。
    pub const ALL: [Self; 11] = [
        Self::PostObject,
        Self::PostWithdrawal,
        Self::LinkPreview,
        Self::MediaManifest,
        Self::TimelineIndex,
        Self::ThreadIndex,
        Self::Reaction,
        Self::Envelope,
        Self::Session,
        Self::Channel,
        Self::Metaverse,
    ];

    /// key の prefix（末尾 `/` 付き）。
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::PostObject => "objects/",
            Self::PostWithdrawal => "withdrawals/",
            Self::LinkPreview => "link-previews/",
            Self::MediaManifest => "manifests/media/",
            Self::TimelineIndex => "indexes/timeline/",
            Self::ThreadIndex => "indexes/thread/",
            Self::Reaction => "reactions/",
            Self::Envelope => "envelopes/",
            Self::Session => "sessions/",
            Self::Channel => "channels/",
            Self::Metaverse => "metaverse/",
        }
    }

    /// key を種別と prefix 以降の残りに分ける。未登録の key は `None`。
    pub fn parse(key: &str) -> Option<(Self, &str)> {
        Self::ALL
            .into_iter()
            .find_map(|family| key.strip_prefix(family.prefix()).map(|rest| (family, rest)))
    }
}
