//! Authenticated encryption for durable sponsorship authority.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::SponsorshipStoreError;
use mongodb::bson::{spec::BinarySubtype, Binary};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const KEY_LEN: usize = 32;
const ENVELOPE_VERSION: i32 = 1;
const AAD_DOMAIN: &[u8] = b"chrono-sponsorship-aead-v1";
const MAX_PREVIOUS_KEYS: usize = 4;

/// One deployment key used only for durable sponsorship provider authority.
#[derive(Clone)]
pub(crate) struct SponsorshipEncryptionKey(Zeroizing<[u8; KEY_LEN]>);

impl SponsorshipEncryptionKey {
    /// Parse the standard-base64 representation accepted by future deployment wiring.
    pub(crate) fn from_base64(encoded: &str) -> Result<Self, String> {
        let decoded = Zeroizing::new(STANDARD.decode(encoded.trim()).map_err(|_| {
            "a sponsorship encryption key must be valid standard base64".to_string()
        })?);
        if decoded.len() != KEY_LEN {
            return Err(format!(
                "a sponsorship encryption key must decode to {KEY_LEN} bytes, got {}",
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

    #[cfg(test)]
    fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl std::fmt::Debug for SponsorshipEncryptionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SponsorshipEncryptionKey(<redacted>)")
    }
}

/// Bounded current/previous key set used during an explicit sponsorship-key rotation.
#[derive(Clone)]
pub(crate) struct SponsorshipKeyring {
    current_key_id: String,
    keys: BTreeMap<String, SponsorshipEncryptionKey>,
}

impl SponsorshipKeyring {
    pub(crate) fn new(
        current_key_id: impl Into<String>,
        current: SponsorshipEncryptionKey,
        previous: impl IntoIterator<Item = (String, SponsorshipEncryptionKey)>,
    ) -> Result<Self, String> {
        let current_key_id = current_key_id.into();
        validate_key_id(&current_key_id)?;
        let previous = previous.into_iter().collect::<Vec<_>>();
        if previous.len() > MAX_PREVIOUS_KEYS {
            return Err(format!(
                "at most {MAX_PREVIOUS_KEYS} previous sponsorship encryption keys are supported"
            ));
        }
        let mut keys = BTreeMap::new();
        keys.insert(current_key_id.clone(), current);
        for (key_id, key) in previous {
            validate_key_id(&key_id)?;
            if keys.insert(key_id.clone(), key).is_some() {
                return Err(format!(
                    "sponsorship encryption key id '{key_id}' is configured more than once"
                ));
            }
        }
        Ok(Self {
            current_key_id,
            keys,
        })
    }

    pub(crate) fn current_key_id(&self) -> &str {
        &self.current_key_id
    }

    fn current(&self) -> &SponsorshipEncryptionKey {
        self.keys
            .get(&self.current_key_id)
            .expect("construction always inserts the current key")
    }

    fn key(&self, key_id: &str) -> Option<&SponsorshipEncryptionKey> {
        self.keys.get(key_id)
    }
}

impl std::fmt::Debug for SponsorshipKeyring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorshipKeyring")
            .field("current_key_id", &self.current_key_id)
            .field("key_count", &self.keys.len())
            .finish()
    }
}

/// One encrypted sponsorship field as stored in BSON.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SponsorshipCipherEnvelope {
    version: i32,
    key_id: String,
    nonce: Binary,
    ciphertext: Binary,
}

impl SponsorshipCipherEnvelope {
    pub(super) fn key_id(&self) -> &str {
        &self.key_id
    }
}

impl std::fmt::Debug for SponsorshipCipherEnvelope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorshipCipherEnvelope")
            .field("version", &self.version)
            .field("key_id", &self.key_id)
            .field("nonce_bytes", &self.nonce.bytes.len())
            .field("ciphertext_bytes", &self.ciphertext.bytes.len())
            .finish()
    }
}

