//! Opaque equality metadata for independently validated authority, never a cache.
use ring::hmac;
use serde::Serialize;

use super::{
    identity::VerifiedSubject, identity_flight::TokenHash, permissions::PermissionEvidence,
    verification_error::ProviderVerificationError as Error, AuthMode, AuthState, ProviderMetadata,
};

// Intentionally neither Debug nor Display: stamps are never diagnostic fields.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct EvidenceStamp([u8; 32]);

/// Where evidence was validated, not why it was requested.
///
/// A retained-owner verification differs from an independent one only by an
/// extra requirement that the provider's subject equal the retained owner. The
/// verified subject is part of every stamp, so equivalent observations are the
/// same authority whichever purpose requested them. Keeping the purpose here
/// made an independent verification of a browser's credential (for example
/// the organization principal) fence every overlapping cookie verification of
/// it, refusing ordinary reads with `store_failed` (#2709).
#[derive(Serialize)]
pub(super) struct EvidenceScope<'a> {
    provider: Option<(&'a str, &'a str)>,
    resource: Option<&'a str>,
    agent_service: Option<&'a str>,
    agent_key: bool,
    endpoints: (&'a str, &'a str, Option<&'a str>, Option<&'a str>),
}

impl<'a> EvidenceScope<'a> {
    pub(super) fn new(auth: &'a AuthState, metadata: &'a ProviderMetadata, token: &str) -> Self {
        // Each AuthState constructs its own private store/key. Configuration is
        // fixed before sharing; changed discovery endpoints are included here
        // so a successful verification against another provider scope fences.
        let provider = match &auth.mode {
            AuthMode::Provider {
                base_url,
                client_id,
                ..
            } => Some((base_url.as_str(), client_id.as_str())),
            AuthMode::Trusted { .. } => None,
        };
        let ProviderMetadata {
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
            introspection_endpoint,
        } = metadata;
        Self {
            provider,
            resource: auth.oauth_api_resource.as_deref(),
            agent_service: auth.agent_key_service_id.as_deref(),
            agent_key: super::agent_keys::is_agent_key(token),
            endpoints: (
                authorization_endpoint,
                token_endpoint,
                userinfo_endpoint.as_deref(),
                introspection_endpoint.as_deref(),
            ),
        }
    }
}

pub(super) fn stamp(
    secret: &hmac::Key,
    key: &TokenHash,
    scope: EvidenceScope<'_>,
    evidence: &VerifiedSubject,
) -> Result<EvidenceStamp, Error> {
    // Exhaustive destructuring forces a decision when authority fields grow.
    // Profile is display-only; each consumer keeps its own unmodified profile.
    let VerifiedSubject {
        subject,
        permissions,
        profile: _,
        independent_commands,
        api_expires_at,
        token_expires_at,
        verification: _,
    } = evidence;
    let (active, mut names) = match permissions {
        PermissionEvidence::Active(names) => (true, names.iter().map(String::as_str).collect()),
        PermissionEvidence::Inactive => (false, Vec::new()),
    };
    names.sort_unstable();
    names.dedup();
    // Serialize exact optional SystemTimes (seconds and nanoseconds), not a
    // relative TTL. The payload is transient; only the full HMAC is retained.
    let payload = serde_json::to_vec(&(
        "cma-identity-authority-v1",
        key,
        scope,
        subject.as_str(),
        active,
        names,
        independent_commands,
        token_expires_at,
        api_expires_at,
    ))
    .map_err(|_| Error::Unavailable("identity authority stamp encoding failed".into()))?;
    Ok(EvidenceStamp(
        hmac::sign(secret, &payload)
            .as_ref()
            .try_into()
            .expect("SHA-256 length"),
    ))
}

#[cfg(test)]
#[path = "identity_evidence_tests.rs"]
pub(super) mod tests;
