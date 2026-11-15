//! Authenticate an inert process before any ordinary composition can run.
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"cma-runtime-release-inert-bootstrap-v1\0";
const MAX_GRANT_BYTES: usize = 16 * 1024;
const MAX_VALIDITY_SECONDS: u64 = 300;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessIdentity {
    pub deployment: String,
    pub instance_id: String,
    pub config_sha256: String,
    pub binary_image: String,
    pub executable_sha256: String,
    pub agent_image: String,
    pub machine_image: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BootstrapGrant {
    pub version: u16,
    pub operation_id: String,
    pub operator_subject: String,
    pub process: ProcessIdentity,
    pub issued_at_unix: u64,
    pub expires_at_unix: u64,
    pub scope: BootstrapScope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BootstrapScope {
    InertOnly,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedGrant {
    grant: BootstrapGrant,
    signature: String,
}

/// Not deserializable. An approval identifier or supplied checksum cannot
/// construct authenticated bootstrap authority or a normal writer grant.
pub(super) struct VerifiedBootstrap {
    grant: BootstrapGrant,
    sha256: String,
}

impl VerifiedBootstrap {
    pub(super) fn process(&self) -> &ProcessIdentity {
        &self.grant.process
    }

    pub(super) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(super) fn expires_at_unix(&self) -> u64 {
        self.grant.expires_at_unix
    }
}

pub(super) fn verify(
    bytes: &[u8],
    trusted_public_key: &[u8; 32],
    expected: &ProcessIdentity,
    now_unix: u64,
) -> Result<VerifiedBootstrap, String> {
    if bytes.is_empty() || bytes.len() > MAX_GRANT_BYTES {
        return Err("runtime release bootstrap exceeds its bound".into());
    }
    let signed: SignedGrant = serde_json::from_slice(bytes)
        .map_err(|_| "runtime release bootstrap envelope is malformed")?;
    let grant = &signed.grant;
    if grant.version != 1
        || &grant.process != expected
        || !identifier(&grant.operation_id)
        || !identifier(&grant.operator_subject)
        || !identifier(&expected.deployment)
        || uuid::Uuid::parse_str(&expected.instance_id).is_err()
        || !digest(&expected.config_sha256)
        || !digest(&expected.executable_sha256)
        || !image(&expected.binary_image)
        || !image(&expected.agent_image)
        || expected
            .machine_image
            .as_ref()
            .is_some_and(|value| !image(value))
        || grant.issued_at_unix > now_unix
        || grant.expires_at_unix <= now_unix
        || grant
            .expires_at_unix
            .checked_sub(grant.issued_at_unix)
            .is_none_or(|seconds| seconds > MAX_VALIDITY_SECONDS)
    {
        return Err("runtime release bootstrap authority does not match this process".into());
    }
    let signature = STANDARD
        .decode(&signed.signature)
        .map_err(|_| "runtime release bootstrap signature is malformed")?;
    let message = message(grant)?;
    UnparsedPublicKey::new(&ED25519, trusted_public_key)
        .verify(&message, &signature)
        .map_err(|_| "runtime release bootstrap signature is invalid")?;
    Ok(VerifiedBootstrap {
        grant: signed.grant,
        sha256: format!("{:x}", Sha256::digest(message)),
    })
}

fn message(grant: &BootstrapGrant) -> Result<Vec<u8>, String> {
    let payload =
        serde_json::to_vec(grant).map_err(|_| "runtime release bootstrap cannot be encoded")?;
    let mut message = DOMAIN.to_vec();
    message.extend(payload);
    Ok(message)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn image(value: &str) -> bool {
    value.rsplit_once("@sha256:").is_some_and(|(name, hash)| {
        value.len() <= 2048
            && !name.is_empty()
            && !name.contains('@')
            && name.contains('/')
            && !name.chars().any(char::is_whitespace)
            && digest(hash)
    })
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
