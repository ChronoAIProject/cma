//! Authenticated encryption for durable authentication secrets.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::StoreError;
use mongodb::bson::{spec::BinarySubtype, Binary};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const KEY_LEN: usize = 32;
const ENVELOPE_VERSION: i32 = 1;
const AAD_DOMAIN: &[u8] = b"chrono-auth-aead-v1";

/// The deployment key used only for durable authentication state.
///
/// It is deliberately distinct from provider, sandbox, and storage credentials:
/// those values rotate in different trust domains and must not decide whether
/// state written by this service remains decryptable.
#[derive(Clone)]
pub struct AuthEncryptionKey(Zeroizing<[u8; KEY_LEN]>);

impl AuthEncryptionKey {
    /// Parse the exact standard-base64 form accepted from configuration.
    pub fn from_base64(encoded: &str) -> Result<Self, String> {
        let decoded = Zeroizing::new(
            STANDARD
                .decode(encoded.trim())
                .map_err(|_| "CHRONO_AUTH_ENCRYPTION_KEY must be valid base64".to_string())?,
        );
        if decoded.len() != KEY_LEN {
            return Err(format!(
                "CHRONO_AUTH_ENCRYPTION_KEY must decode to {KEY_LEN} bytes, got {}",
                decoded.len()
            ));
        }
        let mut key = Zeroizing::new([0_u8; KEY_LEN]);
        key.copy_from_slice(&decoded);
        Ok(Self(key))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }

    pub(crate) fn metrics_cursor_key(&self, deployment: &str) -> Zeroizing<Vec<u8>> {
        let root = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.as_bytes());
        let mut context = ring::hmac::Context::with_key(&root);
        context.update(b"chrono.usage.metrics.cursor.v1\0");
        context.update(deployment.as_bytes());
        Zeroizing::new(context.sign().as_ref().to_vec())
    }

    pub(crate) fn usage_cursor_key(&self, deployment: &str) -> Zeroizing<[u8; 32]> {
        let root = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.as_bytes());
        let mut context = ring::hmac::Context::with_key(&root);
        context.update(b"chrono.usage.continuation.v1\0");
        context.update(deployment.as_bytes());
        let mut key = Zeroizing::new([0; 32]);
        key.copy_from_slice(context.sign().as_ref());
        key
    }

    /// Derive a separate signing key without exposing the encryption key bytes.
    pub(super) fn agent_access_signing_key(&self, namespace: &str) -> ring::hmac::Key {
        let root = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.as_bytes());
        let mut context = ring::hmac::Context::with_key(&root);
        context.update(b"chrono.agent.access.signing-key.v1\0");
        context.update(namespace.as_bytes());
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, context.sign().as_ref())
    }

    pub(super) fn provider_tool_signing_key(&self, namespace: &str) -> ring::hmac::Key {
        let root = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.as_bytes());
        let mut context = ring::hmac::Context::with_key(&root);
        context.update(b"chrono.provider.tool.relay.signing-key.v1\0");
        context.update(namespace.as_bytes());
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, context.sign().as_ref())
    }

    pub(crate) fn agent_manager_signing_key(&self, namespace: &str) -> ring::hmac::Key {
        let root = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.as_bytes());
        let mut context = ring::hmac::Context::with_key(&root);
        context.update(b"chrono.agent.manager.relay.v1\0");
        context.update(namespace.as_bytes());
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, context.sign().as_ref())
    }

    pub(crate) fn binding_diagnostics_key(&self, namespace: &str) -> ring::hmac::Key {
        let root = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.as_bytes());
        let mut context = ring::hmac::Context::with_key(&root);
        context.update(b"chrono.binding.diagnostics.v1\0");
        context.update(namespace.as_bytes());
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, context.sign().as_ref())
    }

    #[cfg(test)]
    fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl std::fmt::Debug for AuthEncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthEncryptionKey(<redacted>)")
    }
}

/// One encrypted field as it is stored in BSON.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CipherEnvelope {
    version: i32,
    nonce: Binary,
    ciphertext: Binary,
}

