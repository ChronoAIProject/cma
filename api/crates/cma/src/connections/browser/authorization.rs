use axum::http::HeaderMap;
use layer1_public::AuthenticatedResourceCaller;
use layer2_core::{account_deletion::OwnerSessionAuthority, Capability};

use crate::auth::service_authorization;

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AuthorizationRequest {
    target: ConnectionTarget,
    return_to: String,
    purpose: Option<AuthorizationPurpose>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthorizationPurpose {
    SkillCatalog,
}

pub(super) async fn start(
    State(state): State<Arc<BrowserConnections>>,
    Extension(auth): Extension<Arc<AuthState>>,
    AuthenticatedResourceCaller { context, .. }: AuthenticatedResourceCaller,
    authority: Option<Extension<OwnerSessionAuthority>>,
    headers: HeaderMap,
    Json(request): Json<AuthorizationRequest>,
) -> Response {
    if let Err(error) =
        service_authorization::start_allowed(&auth, &headers, context.credential_source)
    {
        return error.into_response();
    }
    if authority.is_none_or(|Extension(authority)| authority.subject() != &context.caller.subject) {
        return service_authorization::RecoveryError::Forbidden.into_response();
    }
    if request.purpose.is_some()
        && (!matches!(&request.target, ConnectionTarget::Agent { .. })
            || !context
                .caller
                .authorization
                .allows(Capability::UseOwnSandboxes))
    {
        return service_authorization::RecoveryError::Forbidden.into_response();
    }
    let services = match state
        .targets
        .services(&context.caller, &request.target)
        .await
    {
        Ok(services) => services,
        Err(error) => return error.into_response(),
    };
    let services = match request.purpose {
        // The target above proves ownership of the Agent and its Sandbox.
        // Catalog consent is a browser grant for this single service.
        Some(AuthorizationPurpose::SkillCatalog) => BTreeSet::from([ServiceId::new("ornn-api")]),
        None => services
            .into_iter()
            .filter(|service| service.as_str() != CODEX_SUBSCRIPTION_CONNECTION)
            .collect(),
    };
    service_authorization::begin(
        &auth,
        &headers,
        &context.caller.subject,
        services,
        &request.return_to,
    )
    .await
    .into_response()
}
