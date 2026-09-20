mod crypto;
mod device_backup;
mod direct_messages;
mod dome_connections;
mod dome_envelopes;
mod dome_hosting;
mod dome_layout;
mod dome_recovery;
mod dome_transition;
mod envelope;
mod game;
mod identity_export;
mod ids;
mod live;
mod media;
mod metaverse_audio;
mod metaverse_resource_budget;
mod posts;
mod private_channels;
mod profile;
mod reactions;
mod rendezvous;
mod trust_observations;
pub mod wire;

#[cfg(test)]
mod tests;

pub use crypto::{
    DocsAuthorSeed, KukuriKeys, LEGACY_SECRET_HRP, encode_secret_key_bech32, generate_keys,
    is_placeholder_secret,
};
pub use device_backup::{
    DEVICE_BACKUP_CHUNK_BYTES, DEVICE_BACKUP_COMPONENT_VERSION, DEVICE_BACKUP_FORMAT_VERSION,
    DEVICE_BACKUP_MAX_ENTRY_BYTES, DEVICE_BACKUP_MAX_ENTRY_COUNT, DEVICE_BACKUP_MAX_TOTAL_BYTES,
    DEVICE_BACKUP_MIN_PASSPHRASE_CHARS, DeviceBackupEntryV1, DeviceBackupManifestV1,
    DeviceBackupReader, DeviceBackupWriter,
};
pub use direct_messages::{
    DirectMessageAckV1, DirectMessageAttachmentKind, DirectMessageAttachmentManifestV1,
    DirectMessageEncryptedAttachmentV1, DirectMessageEncryptedBlobRefV1, DirectMessageFrameV1,
    DirectMessagePayloadV1, build_direct_message_ack, decrypt_direct_message_attachment,
    decrypt_direct_message_frame, derive_direct_message_topic, direct_message_id_for_participants,
    encrypt_direct_message_attachment, encrypt_direct_message_frame,
};
pub use dome_connections::{
    DOME_CONNECTION_DRAIN_MILLIS, DOME_CONNECTION_MAX_OPEN_OUTBOUND,
    DOME_CONNECTION_MAX_PER_PEER_SLOT, DOME_CONNECTION_MAX_RECEIVER_QUEUE, DomeComponentTopologyV1,
    DomeConnectionAgreementV1, DomeConnectionEndpointV1, DomeConnectionProposalV1,
    DomeConnectionRecordV1, DomeConnectionStatusV1, DomeConnectionTerminalReasonV1,
    DomeProposalDerivedStatusV1, DomeProposalSelectionV1, DomeRejectedConnectionV1,
    DomeTopologyResolutionV1, DomeTopologyV1, SignedDomeConnectionAgreementV1,
    build_dome_connection_agreement_envelope, build_dome_connection_proposal_envelope,
    build_dome_connection_selection_envelope, build_signed_dome_connection_agreement,
    derive_dome_proposal_status, opposite_dome_direction, resolve_dome_topology,
    resolve_dome_topology_candidates, validate_dome_connection_agreement,
    validate_dome_connection_proposal, validate_dome_connection_record,
    validate_dome_connection_selection, verify_signed_dome_connection_agreement,
};
pub use dome_envelopes::{
    build_dome_instance_envelope, build_dome_move_envelope, build_dome_preset_envelope,
};
pub use dome_hosting::{
    DOME_HOST_HEARTBEAT_INTERVAL_MILLIS, DOME_HOSTING_HEARTBEAT_GRACE_MILLIS,
    DOME_HOSTING_MAX_LEASE_MILLIS, DOME_PARTICIPANT_KEEPALIVE_INTERVAL_MILLIS,
    DOME_PARTICIPANT_TIMEOUT_MILLIS, DOME_SNAPSHOT_RING_CAPACITY, DomeHostHeartbeatV1,
    DomeHostTargetV1, DomeHostingAcceptanceV1, DomeHostingActivationV1, DomeHostingCloseV1,
    DomeHostingLeaseV1, DomeHostingRecordV1, DomeHostingStateKindV1, DomeHostingStateV1,
    DomePhysicsBodyKindV1, DomePhysicsBodyV1, DomePhysicsSnapshotV1, DomeSessionInputKindV1,
    DomeSessionInputV1, SignedDomeHostHeartbeatV1, SignedDomeHostingAcceptanceV1,
    SignedDomeHostingActivationV1, SignedDomeHostingCloseV1, SignedDomeHostingLeaseV1,
    SignedDomePhysicsSnapshotV1, SignedDomeSessionInputV1, accept_dome_hosting_lease,
    activate_dome_hosting_lease, build_signed_dome_host_heartbeat, build_signed_dome_hosting_lease,
    build_signed_dome_physics_snapshot, build_signed_dome_session_input, close_dome_hosting_lease,
    dome_hosting_lease_digest, resolve_dome_hosting_state, validate_dome_hosting_lease,
    verify_signed_dome_host_heartbeat, verify_signed_dome_hosting_lease,
    verify_signed_dome_physics_snapshot, verify_signed_dome_session_input,
};
pub use dome_layout::{
    DOME_LAYOUT_COMMIT_MIN_INTERVAL_MILLIS, DomeLayoutCandidateV1, DomeLayoutCommitV1,
    SignedDomeLayoutCandidateV1, SignedDomeLayoutCommitV1, build_signed_dome_layout_candidate,
    build_signed_dome_layout_commit, dome_layout_candidate_digest,
    verify_signed_dome_layout_candidate, verify_signed_dome_layout_commit,
};
pub use dome_recovery::{
    DomeEvacuationCandidateKindV1, DomeEvacuationCandidateV1, DomeEvacuationPhaseV1,
    DomeEvacuationReasonV1, DomeHostLivenessV1, order_dome_evacuation_candidates,
    resolve_dome_host_liveness,
};
pub use dome_transition::{
    DOME_ACCESS_PROOF_TTL_MILLIS, DOME_TRANSITION_CROSSING_HYSTERESIS_CM,
    DOME_TRANSITION_TICKET_TTL_MILLIS, DomeBoundaryStateV1, DomeSpatialAccessProofV1,
    DomeSpatialAccessStatementV1, DomeTransitionAccessDecisionV1, DomeTransitionAdmissionRequestV1,
    DomeTransitionAdmissionTicketV1, DomeTransitionDenialReasonV1, DomeTransitionPhaseV1,
    advance_dome_transition_phase, build_dome_spatial_access_proof, crossed_dome_transition_center,
    dome_transition_axis_cm, dome_transition_component_position_cm,
    dome_transition_local_position_cm, dome_transition_progress_millionths,
    transform_avatar_between_domes_cm,
};
pub(crate) use envelope::sign_envelope_at;
pub use envelope::{
    GossipHint, HintObjectRef, KukuriAuthEnvelopeContentV1, KukuriEnvelope, sign_envelope_json,
};
pub use game::{
    DomeCustomizationV1, DomeDirection, DomeEnvironmentV1, DomeInstanceManifestV1,
    DomeInstanceStateDocV1, DomeInstanceStatusV1, DomeMaterialPreset, DomeMovePhaseV1,
    DomeMoveRecordV1, DomeMoveStateDocV1, DomePresetManifestV1, DomePresetRefV1,
    DomePresetStateDocV1, DomeRelationshipDetachV1, DomeSurfaceCustomizationV1, FIXED_DOME_SPEC_ID,
    FixedDomeEndpointV1, FixedDomeSpecV1, GameParticipant, GameRoomKind, GameRoomManifestBlobV1,
    GameRoomStateDocV1, GameRoomStatus, GameScoreEntry, METAVERSE_WORLD_VERSION,
    MetaverseAssetKind, MetaverseAssetRef, MetaverseColliderV1, MetaverseDomeV1,
    MetaverseInteractionKind, MetaversePersistentPropV1, MetaversePrimitive,
    MetaverseRoomChatMessageV1, MetaverseRoomEventEnvelopeContentV1, MetaverseRoomEventV1,
    MetaverseRoomPresenceV1, MetaverseRoomSpawnV1, MetaverseRoomStateV1, SharedRoomObjectV1,
    SpatialContextV1, build_game_session_envelope, build_metaverse_room_event_envelope,
    fallback_capsule_collider, fixed_dome_v1, interpolate_dome_environment,
    resolve_metaverse_room_state, validate_dome_customization, validate_dome_instance_manifest,
    validate_dome_move_record, validate_dome_preset_manifest, validate_dome_relationship_scope,
    validate_metaverse_collider, validate_metaverse_room_state,
};
pub use identity_export::{
    ACCOUNT_KEY_EXPORT_KDF, ACCOUNT_KEY_EXPORT_MIN_PASSPHRASE_CHARS, ACCOUNT_KEY_EXPORT_PREFIX,
    ACCOUNT_KEY_EXPORT_VERSION, AccountKeyExportPreview, decrypt_account_key_export,
    encrypt_account_key_export, preview_account_key_export,
};
pub use ids::{
    BlobHash, ChannelId, EnvelopeId, Pubkey, ReplicaId, TopicId, author_profile_topic_id,
};
pub use live::{
    LiveSessionManifestBlobV1, LiveSessionStateDocV1, LiveSessionStatus, LiveSignalKind,
    build_live_session_envelope,
};
pub use media::{
    AssetRef, AssetRole, DOME_INSTANCE_MANIFEST_MIME, DOME_PRESET_MANIFEST_MIME,
    GAME_MANIFEST_MIME, KukuriMediaManifestV1, LIVE_MANIFEST_MIME, ManifestBlobRef,
    MediaManifestItem, blob_hash, build_media_manifest_envelope,
};
pub use metaverse_audio::{
    METAVERSE_AUDIO_MAX_SAMPLES_PER_FRAME, METAVERSE_AUDIO_SAMPLE_RATE_HZ,
    METAVERSE_MEDIA_TTL_MILLIS, MetaverseSpatialAudioFrameV1, connection_opening_audio_distance_cm,
    metaverse_room_event_is_live, spatial_audio_gain_milli, validate_metaverse_room_event_content,
    validate_metaverse_room_event_for_instance,
};
pub use metaverse_resource_budget::{
    ClientResourceBudget, DomeResourceBudget, HostResourceBudget, MetaverseAssetBudgetMetadataV1,
    MetaverseBudgetResource, MetaverseBudgetScope, MetaverseDomeResourceUsage,
    MetaverseResourceBudgetConfig, MetaverseResourceMetricCountV1, MetaverseResourceMetricsV1,
    MetaverseResourceRejection, MetaverseResourceRejectionReason, PlayerResourceBudget,
    inspect_metaverse_asset, metaverse_dome_resource_usage, validate_dome_asset_budget,
    validate_metaverse_asset_metadata,
};
pub use posts::{
    ADULT_CONTENT_LABEL, CanonicalPostHeader, ChannelRef, KukuriPostEnvelopeContentV1,
    KukuriPostObjectV1, KukuriPostWithdrawalEnvelopeContentV1, ObjectStatus, ObjectVisibility,
    PayloadRef, PostWithdrawalReason, PostWithdrawalV1, RepostSourceSnapshotV1, ThreadRef,
    TimelineScope, WithdrawalReasonVisibility, build_post_envelope,
    build_post_envelope_with_payload, build_post_envelope_with_payload_in_channel,
    build_post_withdrawal_envelope, build_repost_envelope, has_adult_content_label,
    timeline_sort_key, verify_post_withdrawal,
};
pub use private_channels::{
    ChannelAudienceKind, ChannelSharingState, CreatePrivateChannelInput, FriendOnlyGrantPreview,
    FriendOnlyGrantTokenV1, FriendPlusSharePreview, FriendPlusShareTokenV1,
    KukuriFriendOnlyGrantEnvelopeContentV1, KukuriFriendPlusShareEnvelopeContentV1,
    KukuriPrivateChannelInviteEnvelopeContentV1, PrivateChannelEpochHandoffGrantDocV1,
    PrivateChannelEpochHandoffGrantPayloadV1, PrivateChannelInvitePreview,
    PrivateChannelInviteTokenParams, PrivateChannelInviteTokenV1, PrivateChannelJoinMode,
    PrivateChannelMetadataDocV1, PrivateChannelParticipantDocV1, PrivateChannelPolicyDocV1,
    build_friend_only_grant_token, build_friend_plus_share_token,
    build_private_channel_epoch_handoff_grant_envelope, build_private_channel_invite_token,
    build_private_channel_participant_envelope, build_private_channel_policy_envelope,
    decrypt_private_channel_epoch_handoff_grant, encrypt_private_channel_epoch_handoff_grant,
    parse_friend_only_grant_token, parse_friend_plus_share_token,
    parse_private_channel_epoch_handoff_grant, parse_private_channel_invite_token,
    parse_private_channel_participant, parse_private_channel_policy,
};
pub use profile::{
    AuthorProfileDocV1, AuthorProfilePostDocV1, AuthorProfileRepostDocV1, BlockEdge,
    BlockEdgeDocV1, BlockEdgeStatus, FollowEdge, FollowEdgeDocV1, FollowEdgeStatus,
    KukuriBlockEdgeEnvelopeContentV1, KukuriFollowEdgeEnvelopeContentV1,
    KukuriProfileEnvelopeContentV1, KukuriProfilePostEnvelopeContentV1,
    KukuriProfileRepostEnvelopeContentV1, Profile, ProfilePost, ProfileRepost,
    build_block_edge_envelope, build_follow_edge_envelope, build_profile_envelope,
    build_profile_post_envelope, build_profile_repost_envelope, parse_block_edge,
    parse_follow_edge, parse_profile, parse_profile_post, parse_profile_repost,
};
pub use reactions::{
    CustomReactionAssetDocV1, CustomReactionAssetSnapshotV1,
    KukuriCustomReactionAssetEnvelopeContentV1, KukuriReactionEnvelopeContentV1, ReactionDocV1,
    ReactionKeyKind, ReactionKeyV1, build_custom_reaction_asset_envelope, build_reaction_envelope,
    deterministic_reaction_id, parse_custom_reaction_asset, parse_reaction,
};
pub use rendezvous::{private_topic_rendezvous_key_hex_secret, public_topic_rendezvous_key};
pub use trust_observations::{
    KukuriMuteObservationEnvelopeContentV1, MUTE_OBSERVATION_KIND, MuteObservationStatus,
    TrustObservation, TrustObservationKind, build_mute_observation_envelope,
    parse_trust_observation,
};
