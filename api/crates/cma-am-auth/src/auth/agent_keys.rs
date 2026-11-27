//! Ordinary agent keys require evidence bound to this resource server.
use super::{
    identity::VerifiedSubject,
    permissions,
    profile::Profile,
    verification_diagnostics::{configuration, Endpoint},
    verification_error::ProviderVerificationError,
    AuthMode, AuthState, ProviderMetadata,
};
use std::time::{Duration, SystemTime};

pub fn is_agent_key(value: &str) -> bool {
    value.starts_with("nyx_") || value.starts_with("nyxid_ag_")
}

impl AuthState {
    pub fn with_agent_key_service(
        mut self: std::sync::Arc<Self>,
        service_id: Option<String>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::get_mut(&mut self)
            .expect("agent key service is configured before sharing auth")
            .agent_key_service_id = service_id;
        self
    }
}

pub(super) async fn verify(
    auth: &AuthState,
    metadata: &ProviderMetadata,
    key: &str,
    deadline: tokio::time::Instant,
) -> Result<VerifiedSubject, ProviderVerificationError> {
    use ProviderVerificationError::{Rejected, Unavailable};
    let service = configuration(
        Endpoint::AgentKey,
        auth.agent_key_service_id
            .as_deref()
            .ok_or_else(|| Rejected("agent key service verification is not configured".into())),
    )?;
    let endpoint = configuration(
        Endpoint::AgentKey,
        metadata
            .introspection_endpoint
            .as_deref()
            .ok_or_else(|| Unavailable("agent keys require provider introspection".into())),
    )?;
    let AuthMode::Provider {
        client_id,
        client_secret,
        ..
    } = &auth.mode
    else {
        return configuration(
            Endpoint::AgentKey,
            Err(Rejected(
                "agent keys require provider authentication".into(),
            )),
        );
    };
    let request = auth.http.post(endpoint).form(&[
        ("token", key),
        ("token_type_hint", "api_key"),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("service_id", service),
    ]);
    let body = super::identity_http::json(
        request,
        deadline,
        super::verification_diagnostics::Endpoint::AgentKey,
    )
    .await?;
    super::verification_diagnostics::claims(
        super::verification_diagnostics::Endpoint::AgentKey,
        (|| {
            match body.get("active").and_then(serde_json::Value::as_bool) {
                Some(false) => {
                    return Err(Rejected("agent key is inactive for this service".into()))
                }
                Some(true) => {}
                None => return Err(Unavailable("missing agent key activity evidence".into())),
            }
            let subject = body
                .get("sub")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| Unavailable("missing agent key subject".into()))?;
            if body.get("aud").and_then(serde_json::Value::as_str) != Some(service)
                || body.get("token_type").and_then(serde_json::Value::as_str) != Some("api_key")
                || !body
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|scopes| scopes.split_whitespace().any(|scope| scope == "proxy"))
                || body.get("resource").is_some_and(|value| !value.is_null())
            {
                return Err(Rejected(
                    "agent key evidence is not bound to this service".into(),
                ));
            }
            // Absence means a provider-verified nonexpiring key. A malformed or past
            // expiry must never be interpreted as an unlimited lifetime.
            let expires_at = match body.get("exp") {
                None => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .and_then(|seconds| {
                            SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
                        })
                        .filter(|expiry| *expiry > SystemTime::now())
                        .ok_or_else(|| Rejected("invalid or expired agent key lifetime".into()))?,
                ),
            };
            Ok(VerifiedSubject {
                verification: None,
                subject: layer2_core::SubjectId::new(subject),
                permissions: permissions::PermissionEvidence::Active(
                    permissions::permissions_claim(&body).map_err(Unavailable)?,
                ),
                profile: Profile::default(),
                api_expires_at: expires_at,
                token_expires_at: expires_at,
                independent_commands: true,
            })
        })(),
    )
}
