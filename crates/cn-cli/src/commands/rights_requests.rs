use anyhow::{Context, Result};
use kukuri_cn_core::{
    LegalDataCipher, action_rights_request, get_rights_request_with_sensitive, initialize_database,
    list_rights_requests_with_sensitive, transition_rights_request,
};
use kukuri_cn_protocol::RightsRequestStatus;
use sqlx::PgPool;

use crate::{RightsRequestStatusArg, RightsRequestsAction};

pub(super) async fn run(pool: &PgPool, action: RightsRequestsAction) -> Result<()> {
    initialize_database(pool).await?;
    let cipher = LegalDataCipher::from_key_material(
        &std::env::var("COMMUNITY_NODE_LEGAL_DATA_KEY")
            .context("COMMUNITY_NODE_LEGAL_DATA_KEY is required")?,
    )?;
    match action {
        RightsRequestsAction::List { limit, offset } => {
            let requests = list_rights_requests_with_sensitive(
                pool,
                &cipher,
                limit,
                offset,
                chrono::Utc::now(),
            )
            .await?;
            if requests.is_empty() {
                println!("no rights requests");
            }
            for request in requests {
                println!(
                    "{}  {}  {}/{}  scope={:?}  status={:?}  version={}",
                    request.created_at.to_rfc3339(),
                    request.id,
                    request.subject_kind,
                    request.subject_id,
                    request.scope_status,
                    request.status,
                    request.version,
                );
            }
        }
        RightsRequestsAction::Show { id } => {
            match get_rights_request_with_sensitive(pool, &cipher, &id, chrono::Utc::now()).await? {
                Some(request) => println!("{}", serde_json::to_string_pretty(&request)?),
                None => println!("rights request not found: {id}"),
            }
        }
        RightsRequestsAction::Transition {
            id,
            expected_version,
            actor,
            status,
            public_message,
            delivery_status,
        } => {
            let request = transition_rights_request(
                pool,
                &id,
                expected_version,
                &actor,
                status.into(),
                public_message.as_deref(),
                &delivery_status,
                chrono::Utc::now(),
            )
            .await?;
            println!(
                "rights request transitioned: id={} status={:?} version={}",
                request.id, request.status, request.version
            );
        }
        RightsRequestsAction::Action {
            id,
            expected_version,
            actor,
            capabilities,
            public_message,
        } => {
            let result = action_rights_request(
                pool,
                &id,
                expected_version,
                &actor,
                capabilities.into_iter().map(Into::into).collect(),
                &public_message,
                chrono::Utc::now(),
            )
            .await?;
            println!(
                "rights request actioned: id={} prevention={} removed_index_scopes={} version={}",
                result.request.id,
                result.prevention.decision.id,
                result.prevention.removed_index_scopes.len(),
                result.request.version,
            );
        }
    }
    Ok(())
}

impl From<RightsRequestStatusArg> for RightsRequestStatus {
    fn from(value: RightsRequestStatusArg) -> Self {
        match value {
            RightsRequestStatusArg::Received => Self::Received,
            RightsRequestStatusArg::NeedsInformation => Self::NeedsInformation,
            RightsRequestStatusArg::Reviewing => Self::Reviewing,
            RightsRequestStatusArg::SenderContacting => Self::SenderContacting,
            RightsRequestStatusArg::Actioned => Self::Actioned,
            RightsRequestStatusArg::Declined => Self::Declined,
            RightsRequestStatusArg::OutOfScope => Self::OutOfScope,
            RightsRequestStatusArg::Withdrawn => Self::Withdrawn,
        }
    }
}
