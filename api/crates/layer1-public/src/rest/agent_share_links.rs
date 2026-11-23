//! Agent-only, principal-bound sharing links. Tokens stay out of persisted
//! records and responses; a browser keeps the secret in the URL fragment.
use super::{idempotency::IdempotencyKey, resource_access_problem::ResourceProblem};
use crate::AuthenticatedResourceCaller;
use axum::response::{IntoResponse, Response};
use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use layer2_core::{resource_access::*, AgentId, SubjectId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

pub(super) fn routes() -> Router<Arc<ResourceAccessService>> {
    Router::new()
        .route("/api/v1/agents/{id}/share-links", get(list).post(create))
        .route("/api/v1/agents/{id}/share-links/{share_id}", delete(revoke))
        .route("/api/v1/agents/{id}/share-recipients", get(recipients))
        .route("/api/v1/agent-share-links/{share_id}/redeem", post(redeem))
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ShareAudience {
    Public,
    Organization { organization_id: String },
    Users { subjects: Vec<String> },
}
impl From<ShareAudience> for AgentShareAudience {
    fn from(value: ShareAudience) -> Self {
        match value {
            ShareAudience::Public => Self::Public,
            ShareAudience::Organization { organization_id } => Self::Organization {
                organization_id: SubjectId::new(organization_id),
            },
            ShareAudience::Users { subjects } => Self::Users {
                subjects: subjects.into_iter().map(SubjectId::new).collect(),
            },
        }
    }
}
impl From<AgentShareAudience> for ShareAudience {
    fn from(value: AgentShareAudience) -> Self {
        match value {
            AgentShareAudience::Public => Self::Public,
            AgentShareAudience::Organization { organization_id } => Self::Organization {
                organization_id: organization_id.to_string(),
            },
            AgentShareAudience::Users { subjects } => Self::Users {
                subjects: subjects.into_iter().map(|s| s.to_string()).collect(),
            },
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct ShareIdentityView {
    subject: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
}
impl From<layer2_core::PublicIdentity> for ShareIdentityView {
    fn from(value: layer2_core::PublicIdentity) -> Self {
        Self {
            subject: value.subject.to_string(),
            display_name: value.display_name,
            avatar_url: value.avatar_url,
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateLinkBody {
    audience: ShareAudience,
    mode: super::resource_access_shapes::ShareModeView,
    /// 32 cryptographically random bytes, encoded as lowercase hex. Only its
    /// SHA-256 digest is retained. Keep the token in the link's URL fragment.
    token: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RedeemLinkBody {
    token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RevokeQuery {
    expected_revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecipientQuery {
    q: String,
}
#[derive(Serialize, ToSchema)]
pub(super) struct ShareLinkView {
    id: String,
    agent_id: String,
    audience: ShareAudience,
    mode: super::resource_access_shapes::ShareModeView,
    shared_by: ShareIdentityView,
    created_at: String,
    revoked_at: Option<String>,
    revision: u64,
    accepted_count: usize,
}
#[derive(Serialize, ToSchema)]
pub(super) struct ShareLinkList {
    items: Vec<ShareLinkView>,
}
#[derive(Serialize, ToSchema)]
pub(super) struct RecipientList {
    items: Vec<ShareIdentityView>,
}
fn invalid() -> ResourceProblem {
    ResourceProblem(ResourceAccessStoreError::Invalid.into())
}
fn credential(caller: &AuthenticatedResourceCaller) -> Option<&str> {
    caller.credential.as_ref().map(|c| c.value())
}
async fn view(
    service: &ResourceAccessService,
    share: ResourceShare,
) -> Result<ShareLinkView, ResourceProblem> {
    let owner = service.share_owner_identity(&share).await?;
    let ResourceSelector::Agent { agent_id } = share.selector else {
        return Err(invalid());
    };
    let link = share.link.ok_or_else(invalid)?;
    Ok(ShareLinkView {
        id: share.id,
        agent_id: agent_id.to_string(),
        audience: link.audience.into(),
        mode: share.mode.into(),
        shared_by: owner.into(),
        created_at: share.created_at,
        revoked_at: share.revoked_at,
        revision: share.revision,
        accepted_count: link.accepted_subjects.len(),
    })
}
#[utoipa::path(get, path="/api/v1/agents/{id}/share-links", tag="agents", params(("id"=String,Path)), responses((status=200,body=ShareLinkList)))]
pub(super) async fn list(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Result<Json<ShareLinkList>, ResourceProblem> {
    let shares = service
        .active_shares(
            &caller.context,
            credential(&caller),
            &ResourceSelector::Agent {
                agent_id: AgentId(id),
            },
        )
        .await?;
    let mut items = Vec::new();
    for share in shares.into_iter().filter(|s| s.link.is_some()) {
        items.push(view(&service, share).await?);
    }
    Ok(Json(ShareLinkList { items }))
}
#[utoipa::path(post, path="/api/v1/agents/{id}/share-links", tag="agents", params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=CreateLinkBody,responses((status=200,body=ShareLinkView)))]
pub(super) async fn create(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    Json(body): Json<CreateLinkBody>,
) -> Result<Json<ShareLinkView>, ResourceProblem> {
    let key = key.into_required_value().map_err(|_| invalid())?;
    let receipt = service
        .create_agent_share_link(
            &caller.context,
            credential(&caller),
            CreateAgentShareLink {
                agent_id: AgentId(id),
                audience: body.audience.into(),
                mode: body.mode.into(),
                token: body.token,
                idempotency_key: key,
            },
        )
        .await?;
    Ok(Json(view(&service, receipt.share).await?))
}
#[utoipa::path(delete, path="/api/v1/agents/{id}/share-links/{share_id}", tag="agents", params(("id"=String,Path),("share_id"=String,Path),("expected_revision"=u64,Query),("Idempotency-Key"=String,Header)),responses((status=200,body=super::resource_revocations::RevocationAwareSuccess<ShareLinkView>),(status=409,body=super::ProblemDocument,content_type="application/problem+json")))]
pub(super) async fn revoke(
    revocations: Option<super::resource_revocations::Revocations>,
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path((id, share_id)): Path<(String, String)>,
    key: IdempotencyKey,
    Query(query): Query<RevokeQuery>,
) -> Result<Response, ResourceProblem> {
    let key = key.into_required_value().map_err(|_| invalid())?;
    let selector = ResourceSelector::Agent {
        agent_id: AgentId(id),
    };
    let share = service
        .read_share(&caller.context, credential(&caller), &selector, &share_id)
        .await?;
    if share.link.is_none() {
        return Err(invalid());
    }
    let command = ShareCommand {
        selector,
        change: ShareChange::Revoke { share_id },
        expected_share_revision: Some(query.expected_revision),
        idempotency_key: key,
    };
    if let Some(axum::Extension(revocations)) = revocations {
        return Ok(
            match revocations
                .request(&caller.context, credential(&caller), command)
                .await
            {
                Ok(value) => match value.receipt.clone() {
                    Some(receipt) => super::resource_revocations::completed(
                        &value,
                        view(&service, receipt.share).await?,
                    ),
                    None => super::resource_revocations::pending(&value),
                },
                Err(error) => super::resource_revocations::problem(error),
            },
        );
    }
    let receipt = service
        .change_share(&caller.context, credential(&caller), command)
        .await?;
    Ok(Json(view(&service, receipt.share).await?).into_response())
}
#[utoipa::path(post,path="/api/v1/agent-share-links/{share_id}/redeem",tag="agents",params(("share_id"=String,Path)),request_body=RedeemLinkBody,responses((status=200,body=ShareLinkView)))]
pub(super) async fn redeem(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path(share_id): Path<String>,
    Json(body): Json<RedeemLinkBody>,
) -> Result<Json<ShareLinkView>, ResourceProblem> {
    let share = service
        .redeem_agent_share_link(&caller.context, credential(&caller), &share_id, &body.token)
        .await?;
    Ok(Json(view(&service, share).await?))
}
#[utoipa::path(get,path="/api/v1/agents/{id}/share-recipients",tag="agents",params(("id"=String,Path),("q"=String,Query)),responses((status=200,body=RecipientList)))]
pub(super) async fn recipients(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    Query(query): Query<RecipientQuery>,
) -> Result<Json<RecipientList>, ResourceProblem> {
    Ok(Json(RecipientList {
        items: service
            .share_recipients(&caller.context, credential(&caller), &AgentId(id), &query.q)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    }))
}
