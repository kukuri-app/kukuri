#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateChannelImportKind {
    InviteOnly,
    FriendOnly,
    FriendPlus,
}

impl PrivateChannelImportKind {
    fn expired_message(self) -> &'static str {
        match self {
            Self::InviteOnly => "private channel invite is expired",
            Self::FriendOnly => "friend-only grant is expired",
            Self::FriendPlus => "friend-plus share is expired",
        }
    }

    fn mutual_relationship_message(self) -> &'static str {
        match self {
            Self::FriendOnly => {
                "friend-only grant import requires a mutual relationship with the channel owner"
            }
            Self::FriendPlus => {
                "friend-plus share import requires a mutual relationship with the sponsor"
            }
            Self::InviteOnly => "private channel invite does not require a mutual relationship",
        }
    }

    fn audience_mismatch_message(self) -> &'static str {
        match self {
            Self::InviteOnly => "invite-only replica audience must be invite_only",
            Self::FriendOnly => "friend-only grant replica audience must be friend_only",
            Self::FriendPlus => "friend-plus share replica audience must be friend_plus",
        }
    }

    fn sharing_closed_message(self) -> &'static str {
        match self {
            Self::InviteOnly => "invite-only access token is no longer open for import",
            Self::FriendOnly => "friend-only grant is no longer open for import",
            Self::FriendPlus => "friend-plus share is no longer open for import",
        }
    }

    fn epoch_mismatch_message(self) -> &'static str {
        match self {
            Self::InviteOnly => "invite-only access token epoch does not match the current policy",
            Self::FriendOnly => "friend-only grant epoch does not match the current policy",
            Self::FriendPlus => "friend-plus share epoch does not match the current policy",
        }
    }

    fn snapshot_timeout_message(self) -> &'static str {
        match self {
            Self::InviteOnly => "timed out waiting for invite-only channel replica sync",
            Self::FriendOnly => "timed out waiting for friend-only channel replica sync",
            Self::FriendPlus => "timed out waiting for friend-plus channel replica sync",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PrivateChannelImportError {
    #[error("{}", .kind.expired_message())]
    Expired { kind: PrivateChannelImportKind },
    #[error("{}", .kind.mutual_relationship_message())]
    MutualRelationshipRequired { kind: PrivateChannelImportKind },
    #[error("{}", .kind.audience_mismatch_message())]
    AudienceMismatch { kind: PrivateChannelImportKind },
    #[error("{}", .kind.sharing_closed_message())]
    SharingClosed { kind: PrivateChannelImportKind },
    #[error("{}", .kind.epoch_mismatch_message())]
    EpochMismatch { kind: PrivateChannelImportKind },
    #[error("{}", .kind.snapshot_timeout_message())]
    SnapshotTimeout { kind: PrivateChannelImportKind },
    #[cfg(test)]
    #[error("sponsor is not an active participant")]
    SponsorInactive,
    #[cfg(test)]
    #[error("timed out waiting for friend-plus sponsor participant sync")]
    SponsorSnapshotTimeout,
}

impl PrivateChannelImportError {
    /// friend-only grant import の再試行可能性(リトライ契約の正本。WP-B11)。
    /// gossip 伝播待ちで解消しうる失敗のみ true。文言ではなく型で判定する。
    pub(crate) fn is_retryable_friend_only(&self) -> bool {
        matches!(
            self,
            Self::MutualRelationshipRequired {
                kind: PrivateChannelImportKind::FriendOnly
            } | Self::EpochMismatch {
                kind: PrivateChannelImportKind::FriendOnly
            } | Self::SnapshotTimeout {
                kind: PrivateChannelImportKind::FriendOnly
            }
        )
    }

    /// friend-plus share import の再試行可能性(リトライ契約の正本。WP-B11)。
    pub(crate) fn is_retryable_friend_plus(&self) -> bool {
        let retryable = matches!(
            self,
            Self::MutualRelationshipRequired {
                kind: PrivateChannelImportKind::FriendPlus
            } | Self::SnapshotTimeout {
                kind: PrivateChannelImportKind::FriendPlus
            }
        );
        // Sponsor 系はテスト専用 variant(production に発生経路が無い)。
        #[cfg(test)]
        let retryable =
            retryable || matches!(self, Self::SponsorInactive | Self::SponsorSnapshotTimeout);
        retryable
    }
}

pub(crate) enum PrivateChannelSnapshotWaitContext {
    Import(PrivateChannelImportKind),
    EpochHandoff,
}

impl PrivateChannelSnapshotWaitContext {
    pub(crate) fn timeout_error(&self) -> anyhow::Error {
        match self {
            Self::Import(kind) => PrivateChannelImportError::SnapshotTimeout { kind: *kind }.into(),
            Self::EpochHandoff => {
                anyhow::anyhow!("timed out waiting for private channel epoch handoff sync")
            }
        }
    }
}
