//! indexing request の受付(#413)と、ユーザー向け index query(#404)。
//! route 上も /v1/indexing(登録)と /v1/index(検索)で対になっているため同居させる。

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use kukuri_cn_core::{
    ApiError, ApiResult, IndexScopeKind, filter_relation_visible, get_channel_secret,
    insert_indexing_request, is_topic_supported, list_indexing_requests_for_requester,
    mark_index_demand, register_channel_secret, register_channel_secret_with_epoch,
    require_bearer_identity, require_consents, revoke_indexing_request,
    rotate_channel_secret_epoch,
};
use kukuri_cn_indexer::IndexQuery;
use kukuri_cn_protocol::{
    BearerIdentity, CHANNEL_MEMBERSHIP_REQUIRED_CODE, CHANNEL_MEMBERSHIP_SECRET_HEADER,
    INDEX_QUERY_NOT_ACTIVATED_CODE, INDEX_QUERY_NOT_CONFIGURED_CODE,
    INDEXING_REQUEST_NOT_ACTIVATED_CODE, INDEXING_REQUEST_NOT_CONFIGURED_CODE, IndexEntryView,
    IndexQueryParams, IndexQueryResponse, IndexingRequestView, IndexingStatusParams,
    IndexingStatusResponse, IndexingTargetStatus, RELATION_VISIBILITY_NOT_CONFIGURED_CODE,
    RevokeIndexingRequestRequest, SubmitIndexingRequestRequest, SubmitIndexingRequestResponse,
};

use crate::errors::{IndexingError, IndexingOperation, indexing_error};
use crate::state::{RelationVisibilityState, UserApiState};

/// user からの indexing request を受け付けて保存する(#413 / ADR 0025 §2.2 / §6.3)。
///
/// 認証済み(bearer)+ consent 済み user のみ要求できる。request は index を保証しない: operator が
/// supported set に入れ、さらに safety verdict が `allow` の content だけが index される多段ゲートの
/// 入口である。
///
/// - public topic: target_id(topic_id)を pending request として保存する。
/// - private channel: channel secret(capability)の提示が必須。secret を提示できること自体を channel
///   権限の証明とみなす(ADR 0025 §6.3。CN は新権限体系を作らない)。secret は at-rest 暗号化して保存し、
///   cn-indexer が Model C と同じ機構で `channel::` replica を sync する。channel secret 暗号鍵が未設定の
///   node は private channel request を受け付けない(平文保存しないため)。
/// - 受付の門(#713): 索引参照が未構成・有効化失効のノードは申請を受け付けない。索引を
///   提供しないノードへ秘密値が保存されるのを防ぐ(検査は read 面と同じ順で認証より先)。
pub(crate) async fn submit_indexing_request(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<SubmitIndexingRequestRequest>,
) -> ApiResult<Json<SubmitIndexingRequestResponse>> {
    let identity = require_indexing_gate(&state, &headers).await?;

    let kind = match request.kind.trim() {
        "public_topic" => IndexScopeKind::PublicTopic,
        "private_channel" => IndexScopeKind::PrivateChannel,
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "INVALID_INDEXING_REQUEST",
                "kind must be `public_topic` or `private_channel`",
            ));
        }
    };
    let target_id = request.target_id.trim();
    if target_id.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_INDEXING_REQUEST",
            "target_id is required",
        ));
    }

    // private channel は capability(secret)の提示が必須。これが権限の証明を兼ねる。
    if kind == IndexScopeKind::PrivateChannel {
        let secret_hex = request
            .channel_secret_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(secret_hex) = secret_hex else {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "CHANNEL_SECRET_REQUIRED",
                "private channel indexing requires the channel secret",
            ));
        };
        // channel secret を平文保存しないため、暗号鍵未設定の node は受け付けない。
        let Some(cipher) = state.channel_secret_cipher.as_ref() else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "CHANNEL_INDEXING_NOT_CONFIGURED",
                "this community node does not accept private channel indexing requests",
            ));
        };
        // first-writer-wins: 別 requester が別 secret で既存 capability を上書きできないようにする。
        // 同一 secret の再提示は冪等。別 secret による乗っ取りは 409 で拒否する。
        let register = if let (Some(previous_epoch), Some(previous_secret), Some(epoch_id)) = (
            request.previous_epoch_id.as_deref(),
            request.previous_channel_secret_hex.as_deref(),
            request.epoch_id.as_deref(),
        ) {
            rotate_channel_secret_epoch(
                &state.pool,
                cipher,
                identity.pubkey.as_str(),
                target_id,
                (previous_epoch, previous_secret),
                (epoch_id, secret_hex),
            )
            .await
        } else if request.previous_epoch_id.is_some()
            || request.previous_channel_secret_hex.is_some()
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "INVALID_INDEXING_REQUEST",
                "epoch rollover requires both previous capability fields and a new epoch",
            ));
        } else if let Some(epoch_id) = request.epoch_id.as_deref() {
            register_channel_secret_with_epoch(&state.pool, cipher, target_id, epoch_id, secret_hex)
                .await
        } else {
            register_channel_secret(&state.pool, cipher, target_id, secret_hex).await
        };
        register
            .map_err(IndexingError::channel_secret)
            .map_err(indexing_error)?;
    } else if request.epoch_id.is_some()
        || request.previous_epoch_id.is_some()
        || request.previous_channel_secret_hex.is_some()
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_INDEXING_REQUEST",
            "epoch_id is only valid for private channels",
        ));
    }

    let stored = insert_indexing_request(&state.pool, identity.pubkey.as_str(), kind, target_id)
        .await
        .map_err(|source| IndexingError::infrastructure(IndexingOperation::RegisterRequest, source))
        .map_err(indexing_error)?;
    Ok(Json(SubmitIndexingRequestResponse {
        request_id: stored.id,
        status: stored.status,
    }))
}