pub(super) struct OpenedSponsorshipSecret {
    pub(super) plaintext: Zeroizing<String>,
    pub(super) needs_rewrap: bool,
}

impl std::fmt::Debug for OpenedSponsorshipSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedSponsorshipSecret")
            .field("plaintext", &"<redacted>")
            .field("needs_rewrap", &self.needs_rewrap)
            .finish()
    }
}

/// AES-256-GCM bound to the sponsorship record namespace and deployment.
pub(super) struct SponsorshipCipher {
    keyring: SponsorshipKeyring,
    random: SystemRandom,
    deployment: String,
}

impl SponsorshipCipher {
    pub(super) fn new(
        deployment: impl Into<String>,
        keyring: SponsorshipKeyring,
    ) -> Result<Self, SponsorshipStoreError> {
        let deployment = deployment.into();
        if deployment.is_empty() || deployment.len() > 255 {
            return Err(SponsorshipStoreError::Malformed(
                "sponsorship encryption deployment must contain 1 to 255 bytes".into(),
            ));
        }
        // Prove the configured current bytes are accepted by the selected AEAD.
        UnboundKey::new(&aead::AES_256_GCM, keyring.current().as_bytes()).map_err(|_| {
            SponsorshipStoreError::Malformed(
                "current sponsorship encryption key is not usable".into(),
            )
        })?;
        Ok(Self {
            keyring,
            random: SystemRandom::new(),
            deployment,
        })
    }

    pub(super) fn current_key_id(&self) -> &str {
        self.keyring.current_key_id()
    }

    pub(super) fn seal(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        plaintext: &str,
    ) -> Result<SponsorshipCipherEnvelope, SponsorshipStoreError> {
        validate_context(record_kind, record_id, field)?;
        let key_id = self.keyring.current_key_id().to_string();
        let key = LessSafeKey::new(
            UnboundKey::new(&aead::AES_256_GCM, self.keyring.current().as_bytes()).map_err(
                |_| {
                    SponsorshipStoreError::Malformed(
                        "current sponsorship encryption key is not usable".into(),
                    )
                },
            )?,
        );
        let mut nonce_bytes = [0_u8; aead::NONCE_LEN];
        self.random.fill(&mut nonce_bytes).map_err(|_| {
            SponsorshipStoreError::Unavailable(
                "encrypt sponsorship authority: secure randomness unavailable".into(),
            )
        })?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aad = self.aad(record_kind, record_id, field, ENVELOPE_VERSION, &key_id);
        let mut ciphertext = Zeroizing::new(plaintext.as_bytes().to_vec());
        key.seal_in_place_append_tag(nonce, Aad::from(aad), &mut *ciphertext)
            .map_err(|_| {
                SponsorshipStoreError::Unavailable(
                    "encrypt sponsorship authority: encryption failed".into(),
                )
            })?;
        Ok(SponsorshipCipherEnvelope {
            version: ENVELOPE_VERSION,
            key_id,
            nonce: binary(nonce_bytes.to_vec()),
            ciphertext: binary(ciphertext.to_vec()),
        })
    }

