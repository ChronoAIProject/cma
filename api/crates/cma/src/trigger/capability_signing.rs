//! Ed25519 signing for short-lived, launch-scoped runtime policy data.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use layer2_core::{
    CapabilityOperation, CapabilityRevocationState, LaunchCapability, RepositoryIdentity,
    RepositoryOperationClass, CAPABILITY_POLICY_VERSION,
};
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde::Serialize;

use crate::config::GithubCapabilitySigningConfig;

pub(crate) const POLICY_VERSION: u32 = CAPABILITY_POLICY_VERSION;
pub(crate) const POLICY_AUDIENCE: &str = "cma-layer4-github-capability-v1";
const SIGNING_DOMAIN: &[u8] = b"CMA_GITHUB_CAPABILITY_V1\0";
const MAX_KEYS: usize = 8;

pub(crate) struct CapabilitySigner {
    key_id: String,
    key_pair: Arc<Ed25519KeyPair>,
    verification_keys_json: String,
}

impl CapabilitySigner {
    pub(crate) fn from_config(config: &GithubCapabilitySigningConfig) -> Result<Self, String> {
        if !usable_identifier(&config.key_id, 128) {
            return Err("the GitHub capability signing key ID is not usable".into());
        }
        let private_key = base64::engine::general_purpose::STANDARD
            .decode(&config.private_key_pkcs8_base64)
            .map_err(|_| "the GitHub capability signing key is not standard base64")?;
        let key_pair = Ed25519KeyPair::from_pkcs8(&private_key)
            .map_err(|_| "the GitHub capability signing key is not Ed25519 PKCS#8")?;

        let mut verification_keys: BTreeMap<String, String> =
            serde_json::from_str(&config.previous_verification_keys_json).map_err(|_| {
                "CHRONO_GITHUB_CAPABILITY_PREVIOUS_VERIFY_KEYS must be a JSON string map"
            })?;
        if verification_keys.len() >= MAX_KEYS {
            return Err(format!(
                "at most {} previous GitHub capability verification keys are accepted",
                MAX_KEYS - 1
            ));
        }
        for (key_id, encoded) in &verification_keys {
            if !usable_identifier(key_id, 128) {
                return Err("a previous GitHub capability key ID is not usable".into());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "a previous GitHub capability public key is not base64")?;
            if bytes.len() != 32 {
                return Err(
                    "a previous Ed25519 GitHub capability public key must contain 32 bytes".into(),
                );
            }
        }
        if verification_keys.contains_key(&config.key_id) {
            return Err(
                "the current GitHub capability key ID is duplicated in previous keys".into(),
            );
        }
        verification_keys.insert(
            config.key_id.clone(),
            base64::engine::general_purpose::STANDARD.encode(key_pair.public_key().as_ref()),
        );
        let verification_keys_json = serde_json::to_string(&verification_keys)
            .map_err(|_| "the GitHub capability keyring could not be rendered")?;
        Ok(Self {
            key_id: config.key_id.clone(),
            key_pair: Arc::new(key_pair),
            verification_keys_json,
        })
    }

    pub(crate) fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Public keyring passed to every runtime image. It carries no signing
    /// authority and is safe to expose to the sandboxed process.
    pub(crate) fn verification_keys_json(&self) -> &str {
        &self.verification_keys_json
    }

    pub(crate) fn sign_policy(&self, capability: &LaunchCapability) -> Result<String, String> {
        capability
            .validate()
            .map_err(|error| format!("runtime policy is invalid: {error}"))?;
        if capability.signature_key_id != self.key_id {
            return Err("runtime policy names a different signing key".into());
        }
        if capability.audience != POLICY_AUDIENCE {
            return Err("runtime policy names an unsupported audience".into());
        }
        if capability.revocation != CapabilityRevocationState::Active {
            return Err("an active policy cannot carry a revoked state".into());
        }
        let operations = capability
            .operations
            .iter()
            .map(|operation| match operation {
                CapabilityOperation::Repository(operation) => (*operation).into(),
            })
            .collect();
        self.sign(&CapabilityDocument::Policy {
            version: POLICY_VERSION,
            launch_id: capability.launch_id.0.clone(),
            session_id: capability.session_id.0.clone(),
            runtime_audience: capability.runtime_audience.clone(),
            service_id: capability.service_id.clone(),
            upstream: RepositoryBinding::from(&capability.upstream),
            fork: RepositoryBinding::from(&capability.fork),
            allowed_ref: capability.allowed_ref.clone(),
            github_operation_classes: operations,
            generation: capability.generation,
            fence: capability.fence,
            not_before_epoch_seconds: capability.not_before_epoch_seconds,
            expires_at_epoch_seconds: capability.expires_at_epoch_seconds,
            issuer: capability.issuer.clone(),
            audience: capability.audience.clone(),
            signature_key_id: capability.signature_key_id.clone(),
            numeric_binding_observed_at: capability.numeric_binding_observed_at.clone(),
        })
    }