pub(crate) async fn revoke_own_indexing_request(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<RevokeIndexingRequestRequest>,
) -> ApiResult<StatusCode> {
    // Deletion remains possible after policy consent withdrawal or index
    // deactivation; authentication still binds it to one account.
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let target_id = request.target_id.trim();
    if target_id.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_INDEXING_REQUEST",
            "target_id is required",
        ));
    }
    revoke_indexing_request(
        &state.pool,
        identity.pubkey.as_str(),
        request.kind,
        target_id,
    )
    .await
    .map_err(|source| IndexingError::infrastructure(IndexingOperation::RegisterRequest, source))
    .map_err(indexing_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 索引申請面(登録 / 状態読取り)の共通の門(#713 / #975)。
///
/// 順序は「索引参照が構成済み → 有効化(準備完了記録)が有効 → bearer → consent」。未提供・
/// 失効は read 面と同じく認証より先に 404 で存在しない扱いにし、索引を提供しないノードへ
/// 申請(非公開チャンネルでは秘密値)が送られる前に client が縮退判別できるようにする。
/// 状態読取りも同じ門を通すことで、申請できないノードの申請状態を語らない。
async fn require_indexing_gate(
    state: &UserApiState,
    headers: &HeaderMap,
) -> ApiResult<BearerIdentity> {
    if state.index_query.is_none() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            INDEXING_REQUEST_NOT_CONFIGURED_CODE,
            "this community node does not provide indexing, so it does not accept indexing requests",
        ));
    }
    if !state.readiness_activation_is_valid().await {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            INDEXING_REQUEST_NOT_ACTIVATED_CODE,
            "this community node index activation is not current, so it does not accept indexing requests",
        ));
    }
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    Ok(identity)
}

/// 自分の索引申請の状態と、任意の対象の supported 判定を返す(#975)。
///
/// - `requests` は呼出し主(bearer identity)の申請だけ。他利用者の申請や supported set 全体は
///   返さない。却下済みも含む(申請者は自分の却下を確認できる)。
/// - `scope_kind` + `scope_id` を指定すると `target.supported` を併せて返す。非公開チャンネルは
///   範囲指定読みと同じ所属証明(#711)を要求し、未提示・不一致・未登録・鍵未設定は同一の 403 で
///   拒否して索引の有無を漏らさない。自分の申請一覧の取得には所属証明を要求しない。
/// - 読取り専用。scope state(`supported_topics` / `indexing_requests` / `channel_secrets`)を
///   書き換えない。
pub(crate) async fn indexing_status(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Query(params): Query<IndexingStatusParams>,
) -> ApiResult<Json<IndexingStatusResponse>> {
    let identity = require_indexing_gate(&state, &headers).await?;
    let target = parse_scope_pair(params.scope_kind.as_deref(), params.scope_id.as_deref())?;
    if let Some((IndexScopeKind::PrivateChannel, scope_id)) = target.as_ref() {
        require_channel_membership(&state, &headers, scope_id.as_str()).await?;
    }
    let requests = list_indexing_requests_for_requester(&state.pool, identity.pubkey.as_str())
        .await
        .map_err(|source| IndexingError::infrastructure(IndexingOperation::ReadStatus, source))
        .map_err(indexing_error)?
        .into_iter()
        .map(|request| IndexingRequestView {
            request_id: request.id,
            scope_kind: request.kind,
            target_id: request.target_id,
            status: request.status,
            created_at: request.created_at.timestamp_millis(),
            decided_at: request.decided_at.map(|at| at.timestamp_millis()),
        })
        .collect();
    let target = match target {
        Some((scope_kind, scope_id)) => {
            let supported = is_topic_supported(&state.pool, scope_kind, scope_id.as_str())
                .await
                .map_err(|source| {
                    IndexingError::infrastructure(IndexingOperation::ReadStatus, source)
                })
                .map_err(indexing_error)?;
            Some(IndexingTargetStatus {
                scope_kind,
                scope_id,
                supported,
            })
        }
        None => None,
    };
    Ok(Json(IndexingStatusResponse { requests, target }))
}

