//! trust / relation read surface(#415 / ADR 0026)。

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use kukuri_cn_core::{
    ApiError, ApiResult, TrustBasisCursor, clear_relation_optout, filter_relation_visible,
    get_relation_optout, latest_successful_relation_snapshot_id, list_active_relation_observations,
    list_disclosed_trust_basis_page, list_trust_basis_page, load_trust_totals,
    relation_pair_is_suppressed, require_bearer_identity, require_consents, set_relation_optout,
    trust_observation_revisions,
};
use kukuri_cn_protocol::{
    RELATION_NOT_FOUND_CODE, RELATION_VISIBILITY_NOT_ACTIVATED_CODE,
    RELATION_VISIBILITY_NOT_CONFIGURED_CODE, RelationNeighborsResponse, RelationOptoutResponse,
    RelationReadResponse, TRUST_EVALUATIONS_MAX_TARGETS, TRUST_READ_NOT_ACTIVATED_CODE,
    TRUST_READ_NOT_CONFIGURED_CODE, TrustEvaluationItem, TrustEvaluationsRequest,
    TrustEvaluationsResponse, TrustReadView, TrustUserReadResponse, normalize_pubkey,
};
use kukuri_cn_trust::{
    PullAudience, RelationAdjustment, TrustRiskInput, apply_viewer_relation, build_trust_read,
    compose_relation_adjustment, cross_node_trust_disclosure, relation_version,
};
use serde::Deserialize;

use crate::errors::{TrustRelationError, TrustRelationOperation, trust_relation_error};
use crate::state::{RelationVisibilityState, TrustReadState, UserApiState};

/// trust / relation read 共通の前処理: 機能ゲート(未構成なら 404)+ 認証 + consent。
///
/// `CommunityLocalTrust` capability が `Availability::Planned` の既定状態では構成されず、
/// この node は trust / relation read を提供しない。認証は challenge への鍵署名を検証して
/// 発行された bearer(`BearerIdentity`)であり、**viewer = bearer の pubkey に固定**される
/// (`viewer_relative_read_requires_authenticated_viewer` / `relation_read_requires_authenticated_viewer`。
/// 他人を viewer に指定する手段を持たない = なりすまし防止)。
pub(crate) async fn require_trust_read(
    state: &UserApiState,
    headers: &HeaderMap,
) -> ApiResult<(Arc<TrustReadState>, String)> {
    let Some(trust_read) = state.trust_read.clone() else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            TRUST_READ_NOT_CONFIGURED_CODE,
            "this community node does not provide trust / relation reads",
        ));
    };
    if !state.readiness_activation_is_valid().await {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            TRUST_READ_NOT_ACTIVATED_CODE,
            "this community node trust activation is not current",
        ));
    }
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    Ok((trust_read, identity.pubkey))
}

/// distance opt-out 設定・判定の共通前処理。index だけを有効にした node でも利用できる。
async fn require_relation_visibility(
    state: &UserApiState,
    headers: &HeaderMap,
) -> ApiResult<(Arc<RelationVisibilityState>, String)> {
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
            RELATION_VISIBILITY_NOT_ACTIVATED_CODE,
            "this community node relation activation is not current",
        ));
    }
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    Ok((relation_visibility, identity.pubkey))
}

pub(crate) fn parse_target_pubkey(raw: &str) -> Result<String, ApiError> {
    normalize_pubkey(raw).map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_TRUST_QUERY",
            error.to_string(),
        )
    })
}

/// `?cursor=` で basis の続きを取る照会の引数(ADR 0026 §10)。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TrustBasisParams {
    #[serde(default)]
    cursor: Option<String>,
}

impl TrustBasisParams {
    fn cursor(&self) -> Result<Option<TrustBasisCursor>, ApiError> {
        self.cursor
            .as_deref()
            .map(|raw| {
                TrustBasisCursor::parse(raw).ok_or_else(|| {
                    ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "INVALID_TRUST_QUERY",
                        "invalid basis cursor",
                    )
                })
            })
            .transpose()
    }
}

fn load_trust_error(operation: TrustRelationOperation) -> impl Fn(anyhow::Error) -> ApiError {
    move |source| trust_relation_error(TrustRelationError::trust_read(operation, source))
}