    pub(crate) fn sign_tombstone(
        &self,
        tombstone: RuntimePolicyTombstone,
    ) -> Result<String, String> {
        if tombstone.version != POLICY_VERSION
            || tombstone.generation == 0
            || tombstone.fence == 0
            || tombstone.signature_key_id != self.key_id
            || tombstone.audience != POLICY_AUDIENCE
            || !usable_identifier(&tombstone.launch_id, 128)
            || !usable_identifier(&tombstone.session_id, 256)
            || !usable_identifier(&tombstone.runtime_audience, 256)
            || !usable_identifier(&tombstone.issuer, 256)
        {
            return Err("runtime policy tombstone is invalid".into());
        }
        self.sign(&CapabilityDocument::Tombstone {
            version: tombstone.version,
            launch_id: tombstone.launch_id,
            session_id: tombstone.session_id,
            runtime_audience: tombstone.runtime_audience,
            generation: tombstone.generation,
            fence: tombstone.fence,
            issued_at_epoch_seconds: tombstone.issued_at_epoch_seconds,
            issuer: tombstone.issuer,
            audience: tombstone.audience,
            signature_key_id: tombstone.signature_key_id,
        })
    }

    fn sign(&self, document: &CapabilityDocument) -> Result<String, String> {
        let payload = serde_json::to_vec(document)
            .map_err(|_| "runtime policy payload could not be rendered")?;
        let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + payload.len());
        message.extend_from_slice(SIGNING_DOMAIN);
        message.extend_from_slice(&payload);
        let envelope = SignedEnvelope {
            version: POLICY_VERSION,
            key_id: self.key_id.clone(),
            payload: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload),
            signature: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(self.key_pair.sign(&message).as_ref()),
        };
        serde_json::to_string(&envelope)
            .map_err(|_| "runtime policy envelope could not be rendered".into())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimePolicyTombstone {
    pub version: u32,
    pub launch_id: String,
    pub session_id: String,
    pub runtime_audience: String,
    pub generation: u64,
    pub fence: u64,
    pub issued_at_epoch_seconds: u64,
    pub issuer: String,
    pub audience: String,
    pub signature_key_id: String,
}

#[derive(Serialize)]
struct SignedEnvelope {
    version: u32,
    key_id: String,
    payload: String,
    signature: String,
}

#[derive(Serialize)]
struct RepositoryBinding {
    repository_id: u64,
    repository_node_id: String,
    owner_id: u64,
    owner_node_id: String,
    canonical_owner: String,
    canonical_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fork_parent_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fork_source_id: Option<u64>,
}