fn index_query_response(entries: Vec<kukuri_cn_indexer::IndexedEntry>) -> IndexQueryResponse {
    IndexQueryResponse {
        entries: entries
            .into_iter()
            .map(|entry| IndexEntryView {
                scope_kind: entry.scope_kind,
                scope_id: entry.scope_id,
                object_id: entry.object_id,
                author_pubkey: entry.author_pubkey,
                text: entry.text,
                created_at: entry.created_at,
                source_replica_id: (entry.scope_kind
                    == kukuri_cn_core::IndexScopeKind::PublicTopic
                    && entry.source_replica_id.starts_with("bucket::"))
                .then_some(entry.source_replica_id),
                // 真実源の最新 verdict 由来（query gate が充填）。署名済み content_labels は
                // 生成・改変しない（`content_advisories_are_separate_from_signed_content_labels`）。
                content_advisories: entry.content_advisories,
            })
            .collect(),
    }
}

#[test]
fn index_query_retains_the_public_source_replica_locator() {
    let result = index_query_response(vec![kukuri_cn_indexer::IndexedEntry {
        scope_kind: kukuri_cn_core::IndexScopeKind::PublicTopic,
        scope_id: "rust".into(),
        object_id: "post-id".into(),
        author_pubkey: "author".into(),
        text: "derived text".into(),
        created_at: 86_400,
        source_replica_id: "bucket::v1::topic::72757374::1".into(),
        content_advisories: Vec::new(),
    }]);
    let wire = serde_json::to_value(result).expect("index response");
    assert_eq!(
        wire["entries"][0]["source_replica_id"],
        "bucket::v1::topic::72757374::1"
    );
}

#[test]
fn index_query_keeps_legacy_and_private_locator_fields_absent() {
    for (kind, source) in [
        (kukuri_cn_core::IndexScopeKind::PublicTopic, "topic::rust"),
        (
            kukuri_cn_core::IndexScopeKind::PrivateChannel,
            "bucket::v1::channel::63::65::1",
        ),
    ] {
        let result = index_query_response(vec![kukuri_cn_indexer::IndexedEntry {
            scope_kind: kind,
            scope_id: "rust".into(),
            object_id: "post".into(),
            author_pubkey: "author".into(),
            text: "body".into(),
            created_at: 42,
            source_replica_id: source.into(),
            content_advisories: Vec::new(),
        }]);
        let wire = serde_json::to_value(result).expect("wire");
        assert!(wire["entries"][0].get("source_replica_id").is_none());
    }
}

/// index query 共通の前処理: 機能ゲート(未構成なら 404)+ 認証 + consent。
///
/// query 境界(`FailClosedIndexQuery`)を返す。`CommunityIndex` capability が
/// `Availability::Planned` の既定状態では index query は構成されず、この node は
/// search / discovery / recommendation を提供しない。
async fn require_index_query(
    state: &UserApiState,
    headers: &HeaderMap,
) -> ApiResult<(Arc<dyn IndexQuery>, Arc<RelationVisibilityState>, String)> {
    let Some(index_query) = state.index_query.clone() else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            INDEX_QUERY_NOT_CONFIGURED_CODE,
            "this community node does not provide index queries",
        ));
    };
    let Some(relation_visibility) = state.relation_visibility.clone() else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            RELATION_VISIBILITY_NOT_CONFIGURED_CODE,
            "this community node does not provide relation distance opt-out",
        ));
    };
    if !state.readiness_activation_is_valid().await {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            INDEX_QUERY_NOT_ACTIVATED_CODE,
            "this community node index activation is not current",
        ));
    }
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    Ok((index_query, relation_visibility, identity.pubkey))
}