/// 閲覧者 viewer から見た各 target の利用者向け view(`trust` = 合算済みの S)を作る
/// (ADR 0026 §8)。
///
/// T は target ごとの集計から閲覧者によらず求め(§10)、R は target への active な観測と
/// viewer → observer の proximity から求める。observer の一覧は戻り値に含めない。`basis` は
/// 単体照会の 1 ページ(一括評価では空)。
async fn evaluate_viewer_trust(
    state: &UserApiState,
    trust_read: &TrustReadState,
    viewer_pubkey: &str,
    targets: &[String],
    basis: &[TrustRiskInput],
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Vec<(String, TrustReadView)>> {
    let mut totals = load_trust_totals(&state.pool, targets)
        .await
        .map_err(load_trust_error(TrustRelationOperation::LoadTrustInputs))?;

    let observation_error = |source| {
        trust_relation_error(TrustRelationError::trust_read(
            TrustRelationOperation::LoadRelationObservations,
            source,
        ))
    };
    let observations = list_active_relation_observations(&state.pool, targets, now)
        .await
        .map_err(observation_error)?;
    let revisions = trust_observation_revisions(&state.pool, targets)
        .await
        .map_err(observation_error)?;
    let snapshot_id = latest_successful_relation_snapshot_id(&state.pool)
        .await
        .map_err(observation_error)?;
    let mut observers: Vec<String> = observations
        .values()
        .flatten()
        .map(|observation| observation.observer_pubkey.clone())
        .filter(|observer| observer != viewer_pubkey)
        .collect();
    observers.sort();
    observers.dedup();
    let proximities = if observers.is_empty() {
        Default::default()
    } else {
        trust_read
            .relation
            .proximity_scores(viewer_pubkey, observers.as_slice())
            .await
            .map_err(|source| {
                TrustRelationError::relation_graph(
                    TrustRelationOperation::ReadProximityScores,
                    source,
                )
            })
            .map_err(trust_relation_error)?
    };

    Ok(targets
        .iter()
        .cloned()
        .map(|target| {
            let target_totals = totals.remove(target.as_str()).unwrap_or_default();
            let trust_view = build_trust_read(
                target.as_str(),
                &target_totals,
                basis,
                now,
                &trust_read.params,
            );
            let adjustment = observations
                .get(target.as_str())
                .map(|items| {
                    compose_relation_adjustment(
                        viewer_pubkey,
                        items.as_slice(),
                        &proximities,
                        now,
                        &trust_read.params,
                    )
                })
                .unwrap_or(RelationAdjustment::NONE);
            let version = relation_version(
                snapshot_id,
                revisions.get(target.as_str()).copied().unwrap_or(0),
            );
            let view = apply_viewer_relation(
                trust_view,
                target_totals.version(),
                adjustment,
                version,
                now,
                &trust_read.params,
            );
            (target, view)
        })
        .collect())
}

/// per-user trust read(ADR 0026 §2.3 / §6.2 / §8 / §10)。
///
/// `trust` は trust 絶対値 T と閲覧者別 relation 値 R を CN 側で合算した S。`absolute` /
/// `relative` / `basis` は T の内訳(寄与 signal の根拠つき)で、`evaluation` に版・期限・
/// 表示 policy を付ける。basis は 50 件ずつで、続きは `basis_next_cursor` を `?cursor=` に渡す。
/// viewer は bearer identity に固定する。
pub(crate) async fn trust_user_read(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Path(pubkey): Path<String>,
    Query(params): Query<TrustBasisParams>,
) -> ApiResult<Json<TrustUserReadResponse>> {
    let (trust_read, viewer_pubkey) = require_trust_read(&state, &headers).await?;
    let target = parse_target_pubkey(pubkey.as_str())?;
    let cursor = params.cursor()?;
    let now = chrono::Utc::now();
    let page = list_trust_basis_page(&state.pool, target.as_str(), cursor.as_ref())
        .await
        .map_err(load_trust_error(TrustRelationOperation::LoadTrustInputs))?;
    let (_, mut view) = evaluate_viewer_trust(
        &state,
        &trust_read,
        viewer_pubkey.as_str(),
        std::slice::from_ref(&target),
        &page.inputs,
        now,
    )
    .await?
    .into_iter()
    .next()
    .expect("one evaluation per requested target");
    view.basis_next_cursor = page.next_cursor;
    Ok(Json(TrustUserReadResponse {
        viewer_pubkey,
        view,
    }))
}

/// 閲覧者向け信頼値の一括評価(ADR 0026 §8.4)。target ごとに合算済みの S と評価 metadata を返す。
pub(crate) async fn trust_evaluations(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<TrustEvaluationsRequest>,
) -> ApiResult<Json<TrustEvaluationsResponse>> {
    let (trust_read, viewer_pubkey) = require_trust_read(&state, &headers).await?;
    if request.targets.is_empty() || request.targets.len() > TRUST_EVALUATIONS_MAX_TARGETS {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_TRUST_QUERY",
            format!("targets must contain 1 to {TRUST_EVALUATIONS_MAX_TARGETS} pubkeys"),
        ));
    }
    let mut targets = request
        .targets
        .iter()
        .map(|raw| parse_target_pubkey(raw.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    targets.sort();
    targets.dedup();
    let now = chrono::Utc::now();
    let evaluations = evaluate_viewer_trust(
        &state,
        &trust_read,
        viewer_pubkey.as_str(),
        targets.as_slice(),
        &[],
        now,
    )
    .await?
    .into_iter()
    .map(|(target_pubkey, view)| TrustEvaluationItem {
        target_pubkey,
        trust: view.trust,
        evaluation: view
            .evaluation
            .expect("viewer evaluation always carries metadata"),
    })
    .collect();
    Ok(Json(TrustEvaluationsResponse {
        viewer_pubkey,
        evaluations,
    }))
}

/// cross-node pull(ADR 0026 §6.3 / §10)。
///
/// **confirmed(known-hash / provider-verdict)な絶対成分のみ**を根拠つきで返す。
/// 相対成分・relation・suspected は visibility に依らず返さない。`visibility` は
/// アクセス範囲: `Local` は返さず(既定)、`SubscribedNodes` は bearer で subscriber と
/// 認証できた要求者のみ、`Public` は匿名でも返す。絶対成分は viewer 非依存のため
/// viewer 証明は要さない(bearer は audience 判定のみに使う)。basis は 50 件ずつで、続きは
/// `basis_next_cursor` を `?cursor=` に渡す。
pub(crate) async fn trust_pull(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Path(pubkey): Path<String>,
    Query(params): Query<TrustBasisParams>,
) -> ApiResult<Json<kukuri_cn_trust::CrossNodeTrustDisclosure>> {
    let Some(_) = state.trust_read.clone() else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            TRUST_READ_NOT_CONFIGURED_CODE,
            "this community node does not provide trust / relation reads",
        ));
    };
    if !state.readiness_activation_is_valid().await {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            TRUST_READ_NOT_ACTIVATED_CODE,
            "this community node trust activation is not current",
        ));
    }
    // bearer が有効なsubscriberはcurrent consent成立後だけSubscribedNodesへ昇格する。
    // 認証できない要求は匿名としてPublic visibilityだけを維持する。
    let audience = match require_bearer_identity(&state.pool, &state.jwt_config, &headers).await {
        Ok(identity) => {
            let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
            PullAudience::SubscribedNodes
        }
        Err(_) => PullAudience::Public,
    };
    let target = parse_target_pubkey(pubkey.as_str())?;
    let cursor = params.cursor()?;
    let now = chrono::Utc::now();
    let load_error = load_trust_error(TrustRelationOperation::LoadTrustPullInputs);
    let totals = load_trust_totals(&state.pool, std::slice::from_ref(&target))
        .await
        .map_err(&load_error)?
        .remove(target.as_str())
        .unwrap_or_default();
    let page =
        list_disclosed_trust_basis_page(&state.pool, target.as_str(), audience, cursor.as_ref())
            .await
            .map_err(&load_error)?;
    let mut disclosure =
        cross_node_trust_disclosure(target.as_str(), &totals, &page.inputs, audience, now);
    disclosure.basis_next_cursor = page.next_cursor;
    Ok(Json(disclosure))
}