impl std::fmt::Debug for CipherEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CipherEnvelope")
            .field("version", &self.version)
            .field("nonce_bytes", &self.nonce.bytes.len())
            .field("ciphertext_bytes", &self.ciphertext.bytes.len())
            .finish()
    }
}

/// AES-256-GCM bound to this deployment's durable record namespace.
pub(crate) struct AuthCipher {
    key: LessSafeKey,
    random: SystemRandom,
    deployment: String,
}

impl AuthCipher {
    pub(crate) fn new(
        deployment: impl Into<String>,
        key: &AuthEncryptionKey,
    ) -> Result<Self, StoreError> {
        let unbound = UnboundKey::new(&aead::AES_256_GCM, key.as_bytes()).map_err(|_| {
            StoreError::Malformed("authentication encryption key is not usable".into())
        })?;
        Ok(Self {
            key: LessSafeKey::new(unbound),
            random: SystemRandom::new(),
            deployment: deployment.into(),
        })
    }

    pub(crate) fn seal(
        &self,
        record_kind: &str,
        record_id: &[u8],
        field: &str,
        plaintext: &str,
    ) -> Result<CipherEnvelope, StoreError> {
        let mut nonce_bytes = [0_u8; aead::NONCE_LEN];
        self.random.fill(&mut nonce_bytes).map_err(|_| {
            StoreError::Unavailable(
                "encrypt authentication state: secure randomness unavailable".into(),
            )
        })?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aad = self.aad(record_kind, record_id, field);
        let mut ciphertext = Zeroizing::new(plaintext.as_bytes().to_vec());
        self.key
            .seal_in_place_append_tag(nonce, Aad::from(aad), &mut *ciphertext)
            .map_err(|_| {
                StoreError::Unavailable("encrypt authentication state: encryption failed".into())
            })?;
        Ok(CipherEnvelope {
            version: ENVELOPE_VERSION,
            nonce: binary(nonce_bytes.to_vec()),
            ciphertext: binary(ciphertext.to_vec()),
        })
    }

    pub(crate) fn open(
        &self,
        record_kind: &str,
        record_id: &[u8],
        field: &str,
        envelope: CipherEnvelope,
    ) -> Result<String, StoreError> {
        if envelope.version != ENVELOPE_VERSION {
            return Err(StoreError::Malformed(format!(
                "authentication ciphertext has unsupported version {}",
                envelope.version
            )));
        }
        if envelope.nonce.subtype != BinarySubtype::Generic {
            return Err(StoreError::Malformed(
                "authentication ciphertext nonce has an unsupported BSON subtype".into(),
            ));
        }
        if envelope.ciphertext.subtype != BinarySubtype::Generic {
            return Err(StoreError::Malformed(
                "authentication ciphertext has an unsupported BSON subtype".into(),
            ));
        }
        let nonce_bytes: [u8; aead::NONCE_LEN] =
            envelope.nonce.bytes.try_into().map_err(|bytes: Vec<u8>| {
                StoreError::Malformed(format!(
                    "authentication ciphertext nonce has {} bytes instead of {}",
                    bytes.len(),
                    aead::NONCE_LEN
                ))
            })?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aad = self.aad(record_kind, record_id, field);
        let mut plaintext = Zeroizing::new(envelope.ciphertext.bytes);
        let opened = self
            .key
            .open_in_place(nonce, Aad::from(aad), &mut plaintext)
            .map_err(|_| {
                StoreError::Malformed(
                    "authentication ciphertext failed integrity validation".into(),
                )
            })?;
        let value = std::str::from_utf8(opened)
            .map_err(|_| {
                StoreError::Malformed("authentication ciphertext is not valid UTF-8".into())
            })?
            .to_string();
        plaintext.zeroize();
        Ok(value)
    }

    fn aad(&self, record_kind: &str, record_id: &[u8], field: &str) -> Vec<u8> {
        let parts: [&[u8]; 5] = [
            AAD_DOMAIN,
            self.deployment.as_bytes(),
            record_kind.as_bytes(),
            record_id,
            field.as_bytes(),
        ];
        let mut aad = Vec::with_capacity(parts.iter().map(|part| part.len() + 8).sum());
        for part in parts {
            aad.extend_from_slice(&(part.len() as u64).to_be_bytes());
            aad.extend_from_slice(part);
        }
        aad
    }
}