async fn filter_index_entries(
    state: &UserApiState,
    relation_visibility: &RelationVisibilityState,
    viewer_pubkey: &str,
    entries: Vec<kukuri_cn_indexer::IndexedEntry>,
) -> ApiResult<Vec<kukuri_cn_indexer::IndexedEntry>> {
    let authors: Vec<String> = entries
        .iter()
        .map(|entry| entry.author_pubkey.clone())
        .collect();
    let visible = filter_relation_visible(
        &state.pool,
        relation_visibility.relation.as_ref(),
        viewer_pubkey,
        authors.as_slice(),
        relation_visibility.min_proximity,
    )
    .await
    .map_err(|source| {
        IndexingError::infrastructure(IndexingOperation::FilterRelationVisibility, source)
    })
    .map_err(indexing_error)?;
    let visible: std::collections::HashSet<String> = visible.into_iter().collect();
    Ok(entries
        .into_iter()
        .filter(|entry| visible.contains(&entry.author_pubkey))
        .collect())
}

/// 非公開チャンネル範囲指定読みの所属証明(#711 / ADR 0025 §6.3)。
///
/// 提示された channel secret を保存済み capability の復号値と定数時間比較する。
/// 「秘密値の提示が権限の証明」(申請側と同じ原則)を read にも適用し、新しい権限体系は
/// 作らない。未提示・不一致・チャンネル未登録・暗号鍵未設定は同一の安定コードで拒否し、
/// 非所属者に索引の存在有無を漏らさない。提示値・保存値はログへ出さない。
async fn require_channel_membership(
    state: &UserApiState,
    headers: &HeaderMap,
    channel_id: &str,
) -> ApiResult<Option<String>> {
    let denied = || {
        ApiError::new(
            StatusCode::FORBIDDEN,
            CHANNEL_MEMBERSHIP_REQUIRED_CODE,
            "private channel index queries require the channel secret of a participant",
        )
    };
    let presented = headers
        .get(CHANNEL_MEMBERSHIP_SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(presented) = presented else {
        return Err(denied());
    };
    let Some(cipher) = state.channel_secret_cipher.as_ref() else {
        return Err(denied());
    };
    let stored = get_channel_secret(&state.pool, cipher, channel_id)
        .await
        .map_err(|source| {
            IndexingError::infrastructure(IndexingOperation::VerifyChannelMembership, source)
        })
        .map_err(indexing_error)?;
    let Some(stored) = stored else {
        return Err(denied());
    };
    if !constant_time_str_eq(stored.namespace_secret_hex.as_str(), presented) {
        return Err(denied());
    }
    Ok(stored.rotated.then_some(stored.epoch_id).flatten())
}

fn retain_current_private_epoch(
    entries: Vec<kukuri_cn_indexer::IndexedEntry>,
    scope: Option<(&str, &str)>,
) -> Vec<kukuri_cn_indexer::IndexedEntry> {
    let Some((channel_id, epoch_id)) = scope else {
        return entries;
    };
    let legacy = format!("channel::{channel_id}::epoch::{epoch_id}");
    let bucket = format!(
        "bucket::v1::channel::{}::{}::",
        hex::encode(channel_id),
        hex::encode(epoch_id)
    );
    entries
        .into_iter()
        .filter(|entry| {
            entry.source_replica_id == legacy || entry.source_replica_id.starts_with(&bucket)
        })
        .collect()
}

#[test]
fn rotated_capability_never_surfaces_an_old_epoch_hit() {
    let entry = |source: &str| kukuri_cn_indexer::IndexedEntry {
        scope_kind: IndexScopeKind::PrivateChannel,
        scope_id: "room".into(),
        object_id: source.into(),
        author_pubkey: "author".into(),
        text: "body".into(),
        created_at: 1,
        source_replica_id: source.into(),
        content_advisories: Vec::new(),
    };
    let old = "channel::room::epoch::e1";
    let current = "channel::room::epoch::e2";
    let bucket = "bucket::v1::channel::726f6f6d::6532::1";
    let hits = retain_current_private_epoch(
        vec![entry(old), entry(current), entry(bucket)],
        Some(("room", "e2")),
    );
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].source_replica_id, current);
    assert_eq!(hits[1].source_replica_id, bucket);
}

