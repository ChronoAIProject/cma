//! Operator authentication and finality are separate authority boundaries.
use super::{files, settings::Settings};
use crate::{config::Config, release_authority::*};
use axum::http::{header, HeaderMap, HeaderValue};
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlGrant {
    version: u16,
    deployment: String,
    operation_id: String,
    operator_subject: String,
    authorization_sha256: String,
    action: EvidenceAction,
    request_sha256: String,
    state_sha256: String,
    epoch: i64,
    control_revision: i64,
    issued_at_unix: u64,
    expires_at_unix: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedControl {
    grant: ControlGrant,
    signature: String,
}

pub(super) struct LiveVerifier {
    proof: crate::auth::read_only::ReadProof,
    grant: ControlGrant,
    authorization_sha256: String,
}

impl LiveVerifier {
    pub(super) async fn open(
        config: &Config,
        cookie_path: &Path,
        signature_path: &Path,
        operator: &OperatorIdentity,
        expected_request: &impl Serialize,
    ) -> std::result::Result<Self, String> {
        let Settings::Managed {
            trusted_public_key, ..
        } = &config.runtime_release
        else {
            return Err("operator release writes require explicit managed authority".into());
        };
        let cookie = files::read_private(cookie_path, 16 * 1024)
            .map_err(|_| "operator credential must be a bounded private file")?;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_bytes(&cookie).map_err(|_| "operator cookie is malformed")?,
        );
        let proof = super::readonly::verify(config, &headers).await?;
        let authorization_sha256 = hash(&(
            "runtime-release-operator-session-v1",
            &config.deployment_id,
            &cookie,
        ))?;
        let encoded = files::read_private(signature_path, 16 * 1024)
            .map_err(|_| "operator control grant must be a bounded private file")?;
        let signed: SignedControl =
            serde_json::from_slice(&encoded).map_err(|_| "operator control grant is malformed")?;
        let mut message = b"cma-runtime-release-control-v1\0".to_vec();
        message.extend(
            serde_json::to_vec(&signed.grant).map_err(|_| "control grant encoding failed")?,
        );
        let signature = STANDARD
            .decode(&signed.signature)
            .map_err(|_| "control signature is malformed")?;
        UnparsedPublicKey::new(&ED25519, trusted_public_key)
            .verify(&message, &signature)
            .map_err(|_| "control signature is invalid")?;
        let caller = proof.caller();
        if !caller
            .authorization
            .allows(layer2_core::Capability::ConfigurePlatform)
            || operator.subject != caller.subject.as_str()
            || operator.authorization_sha256 != authorization_sha256
            || signed.grant.operator_subject != operator.subject
            || signed.grant.authorization_sha256 != authorization_sha256
            || signed.grant.deployment != config.deployment_id
            || signed.grant.request_sha256 != hash(expected_request)?
        {
            return Err("fresh operator authority does not match the exact release request".into());
        }
        valid_time(&signed.grant)?;
        proof
            .recheck()
            .await
            .map_err(|_| "fresh operator authority was revoked")?;
        Ok(Self {
            proof,
            grant: signed.grant,
            authorization_sha256,
        })
    }
}

#[async_trait::async_trait]
impl EvidenceVerifier for LiveVerifier {
    async fn verify(&self, request: &EvidenceRequest) -> Result<VerifiedEvidence> {
        self.proof
            .recheck()
            .await
            .map_err(|_| AuthorityError::EvidenceRequired)?;
        valid_time(&self.grant).map_err(|_| AuthorityError::EvidenceRequired)?;
        let grant = &self.grant;
        if grant.deployment != request.deployment
            || grant.operation_id != request.operation_id
            || grant.action != request.action
            || grant.request_sha256 != request.request_sha256
            || grant.state_sha256 != request.state_sha256
            || grant.epoch != request.epoch
            || grant.control_revision != request.control_revision
        {
            return Err(AuthorityError::EvidenceRequired);
        }
        // Closing admission needs fresh operator authority, not receiver
        // settlement. Every action that may select/reopen/release authority
        // requires a live custody collector, which is not implemented here.
        if !matches!(request.action, EvidenceAction::Begin | EvidenceAction::Hold) {
            return Err(AuthorityError::EvidenceRequired);
        }
        VerifiedEvidence::from_verified_observations(
            request,
            hash(&(
                "fresh-operator-closed-admission-v1",
                &self.authorization_sha256,
                grant,
            ))
            .map_err(|_| AuthorityError::Invalid)?,
        )
    }
}

fn valid_time(grant: &ControlGrant) -> std::result::Result<(), String> {
    let now = files::now()?;
    if grant.version != 1
        || grant.issued_at_unix > now
        || grant.expires_at_unix <= now
        || grant
            .expires_at_unix
            .checked_sub(grant.issued_at_unix)
            .is_none_or(|span| span > 300)
    {
        return Err("operator control authorization expired or exceeds its bound".into());
    }
    Ok(())
}

pub(super) fn hash(value: &impl Serialize) -> std::result::Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| "release request encoding failed".into())
}