/// relation read の応答(pairwise cluster proximity。根拠つき)。
/// pairwise relation read(ADR 0026 §2.4)。viewer = bearer identity。
///
/// どちらかが distance opt-out を選択し、この node の境界より遠い場合だけ、edge が無い場合と
/// 同じ 404 を返す。選択状態そのものは相手へ漏らさない。
pub(crate) async fn relation_user_read(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Path(target): Path<String>,
) -> ApiResult<Json<RelationReadResponse>> {
    let (trust_read, viewer_pubkey) = require_trust_read(&state, &headers).await?;
    let relation_visibility = state.relation_visibility.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            RELATION_VISIBILITY_NOT_CONFIGURED_CODE,
            "this community node does not provide relation distance opt-out",
        )
    })?;
    let target = parse_target_pubkey(target.as_str())?;
    let not_found = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            RELATION_NOT_FOUND_CODE,
            "no relation observed for this pair",
        )
    };
    let proximity = trust_read
        .relation
        .pairwise_proximity(viewer_pubkey.as_str(), target.as_str())
        .await
        .map_err(|source| {
            TrustRelationError::relation_graph(
                TrustRelationOperation::ReadPairwiseProximity,
                source,
            )
        })
        .map_err(trust_relation_error)?
        .ok_or_else(not_found)?;
    if relation_pair_is_suppressed(
        &state.pool,
        viewer_pubkey.as_str(),
        target.as_str(),
        Some(proximity.score),
        relation_visibility.min_proximity,
    )
    .await
    .map_err(|source| {
        TrustRelationError::relation_opt_out(
            TrustRelationOperation::CheckRelationVisibility,
            source,
        )
    })
    .map_err(trust_relation_error)?
    {
        return Err(not_found());
    }
    Ok(Json(RelationReadResponse {
        viewer_pubkey,
        target_pubkey: target,
        proximity,
    }))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RelationNeighborsParams {
    #[serde(default)]
    limit: Option<usize>,
}