fn constant_time_str_eq(expected: &str, supplied: &str) -> bool {
    if expected.len() != supplied.len() || expected.is_empty() {
        return false;
    }
    expected
        .bytes()
        .zip(supplied.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

/// `scope_kind` / `scope_id` パラメータの組を解釈する。
///
/// 両方指定 = scope 内読み、両方無指定 = 横断。片方のみは 400。
fn parse_index_scope_params(
    params: &IndexQueryParams,
) -> Result<Option<(IndexScopeKind, String)>, ApiError> {
    parse_scope_pair(params.scope_kind.as_deref(), params.scope_id.as_deref())
}

/// `scope_kind` / `scope_id` の組を解釈する(index query と索引状況読取りで共有)。
fn parse_scope_pair(
    scope_kind: Option<&str>,
    scope_id: Option<&str>,
) -> Result<Option<(IndexScopeKind, String)>, ApiError> {
    let scope_kind = scope_kind.map(str::trim).filter(|v| !v.is_empty());
    let scope_id = scope_id.map(str::trim).filter(|v| !v.is_empty());
    match (scope_kind, scope_id) {
        (None, None) => Ok(None),
        (Some(kind), Some(id)) => {
            let kind = IndexScopeKind::parse(kind).map_err(|error| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "INVALID_INDEX_QUERY",
                    error.to_string(),
                )
            })?;
            Ok(Some((kind, id.to_string())))
        }
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_INDEX_QUERY",
            "scope_kind and scope_id must be provided together",
        )),
    }
}

/// limit パラメータ(未指定は既定 20。gate 側で `MAX_QUERY_LIMIT` に丸められる)。
fn index_query_limit(params: &IndexQueryParams) -> usize {
    params.limit.unwrap_or(20)
}

/// ユーザー向け検索(#404 / ADR 0025 §2.7)。
///
/// `scope_kind` + `scope_id` 指定で topic 内検索(基本 UX)、無指定で supported set 横断検索
/// (別画面)。結果は fail-closed query gate を通った `allow` verdict の entry のみ。
pub(crate) async fn index_search(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Query(params): Query<IndexQueryParams>,
) -> ApiResult<Json<IndexQueryResponse>> {
    let (index_query, relation_visibility, viewer_pubkey) =
        require_index_query(&state, &headers).await?;
    let query = params
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "INVALID_INDEX_QUERY",
                "q is required",
            )
        })?;
    let limit = index_query_limit(&params);
    let mut private_epoch = None;
    let entries = match parse_index_scope_params(&params)? {
        Some((scope_kind, scope_id)) => {
            if scope_kind == IndexScopeKind::PrivateChannel {
                private_epoch = require_channel_membership(&state, &headers, scope_id.as_str())
                    .await?
                    .map(|epoch| (scope_id.clone(), epoch));
            }
            if let Err(error) = mark_index_demand(&state.pool, scope_kind, &scope_id).await {
                tracing::warn!(scope_id = %scope_id, %error, "failed to mark index demand");
            }
            index_query
                .search_scope(scope_kind, scope_id.as_str(), query, limit)
                .await
                .map_err(|source| {
                    IndexingError::infrastructure(IndexingOperation::SearchScope, source)
                })
                .map_err(indexing_error)?
        }
        None => index_query
            .search_all(query, limit)
            .await
            .map_err(|source| IndexingError::infrastructure(IndexingOperation::SearchAll, source))
            .map_err(indexing_error)?,
    };
    let entries = retain_current_private_epoch(
        entries,
        private_epoch
            .as_ref()
            .map(|(channel, epoch)| (channel.as_str(), epoch.as_str())),
    );
    let entries = filter_index_entries(
        &state,
        relation_visibility.as_ref(),
        viewer_pubkey.as_str(),
        entries,
    )
    .await?;
    Ok(Json(index_query_response(entries)))
}