fn binary(bytes: Vec<u8>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }
}

#[cfg(test)]
mod restore_evidence_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_diagnostics_signatures_are_isolated_from_agent_relay_and_other_deployments() {
        let root = AuthEncryptionKey::from_bytes([7; KEY_LEN]);
        let signature = ring::hmac::sign(&root.binding_diagnostics_key("deployment-a"), b"sample");
        assert!(ring::hmac::verify(
            &root.binding_diagnostics_key("deployment-a"),
            b"sample",
            signature.as_ref()
        )
        .is_ok());
        for key in [
            root.agent_manager_signing_key("deployment-a"),
            root.binding_diagnostics_key("deployment-b"),
        ] {
            assert!(ring::hmac::verify(&key, b"sample", signature.as_ref()).is_err());
        }
    }

    #[test]
    fn usage_cursor_keys_are_stable_and_isolated_from_metrics_deployments_and_roots() {
        let root = AuthEncryptionKey::from_bytes([7; 32]);
        let key = root.usage_cursor_key("deployment-a");
        assert_eq!(key, root.clone().usage_cursor_key("deployment-a"));
        assert_ne!(
            key.as_slice(),
            root.metrics_cursor_key("deployment-a").as_slice()
        );
        assert_ne!(key, root.usage_cursor_key("deployment-b"));
        assert_ne!(
            key,
            AuthEncryptionKey::from_bytes([8; 32]).usage_cursor_key("deployment-a")
        );
    }

    fn test_cipher(key: u8, deployment: &str) -> AuthCipher {
        AuthCipher::new(deployment, &AuthEncryptionKey::from_bytes([key; KEY_LEN])).unwrap()
    }

    #[test]
    fn configured_keys_require_exactly_32_base64_decoded_bytes() {
        let encoded = STANDARD.encode([7_u8; KEY_LEN]);
        let key = AuthEncryptionKey::from_base64(&encoded).expect("valid key");
        assert_eq!(key.as_bytes(), &[7_u8; KEY_LEN]);

        for invalid in ["not base64!", "", &STANDARD.encode([7_u8; 31])] {
            assert!(AuthEncryptionKey::from_base64(invalid).is_err());
        }
    }

    #[test]
    fn key_and_envelope_diagnostics_never_reveal_secret_material() {
        let key = AuthEncryptionKey::from_bytes([42; KEY_LEN]);
        let envelope = test_cipher(42, "deployment")
            .seal("subject", b"subject-a", "access_token", "visible-secret")
            .unwrap();
        let diagnostics = format!("{key:?} {envelope:?}");
        assert!(diagnostics.contains("redacted"));
        assert!(!diagnostics.contains("visible-secret"));
    }

    #[test]
    fn ciphertext_round_trips_only_in_its_original_context() {
        let cipher = test_cipher(1, "deployment-a");
        let envelope = cipher
            .seal("subject", b"subject-a", "access_token", "token-a")
            .unwrap();
        assert_eq!(
            cipher
                .open("subject", b"subject-a", "access_token", envelope.clone())
                .unwrap(),
            "token-a"
        );

        assert!(cipher
            .open("subject", b"subject-a", "refresh_token", envelope.clone())
            .is_err());
        assert!(cipher
            .open("subject", b"subject-b", "access_token", envelope.clone())
            .is_err());
        assert!(test_cipher(1, "deployment-b")
            .open("subject", b"subject-a", "access_token", envelope)
            .is_err());
    }

    #[test]
    fn wrong_keys_and_corrupted_ciphertexts_fail_closed() {
        let cipher = test_cipher(2, "deployment");
        let mut envelope = cipher
            .seal("login", b"state-id", "verifier", "pkce-value")
            .unwrap();
        assert!(test_cipher(3, "deployment")
            .open("login", b"state-id", "verifier", envelope.clone())
            .is_err());

        let last = envelope.ciphertext.bytes.last_mut().unwrap();
        *last ^= 1;
        assert!(cipher
            .open("login", b"state-id", "verifier", envelope)
            .is_err());
    }
}