/// discovery / surfacing 用の近接近傍(ADR 0026 §6.1 `neighbors`)。viewer = bearer identity。
///
/// viewer または候補が opt-out 済みで、距離境界外の user だけを結果から除外する。
pub(crate) async fn relation_neighbors(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Query(params): Query<RelationNeighborsParams>,
) -> ApiResult<Json<RelationNeighborsResponse>> {
    let (trust_read, viewer_pubkey) = require_trust_read(&state, &headers).await?;
    let relation_visibility = state.relation_visibility.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            RELATION_VISIBILITY_NOT_CONFIGURED_CODE,
            "this community node does not provide relation distance opt-out",
        )
    })?;
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let neighbors = trust_read
        .relation
        .neighbors(viewer_pubkey.as_str(), limit)
        .await
        .map_err(|source| {
            TrustRelationError::relation_graph(TrustRelationOperation::ReadNeighbors, source)
        })
        .map_err(trust_relation_error)?;
    let neighbors = filter_relation_visible(
        &state.pool,
        relation_visibility.relation.as_ref(),
        viewer_pubkey.as_str(),
        neighbors.as_slice(),
        relation_visibility.min_proximity,
    )
    .await
    .map_err(|source| {
        TrustRelationError::relation_graph(TrustRelationOperation::FilterVisibleNeighbors, source)
    })
    .map_err(trust_relation_error)?;
    Ok(Json(RelationNeighborsResponse {
        viewer_pubkey,
        neighbors,
    }))
}

fn relation_optout_response(
    pubkey: String,
    opted_out_at: Option<chrono::DateTime<chrono::Utc>>,
    min_proximity: f64,
) -> RelationOptoutResponse {
    RelationOptoutResponse {
        pubkey,
        opted_out: opted_out_at.is_some(),
        opted_out_at: opted_out_at.map(|at| at.to_rfc3339()),
        min_proximity,
    }
}

/// 本人の distance opt-out 状態と、この node の距離境界を返す。
pub(crate) async fn relation_optout_get(
    State(state): State<UserApiState>,
    headers: HeaderMap,
) -> ApiResult<Json<RelationOptoutResponse>> {
    let (relation_visibility, pubkey) = require_relation_visibility(&state, &headers).await?;
    let opted_out_at = get_relation_optout(&state.pool, pubkey.as_str())
        .await
        .map_err(|source| {
            TrustRelationError::relation_opt_out(TrustRelationOperation::GetOptOut, source)
        })
        .map_err(trust_relation_error)?;
    Ok(Json(relation_optout_response(
        pubkey,
        opted_out_at,
        relation_visibility.min_proximity,
    )))
}

/// 「見えない」opt-out の設定(ADR 0026 §2.6 / §6.3)。
///
/// **自分自身のみ**設定できる(bearer identity に固定)。可逆(DELETE で解除)で、
/// trust には影響しない(troll 判定回避の手段にしない)。social graph canonical の削除でもない。
pub(crate) async fn relation_optout_set(
    State(state): State<UserApiState>,
    headers: HeaderMap,
) -> ApiResult<Json<RelationOptoutResponse>> {
    let (relation_visibility, pubkey) = require_relation_visibility(&state, &headers).await?;
    set_relation_optout(&state.pool, pubkey.as_str())
        .await
        .map_err(|source| {
            TrustRelationError::relation_opt_out(TrustRelationOperation::SetOptOut, source)
        })
        .map_err(trust_relation_error)?;
    let opted_out_at = get_relation_optout(&state.pool, pubkey.as_str())
        .await
        .map_err(|source| {
            TrustRelationError::relation_opt_out(TrustRelationOperation::GetOptOut, source)
        })
        .map_err(trust_relation_error)?;
    Ok(Json(RelationOptoutResponse {
        pubkey,
        opted_out: true,
        opted_out_at: opted_out_at.map(|at| at.to_rfc3339()),
        min_proximity: relation_visibility.min_proximity,
    }))
}

/// 「見えない」opt-out の解除(可逆性の実装)。
pub(crate) async fn relation_optout_clear(
    State(state): State<UserApiState>,
    headers: HeaderMap,
) -> ApiResult<Json<RelationOptoutResponse>> {
    let (relation_visibility, pubkey) = require_relation_visibility(&state, &headers).await?;
    clear_relation_optout(&state.pool, pubkey.as_str())
        .await
        .map_err(|source| {
            TrustRelationError::relation_opt_out(TrustRelationOperation::ClearOptOut, source)
        })
        .map_err(trust_relation_error)?;
    Ok(Json(RelationOptoutResponse {
        pubkey,
        opted_out: false,
        opted_out_at: None,
        min_proximity: relation_visibility.min_proximity,
    }))
}
