//! Resolve the retained media parent before either peer can select an owner.
use super::*;
use layer1_public::rest::binding_media::{BindingMediaError, BindingMediaPath};

pub(in crate::agent_manager::relay) struct PublicMediaRequest<'a> {
    pub method: &'a Method,
    pub uri: &'a Uri,
    pub headers: &'a HeaderMap,
    pub context: &'a CallerContext,
    pub credential: Option<&'a str>,
    pub path: &'a BindingMediaPath,
}
pub(in crate::agent_manager::relay) fn media_mutation(method: &Method, uri: &Uri) -> bool {
    if method != Method::POST {
        return false;
    }
    let parts = uri
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    matches!(
        parts.as_slice(),
        [
            "api",
            "v1",
            "cmaeg",
            "bindings",
            _,
            "conversations",
            _,
            "events",
            _,
            "media",
            _,
            "attachment" | "supplement"
        ]
    )
}
impl PublicAccess {
    pub(super) async fn media_target(
        &self,
        request: PublicMediaRequest<'_>,
    ) -> Result<AgentId, StatusCode> {
        let media = self.media.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let credential = request.credential.ok_or(StatusCode::UNAUTHORIZED)?;
        self.auth
            .verify_owner_admission_origin(
                request.context,
                credential,
                request.method,
                request.uri,
                request.headers,
            )
            .await
            .map_err(auth_error)?;
        let caller = layer1_public::AuthenticatedResourceCaller {
            context: request.context.clone(),
            credential: Some(layer1_public::DelegatedCredential::new(
                credential.to_owned(),
            )),
        };
        let id = media
            .owner_target(&caller, request.path)
            .await
            .map_err(|error| match error {
                BindingMediaError::Invalid => StatusCode::BAD_REQUEST,
                BindingMediaError::Forbidden => StatusCode::FORBIDDEN,
                BindingMediaError::NotFound => StatusCode::NOT_FOUND,
                BindingMediaError::Conflict | BindingMediaError::NotReady => StatusCode::CONFLICT,
                BindingMediaError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
                BindingMediaError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            })?;
        self.auth
            .verify_owner_admission_origin(
                request.context,
                credential,
                request.method,
                request.uri,
                request.headers,
            )
            .await
            .map_err(auth_error)?;
        Ok(id)
    }
}
fn auth_error(error: layer2_core::sandbox::agent_io::CommandAuthorizationError) -> StatusCode {
    use layer2_core::sandbox::agent_io::CommandAuthorizationError;
    match error {
        CommandAuthorizationError::Revoked => StatusCode::FORBIDDEN,
        CommandAuthorizationError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}