impl From<&RepositoryIdentity> for RepositoryBinding {
    fn from(value: &RepositoryIdentity) -> Self {
        Self {
            repository_id: value.repository_id,
            repository_node_id: value.repository_node_id.clone(),
            owner_id: value.owner_id,
            owner_node_id: value.owner_node_id.clone(),
            canonical_owner: value.canonical_owner.clone(),
            canonical_name: value.canonical_name.clone(),
            fork_parent_id: value.fork_parent_id,
            fork_source_id: value.fork_source_id,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CapabilityDocument {
    Policy {
        version: u32,
        launch_id: String,
        session_id: String,
        runtime_audience: String,
        service_id: String,
        upstream: RepositoryBinding,
        fork: RepositoryBinding,
        allowed_ref: String,
        github_operation_classes: Vec<GithubOperationClass>,
        generation: u64,
        fence: u64,
        not_before_epoch_seconds: u64,
        expires_at_epoch_seconds: u64,
        issuer: String,
        audience: String,
        signature_key_id: String,
        numeric_binding_observed_at: String,
    },
    Tombstone {
        version: u32,
        launch_id: String,
        session_id: String,
        runtime_audience: String,
        generation: u64,
        fence: u64,
        issued_at_epoch_seconds: u64,
        issuer: String,
        audience: String,
        signature_key_id: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum GithubOperationClass {
    RepositoryRead,
    TreeRead,
    ObjectCreate,
    LaunchRefCreate,
    LaunchRefUpdate,
    StatusRead,
}

impl From<RepositoryOperationClass> for GithubOperationClass {
    fn from(value: RepositoryOperationClass) -> Self {
        match value {
            RepositoryOperationClass::RepositoryRead => Self::RepositoryRead,
            RepositoryOperationClass::TreeRead => Self::TreeRead,
            RepositoryOperationClass::ObjectCreate => Self::ObjectCreate,
            RepositoryOperationClass::LaunchRefCreate => Self::LaunchRefCreate,
            RepositoryOperationClass::LaunchRefUpdate => Self::LaunchRefUpdate,
            RepositoryOperationClass::StatusRead => Self::StatusRead,
        }
    }
}

fn usable_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{LaunchId, RepositoryVisibility, SessionId};
    use ring::rand::SystemRandom;
    use ring::signature;

    fn config() -> GithubCapabilitySigningConfig {
        let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        GithubCapabilitySigningConfig {
            key_id: "key-current".into(),
            private_key_pkcs8_base64: base64::engine::general_purpose::STANDARD
                .encode(key.as_ref()),
            previous_verification_keys_json: "{}".into(),
        }
    }

    #[test]
    fn signer_derives_only_a_public_runtime_keyring() {
        let signer = CapabilitySigner::from_config(&config()).unwrap();
        let keys: BTreeMap<String, String> =
            serde_json::from_str(signer.verification_keys_json()).unwrap();
        let public = base64::engine::general_purpose::STANDARD
            .decode(&keys["key-current"])
            .unwrap();
        assert_eq!(public.len(), 32);
        assert!(!signer.verification_keys_json().contains("PRIVATE"));
    }

    #[test]
    fn malformed_or_duplicated_rotation_configuration_is_rejected() {
        let mut invalid = config();
        invalid.private_key_pkcs8_base64 = "not-base64".into();
        assert!(CapabilitySigner::from_config(&invalid).is_err());

        let mut duplicated = config();
        duplicated.previous_verification_keys_json = serde_json::json!({
            "key-current": base64::engine::general_purpose::STANDARD.encode([0_u8; 32])
        })
        .to_string();
        assert!(CapabilitySigner::from_config(&duplicated).is_err());
    }

    #[test]
    fn policy_wire_is_closed_and_verifiable_by_the_published_key() {
        let signer = CapabilitySigner::from_config(&config()).unwrap();
        let capability = LaunchCapability {
            version: POLICY_VERSION,
            launch_id: LaunchId("launch-wire".into()),
            session_id: SessionId("session-wire".into()),
            runtime_audience: "runtime-wire".into(),
            service_id: "service-wire".into(),
            upstream: repository(10, 20, "upstream", None, None),
            fork: repository(30, 40, "contributor", Some(10), Some(10)),
            allowed_ref: "refs/heads/cma/launch-wire".into(),
            operations: vec![
                CapabilityOperation::Repository(RepositoryOperationClass::RepositoryRead),
                CapabilityOperation::Repository(RepositoryOperationClass::ObjectCreate),
            ],
            generation: 3,
            fence: 7,
            not_before_epoch_seconds: 1_000,
            expires_at_epoch_seconds: 1_060,
            issuer: "cma-trigger".into(),
            audience: POLICY_AUDIENCE.into(),
            signature_key_id: signer.key_id().into(),
            numeric_binding_observed_at: "2026-08-27T00:00:00Z".into(),
            revocation: CapabilityRevocationState::Active,
        };

        let envelope: serde_json::Value =
            serde_json::from_str(&signer.sign_policy(&capability).unwrap()).unwrap();
        assert_eq!(envelope["version"], POLICY_VERSION);
        assert_eq!(envelope["key_id"], signer.key_id());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(envelope["payload"].as_str().unwrap())
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(
            document,
            serde_json::json!({
                "kind": "policy",
                "version": 1,
                "launch_id": "launch-wire",
                "session_id": "session-wire",
                "runtime_audience": "runtime-wire",
                "service_id": "service-wire",
                "upstream": {
                    "repository_id": 10,
                    "repository_node_id": "R_10",
                    "owner_id": 20,
                    "owner_node_id": "U_20",
                    "canonical_owner": "upstream",
                    "canonical_name": "project",
                },
                "fork": {
                    "repository_id": 30,
                    "repository_node_id": "R_30",
                    "owner_id": 40,
                    "owner_node_id": "U_40",
                    "canonical_owner": "contributor",
                    "canonical_name": "project",
                    "fork_parent_id": 10,
                    "fork_source_id": 10,
                },
                "allowed_ref": "refs/heads/cma/launch-wire",
                "github_operation_classes": ["repository_read", "object_create"],
                "generation": 3,
                "fence": 7,
                "not_before_epoch_seconds": 1000,
                "expires_at_epoch_seconds": 1060,
                "issuer": "cma-trigger",
                "audience": "cma-layer4-github-capability-v1",
                "signature_key_id": "key-current",
                "numeric_binding_observed_at": "2026-08-27T00:00:00Z",
            })
        );
        let keys: BTreeMap<String, String> =
            serde_json::from_str(signer.verification_keys_json()).unwrap();
        let public = base64::engine::general_purpose::STANDARD
            .decode(&keys[signer.key_id()])
            .unwrap();
        let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(envelope["signature"].as_str().unwrap())
            .unwrap();
        let mut signed = SIGNING_DOMAIN.to_vec();
        signed.extend_from_slice(&payload);
        signature::UnparsedPublicKey::new(&signature::ED25519, public)
            .verify(&signed, &signature_bytes)
            .unwrap();
    }

    fn repository(
        repository_id: u64,
        owner_id: u64,
        owner: &str,
        fork_parent_id: Option<u64>,
        fork_source_id: Option<u64>,
    ) -> RepositoryIdentity {
        RepositoryIdentity {
            repository_id,
            repository_node_id: format!("R_{repository_id}"),
            owner_id,
            owner_node_id: format!("U_{owner_id}"),
            canonical_owner: owner.into(),
            canonical_name: "project".into(),
            visibility: RepositoryVisibility::Public,
            allow_forking: true,
            default_branch: "main".into(),
            selected_ref: "main".into(),
            fork_parent_id,
            fork_source_id,
        }
    }
}