    pub(super) fn open(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<OpenedSponsorshipSecret, SponsorshipStoreError> {
        validate_context(record_kind, record_id, field)?;
        if envelope.version != ENVELOPE_VERSION {
            return Err(SponsorshipStoreError::Malformed(format!(
                "unsupported sponsorship ciphertext envelope version {}",
                envelope.version
            )));
        }
        validate_key_id(&envelope.key_id).map_err(SponsorshipStoreError::Malformed)?;
        if envelope.nonce.subtype != BinarySubtype::Generic
            || envelope.nonce.bytes.len() != aead::NONCE_LEN
            || envelope.ciphertext.subtype != BinarySubtype::Generic
            || envelope.ciphertext.bytes.len() < aead::AES_256_GCM.tag_len()
        {
            return Err(SponsorshipStoreError::Malformed(
                "stored sponsorship ciphertext envelope is malformed".into(),
            ));
        }
        let configured = self.keyring.key(&envelope.key_id).ok_or_else(|| {
            SponsorshipStoreError::Unavailable(format!(
                "sponsorship ciphertext requires unavailable key id '{}'",
                envelope.key_id
            ))
        })?;
        let key = LessSafeKey::new(
            UnboundKey::new(&aead::AES_256_GCM, configured.as_bytes()).map_err(|_| {
                SponsorshipStoreError::Malformed(
                    "stored sponsorship encryption key is not usable".into(),
                )
            })?,
        );
        let nonce_bytes: [u8; aead::NONCE_LEN] =
            envelope.nonce.bytes.as_slice().try_into().map_err(|_| {
                SponsorshipStoreError::Malformed(
                    "stored sponsorship ciphertext nonce is malformed".into(),
                )
            })?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aad = self.aad(
            record_kind,
            record_id,
            field,
            envelope.version,
            &envelope.key_id,
        );
        let mut ciphertext = Zeroizing::new(envelope.ciphertext.bytes.clone());
        let plaintext_len = key
            .open_in_place(nonce, Aad::from(aad), &mut ciphertext)
            .map_err(|_| {
                SponsorshipStoreError::Malformed(
                    "stored sponsorship ciphertext failed authentication".into(),
                )
            })?
            .len();
        ciphertext.truncate(plaintext_len);
        let plaintext = String::from_utf8(ciphertext.to_vec()).map_err(|_| {
            SponsorshipStoreError::Malformed(
                "stored sponsorship ciphertext is not valid UTF-8".into(),
            )
        })?;
        Ok(OpenedSponsorshipSecret {
            plaintext: Zeroizing::new(plaintext),
            needs_rewrap: envelope.key_id != self.keyring.current_key_id,
        })
    }

    pub(super) fn rewrap(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<Option<SponsorshipCipherEnvelope>, SponsorshipStoreError> {
        let opened = self.open(record_kind, record_id, field, envelope)?;
        if !opened.needs_rewrap {
            return Ok(None);
        }
        self.seal(record_kind, record_id, field, &opened.plaintext)
            .map(Some)
    }

    fn aad(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        version: i32,
        key_id: &str,
    ) -> Vec<u8> {
        let mut aad = Vec::with_capacity(
            AAD_DOMAIN.len()
                + self.deployment.len()
                + record_kind.len()
                + record_id.len()
                + field.len()
                + key_id.len()
                + 48,
        );
        append_aad_part(&mut aad, AAD_DOMAIN);
        append_aad_part(&mut aad, self.deployment.as_bytes());
        append_aad_part(&mut aad, record_kind.as_bytes());
        append_aad_part(&mut aad, record_id.as_bytes());
        append_aad_part(&mut aad, field.as_bytes());
        append_aad_part(&mut aad, &version.to_be_bytes());
        append_aad_part(&mut aad, key_id.as_bytes());
        aad
    }
}

fn append_aad_part(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

fn validate_context(
    record_kind: &str,
    record_id: &str,
    field: &str,
) -> Result<(), SponsorshipStoreError> {
    for (name, value, maximum) in [
        ("record kind", record_kind, 64_usize),
        ("record id", record_id, 512_usize),
        ("field", field, 64_usize),
    ] {
        if value.is_empty() || value.len() > maximum || value.bytes().any(|byte| byte == 0) {
            return Err(SponsorshipStoreError::Malformed(format!(
                "sponsorship encryption {name} is invalid"
            )));
        }
    }
    Ok(())
}

fn validate_key_id(key_id: &str) -> Result<(), String> {
    if key_id.is_empty()
        || key_id.len() > 64
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err("a sponsorship encryption key id must be 1 to 64 safe ASCII bytes".into());
    }
    Ok(())
}

fn binary(bytes: Vec<u8>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> SponsorshipEncryptionKey {
        SponsorshipEncryptionKey::from_bytes([byte; KEY_LEN])
    }

    fn cipher(current_id: &str, current: u8, previous: Vec<(&str, u8)>) -> SponsorshipCipher {
        SponsorshipCipher::new(
            "test-deployment",
            SponsorshipKeyring::new(
                current_id,
                key(current),
                previous
                    .into_iter()
                    .map(|(id, byte)| (id.to_string(), key(byte))),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn keyring_rejects_duplicate_and_unbounded_previous_keys() {
        let duplicate =
            SponsorshipKeyring::new("current", key(1), vec![("current".to_string(), key(2))])
                .unwrap_err();
        assert!(duplicate.contains("more than once"));

        let too_many = SponsorshipKeyring::new(
            "current",
            key(1),
            (0..=MAX_PREVIOUS_KEYS).map(|index| (format!("old-{index}"), key(index as u8 + 2))),
        )
        .unwrap_err();
        assert!(too_many.contains("at most"));
    }

    #[test]
    fn ciphertext_is_bound_to_deployment_record_and_field() {
        let cipher = cipher("current", 7, vec![]);
        let envelope = cipher
            .seal(
                "credential",
                "credential-a",
                "current_secret",
                "test material",
            )
            .unwrap();
        assert_eq!(
            cipher
                .open("credential", "credential-a", "current_secret", &envelope)
                .unwrap()
                .plaintext
                .as_str(),
            "test material"
        );
        for (deployment, record, field) in [
            ("other-deployment", "credential-a", "current_secret"),
            ("test-deployment", "credential-b", "current_secret"),
            ("test-deployment", "credential-a", "pending_secret"),
        ] {
            let other = SponsorshipCipher::new(
                deployment,
                SponsorshipKeyring::new("current", key(7), Vec::new()).unwrap(),
            )
            .unwrap();
            assert!(matches!(
                other.open("credential", record, field, &envelope),
                Err(SponsorshipStoreError::Malformed(_))
            ));
        }
    }

    #[test]
    fn previous_key_reads_and_rewraps_under_current_key() {
        let old = cipher("old", 3, vec![]);
        let envelope = old
            .seal("grant", "grant-a", "provider_grant_id", "opaque handle")
            .unwrap();
        let rotated = cipher("new", 9, vec![("old", 3)]);
        let opened = rotated
            .open("grant", "grant-a", "provider_grant_id", &envelope)
            .unwrap();
        assert!(opened.needs_rewrap);
        assert_eq!(opened.plaintext.as_str(), "opaque handle");

        let replacement = rotated
            .rewrap("grant", "grant-a", "provider_grant_id", &envelope)
            .unwrap()
            .expect("an old envelope is rewrapped");
        assert_eq!(replacement.key_id, "new");
        assert!(
            !rotated
                .open("grant", "grant-a", "provider_grant_id", &replacement)
                .unwrap()
                .needs_rewrap
        );
        assert!(rotated
            .rewrap("grant", "grant-a", "provider_grant_id", &replacement)
            .unwrap()
            .is_none());
    }

    #[test]
    fn tampering_and_missing_keys_fail_closed_without_plaintext_diagnostics() {
        let active = cipher("current", 4, vec![]);
        let mut envelope = active
            .seal(
                "credential",
                "credential-a",
                "current_secret",
                "private fixture",
            )
            .unwrap();
        envelope.ciphertext.bytes[0] ^= 1;
        let error = active
            .open("credential", "credential-a", "current_secret", &envelope)
            .unwrap_err();
        assert!(!error.to_string().contains("private fixture"));

        let old = cipher("old", 8, vec![])
            .seal(
                "credential",
                "credential-a",
                "current_secret",
                "private fixture",
            )
            .unwrap();
        let error = active
            .open("credential", "credential-a", "current_secret", &old)
            .unwrap_err();
        assert!(matches!(error, SponsorshipStoreError::Unavailable(_)));
        assert!(!error.to_string().contains("private fixture"));
        assert!(!format!("{old:?}").contains("private fixture"));
    }
}