/// discovery(新着列挙。#404)。scope 指定で topic 内、無指定で supported set 横断。
///
/// ranking / 関連度スコアリングの具体は ADR 0025 §4 でスコープ外のため、最小 surface として
/// created_at 降順の新着を返す。critical / 非 allow verdict は fail-closed gate で入らない。
pub(crate) async fn index_discovery(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Query(params): Query<IndexQueryParams>,
) -> ApiResult<Json<IndexQueryResponse>> {
    let (index_query, relation_visibility, viewer_pubkey) =
        require_index_query(&state, &headers).await?;
    let limit = index_query_limit(&params);
    let scope = parse_index_scope_params(&params)?;
    let private_epoch = if let Some((IndexScopeKind::PrivateChannel, scope_id)) = scope.as_ref() {
        require_channel_membership(&state, &headers, scope_id.as_str())
            .await?
            .map(|epoch| (scope_id.clone(), epoch))
    } else {
        None
    };
    if let Some((kind, scope_id)) = scope.as_ref()
        && let Err(error) = mark_index_demand(&state.pool, *kind, scope_id).await
    {
        tracing::warn!(scope_id = %scope_id, %error, "failed to mark index demand");
    }
    let entries = index_query
        .list_recent(scope.as_ref().map(|(kind, id)| (*kind, id.as_str())), limit)
        .await
        .map_err(|source| IndexingError::infrastructure(IndexingOperation::Discovery, source))
        .map_err(indexing_error)?;
    let entries = retain_current_private_epoch(
        entries,
        private_epoch
            .as_ref()
            .map(|(channel, epoch)| (channel.as_str(), epoch.as_str())),
    );
    let entries = filter_index_entries(
        &state,
        relation_visibility.as_ref(),
        viewer_pubkey.as_str(),
        entries,
    )
    .await?;
    Ok(Json(index_query_response(entries)))
}

/// recommendation(#404)。supported set 横断の新着列挙を最小 surface として返す。
///
/// ranking アルゴリズムの具体は ADR 0025 §4 でスコープ外。critical verdict が recommendation に
/// 入らないことは fail-closed gate(真実源 + 最新 verdict 突合)が保証する。
pub(crate) async fn index_recommendations(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Query(params): Query<IndexQueryParams>,
) -> ApiResult<Json<IndexQueryResponse>> {
    let (index_query, relation_visibility, viewer_pubkey) =
        require_index_query(&state, &headers).await?;
    let limit = index_query_limit(&params);
    let entries = index_query
        .list_recent(None, limit)
        .await
        .map_err(|source| IndexingError::infrastructure(IndexingOperation::Recommendations, source))
        .map_err(indexing_error)?;
    let entries = filter_index_entries(
        &state,
        relation_visibility.as_ref(),
        viewer_pubkey.as_str(),
        entries,
    )
    .await?;
    Ok(Json(index_query_response(entries)))
}

/// channel secret 登録失敗を HTTP 応答へマップする。
///
/// 既存 capability と異なる secret での上書き(乗っ取り試行)は 409、hex 形式不正等は 400。
#[cfg(test)]
mod error_contract_tests {
    use axum::http::StatusCode;
    use kukuri_cn_core::ChannelSecretConflict;

    use crate::errors::{IndexingError, IndexingOperation, assert_error_contract, indexing_error};

    #[tokio::test]
    async fn channel_secret_error_contracts_are_stable() {
        assert_error_contract(
            indexing_error(IndexingError::channel_secret(
                ChannelSecretConflict::AlreadyRegistered.into(),
            )),
            StatusCode::CONFLICT,
            "CHANNEL_SECRET_CONFLICT",
            "a different channel capability is already registered for this channel",
        )
        .await;
        assert_error_contract(
            indexing_error(IndexingError::channel_secret(anyhow::anyhow!(
                "channel secret must be 32 bytes"
            ))),
            StatusCode::BAD_REQUEST,
            "INVALID_CHANNEL_SECRET",
            "channel secret must be 32 bytes",
        )
        .await;

        for operation in [
            IndexingOperation::RegisterRequest,
            IndexingOperation::SearchScope,
            IndexingOperation::SearchAll,
            IndexingOperation::Discovery,
            IndexingOperation::Recommendations,
            IndexingOperation::FilterRelationVisibility,
            IndexingOperation::VerifyChannelMembership,
            IndexingOperation::ReadStatus,
        ] {
            assert_error_contract(
                indexing_error(IndexingError::infrastructure(
                    operation,
                    anyhow::anyhow!("index backend unavailable"),
                )),
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "index backend unavailable",
            )
            .await;
        }
    }
}
