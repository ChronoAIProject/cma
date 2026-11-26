//! Transport-only fixtures inject a policy; public_access tests exercise real policy.
use super::*;
use layer2_core::resource_access::{
    CallerContext, CredentialContext, CredentialSource, ResourceActor,
};

pub(super) struct TransportPolicy;
#[async_trait::async_trait]
impl public_access::PublicAuthorization for TransportPolicy {
    async fn authorize(
        &self,
        request: public_access::PublicRequest<'_>,
    ) -> Result<
        Option<layer2_core::sandbox::agent_filesystem::FileWriteTarget>,
        public_access::PublicRefusal,
    > {
        if request.context.caller.subject.as_str() == "synthetic-caller" {
            Ok(None)
        } else {
            Err(StatusCode::UNAUTHORIZED.into())
        }
    }
}

pub(super) async fn test_auth(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if request
        .headers()
        .get("authorization")
        .is_some_and(|value| value == "Bearer synthetic-authorized")
    {
        let subject = layer2_core::SubjectId::new("synthetic-caller");
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(subject.clone()));
        request.extensions_mut().insert(CallerContext {
            credential_source: CredentialSource::Retained,
            caller: layer2_core::Caller::new(
                subject.clone(),
                crate::auth::AuthState::trusted_authorization(),
            ),
            credential: CredentialContext {
                issuer: "fixture".into(),
                subject: subject.clone(),
                credential_id: "transport".into(),
            },
            actor: ResourceActor::Human { subject },
            memberships: Vec::new(),
        });
    }
    next.run(request).await
}
