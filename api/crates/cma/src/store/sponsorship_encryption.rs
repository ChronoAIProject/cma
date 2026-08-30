//! Authenticated encryption for durable sponsorship authority.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::{
    SponsorshipOperationalPseudonymizer, SponsorshipStoreError, MAX_SPONSORED_MODEL_REQUEST_BYTES,
    MAX_SPONSORED_MODEL_RESPONSE_BYTES,
};
use mongodb::bson::{spec::BinarySubtype, Binary};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    hkdf,
    rand::{SecureRandom, SystemRandom},
};
use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use zeroize::{Zeroize, Zeroizing};

const KEY_LEN: usize = 32;
const ENVELOPE_VERSION: i32 = 2;
const AAD_DOMAIN: &[u8] = b"chrono-sponsorship-aead-v1";
const OPERATIONAL_HKDF_SALT: &[u8] = b"chrono-sponsorship-operational-hkdf-salt-v1";
const OPERATIONAL_HKDF_INFO: &[u8] = b"chrono-sponsorship-operational-hmac-subkey-v1";
const MAX_PREVIOUS_KEYS: usize = 4;
const MAX_PLAINTEXT_BYTES: usize = 64 * 1024;

struct OperationalSubkeyLength;

impl hkdf::KeyType for OperationalSubkeyLength {
    fn len(&self) -> usize {
        KEY_LEN
    }
}

struct UniqueStringMap(BTreeMap<String, String>);

impl UniqueStringMap {
    fn zeroize_values(&mut self) {
        for value in self.0.values_mut() {
            value.zeroize();
        }
    }
}

impl Drop for UniqueStringMap {
    fn drop(&mut self) {
        self.zeroize_values();
    }
}

impl<'de> Deserialize<'de> for UniqueStringMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueStringMapVisitor;

        impl<'de> Visitor<'de> for UniqueStringMapVisitor {
            type Value = UniqueStringMap;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map with unique sponsorship key ids")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = UniqueStringMap(BTreeMap::new());
                while let Some((key, mut value)) = map.next_entry::<String, String>()? {
                    if values.0.contains_key(&key) {
                        value.zeroize();
                        return Err(serde::de::Error::custom(format!(
                            "duplicate sponsorship key id '{key}'"
                        )));
                    }
                    values.0.insert(key, value);
                }
                Ok(values)
            }
        }

        deserializer.deserialize_map(UniqueStringMapVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SponsorshipKeyringFile {
    version: u32,
    current_key_id: String,
    keys: UniqueStringMap,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SponsorshipOperationalKeyringFile {
    version: u32,
    current_key_id: String,
    keys: UniqueStringMap,
}

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
    pub(crate) fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
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
    /// Parse the dedicated file-only keyring format.
    ///
    /// The map contains the current key and at most four predecessors. Keeping
    /// the selected ID separate makes rotation an explicit configuration
    /// change without depending on JSON map ordering.
    pub(crate) fn from_json(encoded: &str) -> Result<Self, String> {
        let file: SponsorshipKeyringFile = serde_json::from_str(encoded)
            .map_err(|_| "the sponsorship encryption keyring is not valid JSON".to_string())?;
        if file.version != 1 {
            return Err("the sponsorship encryption keyring version is unsupported".into());
        }
        let current = file.keys.0.get(&file.current_key_id).ok_or_else(|| {
            "the sponsorship encryption keyring does not contain its current key".to_string()
        })?;
        let current = SponsorshipEncryptionKey::from_base64(current)?;
        let mut previous = Vec::with_capacity(file.keys.0.len().saturating_sub(1));
        for (key_id, encoded) in &file.keys.0 {
            if key_id != &file.current_key_id {
                previous.push((
                    key_id.clone(),
                    SponsorshipEncryptionKey::from_base64(encoded)?,
                ));
            }
        }
        Self::new(file.current_key_id.clone(), current, previous)
    }

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
            if keys.contains_key(&key_id) {
                return Err(format!(
                    "sponsorship encryption key id '{key_id}' is configured more than once"
                ));
            }
            if keys
                .values()
                .any(|configured| configured.as_bytes() == key.as_bytes())
            {
                return Err(format!(
                    "sponsorship encryption key material is configured under multiple ids, including '{key_id}'"
                ));
            }
            keys.insert(key_id, key);
        }
        Ok(Self {
            current_key_id,
            keys,
        })
    }

    pub(crate) fn current_key_id(&self) -> &str {
        &self.current_key_id
    }

    /// Key ids whose ciphertext must be rewrapped before the key can retire.
    ///
    /// This deliberately exposes no key bytes and says nothing about the
    /// independently rotated operational-pseudonym keyring.
    pub(crate) fn retiring_key_ids(&self) -> Vec<String> {
        self.keys
            .keys()
            .filter(|key_id| *key_id != &self.current_key_id)
            .cloned()
            .collect()
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

/// One independently supplied root used only for durable operational HMAC
/// pseudonyms. It is never accepted as AEAD material.
#[derive(Clone)]
pub(crate) struct SponsorshipOperationalKey(Zeroizing<[u8; KEY_LEN]>);

impl SponsorshipOperationalKey {
    pub(crate) fn from_base64(encoded: &str) -> Result<Self, String> {
        let decoded = Zeroizing::new(STANDARD.decode(encoded.trim()).map_err(|_| {
            "a sponsorship operational key must be valid standard base64".to_string()
        })?);
        if decoded.len() != KEY_LEN {
            return Err(format!(
                "a sponsorship operational key must decode to {KEY_LEN} bytes, got {}",
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
    pub(crate) fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl std::fmt::Debug for SponsorshipOperationalKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SponsorshipOperationalKey(<redacted>)")
    }
}

/// Independently rotated current/predecessor roots for durable `sp1`
/// identities. Operational predecessors remain configured until the Mongo
/// dependency inventory proves no persisted identity references them.
#[derive(Clone)]
pub(crate) struct SponsorshipOperationalKeyring {
    current_key_id: String,
    keys: BTreeMap<String, SponsorshipOperationalKey>,
}

impl SponsorshipOperationalKeyring {
    pub(crate) fn from_json(encoded: &str) -> Result<Self, String> {
        let file: SponsorshipOperationalKeyringFile = serde_json::from_str(encoded)
            .map_err(|_| "the sponsorship operational keyring is not valid JSON".to_string())?;
        if file.version != 1 {
            return Err("the sponsorship operational keyring version is unsupported".into());
        }
        let current = file.keys.0.get(&file.current_key_id).ok_or_else(|| {
            "the sponsorship operational keyring does not contain its current key".to_string()
        })?;
        let current = SponsorshipOperationalKey::from_base64(current)?;
        let mut previous = Vec::with_capacity(file.keys.0.len().saturating_sub(1));
        for (key_id, encoded) in &file.keys.0 {
            if key_id != &file.current_key_id {
                previous.push((
                    key_id.clone(),
                    SponsorshipOperationalKey::from_base64(encoded)?,
                ));
            }
        }
        Self::new(file.current_key_id.clone(), current, previous)
    }

    pub(crate) fn new(
        current_key_id: impl Into<String>,
        current: SponsorshipOperationalKey,
        previous: impl IntoIterator<Item = (String, SponsorshipOperationalKey)>,
    ) -> Result<Self, String> {
        let current_key_id = current_key_id.into();
        validate_key_id(&current_key_id).map_err(|_| {
            "a sponsorship operational key id must be 1 to 64 safe ASCII bytes".to_string()
        })?;
        let previous = previous.into_iter().collect::<Vec<_>>();
        if previous.len() > MAX_PREVIOUS_KEYS {
            return Err(format!(
                "at most {MAX_PREVIOUS_KEYS} previous sponsorship operational keys are supported"
            ));
        }
        let mut keys = BTreeMap::from([(current_key_id.clone(), current)]);
        for (key_id, key) in previous {
            validate_key_id(&key_id).map_err(|_| {
                "a sponsorship operational key id must be 1 to 64 safe ASCII bytes".to_string()
            })?;
            if keys.contains_key(&key_id) {
                return Err(format!(
                    "sponsorship operational key id '{key_id}' is configured more than once"
                ));
            }
            if keys
                .values()
                .any(|configured| configured.as_bytes() == key.as_bytes())
            {
                return Err(format!(
                    "sponsorship operational key material is configured under multiple ids, including '{key_id}'"
                ));
            }
            keys.insert(key_id, key);
        }
        Ok(Self {
            current_key_id,
            keys,
        })
    }

    #[cfg(test)]
    pub(crate) fn contains_key_id(&self, key_id: &str) -> bool {
        self.keys.contains_key(key_id)
    }

    pub(crate) fn pseudonymizer(
        &self,
        deployment: &str,
    ) -> Result<SponsorshipOperationalPseudonymizer, SponsorshipStoreError> {
        if deployment.is_empty() || deployment.len() > 255 || deployment.bytes().any(|b| b == 0) {
            return Err(SponsorshipStoreError::Malformed(
                "sponsorship operational deployment must contain 1 to 255 non-NUL bytes".into(),
            ));
        }
        let current_root = self
            .keys
            .get(&self.current_key_id)
            .expect("construction always inserts the current operational key");
        let current = derive_operational_subkey(deployment, &self.current_key_id, current_root)?;
        let previous = self
            .keys
            .iter()
            .filter(|(key_id, _)| *key_id != &self.current_key_id)
            .map(|(key_id, key)| {
                derive_operational_subkey(deployment, key_id, key)
                    .map(|derived| (key_id.clone(), derived))
            })
            .collect::<Result<Vec<_>, _>>()?;
        SponsorshipOperationalPseudonymizer::new(
            self.current_key_id.clone(),
            current.to_vec(),
            previous
                .iter()
                .map(|(key_id, key)| (key_id.clone(), key.to_vec())),
        )
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))
    }
}

impl std::fmt::Debug for SponsorshipOperationalKeyring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorshipOperationalKeyring")
            .field("current_key_id", &self.current_key_id)
            .field("key_count", &self.keys.len())
            .finish()
    }
}

pub(crate) fn validate_independent_keyrings(
    encryption: &SponsorshipKeyring,
    operational: &SponsorshipOperationalKeyring,
) -> Result<(), String> {
    for (encryption_id, encryption_key) in &encryption.keys {
        if operational.keys.contains_key(encryption_id) {
            return Err(format!(
                "sponsorship encryption and operational keyrings reuse key id '{encryption_id}'"
            ));
        }
        if operational
            .keys
            .values()
            .any(|operational_key| operational_key.as_bytes() == encryption_key.as_bytes())
        {
            return Err(
                "sponsorship encryption and operational keyrings reuse secret material".into(),
            );
        }
    }
    Ok(())
}

fn derive_operational_subkey(
    deployment: &str,
    key_id: &str,
    operational_key: &SponsorshipOperationalKey,
) -> Result<Zeroizing<Vec<u8>>, SponsorshipStoreError> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, OPERATIONAL_HKDF_SALT);
    let pseudorandom_key = salt.extract(operational_key.as_bytes());
    let mut info = Zeroizing::new(Vec::with_capacity(
        OPERATIONAL_HKDF_INFO.len() + deployment.len() + key_id.len() + 24,
    ));
    append_aad_part(&mut info, OPERATIONAL_HKDF_INFO);
    append_aad_part(&mut info, deployment.as_bytes());
    append_aad_part(&mut info, key_id.as_bytes());
    let info_parts = [info.as_slice()];
    let output = pseudorandom_key
        .expand(&info_parts, OperationalSubkeyLength)
        .map_err(|_| {
            SponsorshipStoreError::Malformed(
                "sponsorship operational subkey context is unusable".into(),
            )
        })?;
    let mut derived = Zeroizing::new(vec![0_u8; KEY_LEN]);
    output.fill(derived.as_mut_slice()).map_err(|_| {
        SponsorshipStoreError::Malformed("sponsorship operational subkey derivation failed".into())
    })?;
    Ok(derived)
}

/// One encrypted sponsorship field as stored in BSON.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        authority_context: &str,
        plaintext: &str,
    ) -> Result<SponsorshipCipherEnvelope, SponsorshipStoreError> {
        self.seal_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            plaintext,
            MAX_PLAINTEXT_BYTES,
        )
    }

    pub(super) fn seal_model_request(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        plaintext: &str,
    ) -> Result<SponsorshipCipherEnvelope, SponsorshipStoreError> {
        self.seal_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            plaintext,
            MAX_SPONSORED_MODEL_REQUEST_BYTES,
        )
    }

    pub(super) fn seal_model_response(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        plaintext: &str,
    ) -> Result<SponsorshipCipherEnvelope, SponsorshipStoreError> {
        self.seal_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            plaintext,
            MAX_SPONSORED_MODEL_RESPONSE_BYTES,
        )
    }

    fn seal_with_limit(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        plaintext: &str,
        maximum_plaintext_bytes: usize,
    ) -> Result<SponsorshipCipherEnvelope, SponsorshipStoreError> {
        validate_context(record_kind, record_id, field, authority_context)?;
        if plaintext.len() > maximum_plaintext_bytes {
            return Err(SponsorshipStoreError::Malformed(format!(
                "sponsorship authority plaintext exceeds {maximum_plaintext_bytes} bytes"
            )));
        }
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
        let aad = self.aad(
            record_kind,
            record_id,
            field,
            authority_context,
            ENVELOPE_VERSION,
            &key_id,
        );
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
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<OpenedSponsorshipSecret, SponsorshipStoreError> {
        self.open_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            envelope,
            MAX_PLAINTEXT_BYTES,
        )
    }

    pub(super) fn open_model_request(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<OpenedSponsorshipSecret, SponsorshipStoreError> {
        self.open_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            envelope,
            MAX_SPONSORED_MODEL_REQUEST_BYTES,
        )
    }

    pub(super) fn open_model_response(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<OpenedSponsorshipSecret, SponsorshipStoreError> {
        self.open_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            envelope,
            MAX_SPONSORED_MODEL_RESPONSE_BYTES,
        )
    }

    fn open_with_limit(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
        maximum_plaintext_bytes: usize,
    ) -> Result<OpenedSponsorshipSecret, SponsorshipStoreError> {
        validate_context(record_kind, record_id, field, authority_context)?;
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
            || envelope.ciphertext.bytes.len()
                > maximum_plaintext_bytes + aead::AES_256_GCM.tag_len()
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
            authority_context,
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
        // Validate through a borrow so an invalid UTF-8 path never creates a
        // `FromUtf8Error` that owns and ordinarily drops cloned plaintext.
        let plaintext = std::str::from_utf8(&ciphertext).map_err(|_| {
            SponsorshipStoreError::Malformed(
                "stored sponsorship ciphertext is not valid UTF-8".into(),
            )
        })?;
        Ok(OpenedSponsorshipSecret {
            plaintext: Zeroizing::new(plaintext.to_owned()),
            needs_rewrap: envelope.key_id != self.keyring.current_key_id,
        })
    }

    pub(super) fn rewrap(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<Option<SponsorshipCipherEnvelope>, SponsorshipStoreError> {
        self.rewrap_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            envelope,
            MAX_PLAINTEXT_BYTES,
        )
    }

    pub(super) fn rewrap_model_request(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<Option<SponsorshipCipherEnvelope>, SponsorshipStoreError> {
        self.rewrap_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            envelope,
            MAX_SPONSORED_MODEL_REQUEST_BYTES,
        )
    }

    pub(super) fn rewrap_model_response(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
    ) -> Result<Option<SponsorshipCipherEnvelope>, SponsorshipStoreError> {
        self.rewrap_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            envelope,
            MAX_SPONSORED_MODEL_RESPONSE_BYTES,
        )
    }

    fn rewrap_with_limit(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        envelope: &SponsorshipCipherEnvelope,
        maximum_plaintext_bytes: usize,
    ) -> Result<Option<SponsorshipCipherEnvelope>, SponsorshipStoreError> {
        let opened = self.open_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            envelope,
            maximum_plaintext_bytes,
        )?;
        if !opened.needs_rewrap {
            return Ok(None);
        }
        self.seal_with_limit(
            record_kind,
            record_id,
            field,
            authority_context,
            &opened.plaintext,
            maximum_plaintext_bytes,
        )
        .map(Some)
    }

    fn aad(
        &self,
        record_kind: &str,
        record_id: &str,
        field: &str,
        authority_context: &str,
        version: i32,
        key_id: &str,
    ) -> Vec<u8> {
        let mut aad = Vec::with_capacity(
            AAD_DOMAIN.len()
                + self.deployment.len()
                + record_kind.len()
                + record_id.len()
                + field.len()
                + authority_context.len()
                + key_id.len()
                + 48,
        );
        append_aad_part(&mut aad, AAD_DOMAIN);
        append_aad_part(&mut aad, self.deployment.as_bytes());
        append_aad_part(&mut aad, record_kind.as_bytes());
        append_aad_part(&mut aad, record_id.as_bytes());
        append_aad_part(&mut aad, field.as_bytes());
        append_aad_part(&mut aad, authority_context.as_bytes());
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
    authority_context: &str,
) -> Result<(), SponsorshipStoreError> {
    for (name, value, maximum) in [
        ("record kind", record_kind, 64_usize),
        ("record id", record_id, 512_usize),
        ("field", field, 64_usize),
        ("authority context", authority_context, 128_usize),
    ] {
        if value.is_empty() || value.len() > maximum || value.bytes().any(|byte| byte == 0) {
            return Err(SponsorshipStoreError::Malformed(format!(
                "sponsorship encryption {name} is invalid"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_key_id(key_id: &str) -> Result<(), String> {
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
    use layer2_core::SponsorshipOperationalDomain;

    fn key(byte: u8) -> SponsorshipEncryptionKey {
        SponsorshipEncryptionKey::from_bytes([byte; KEY_LEN])
    }

    fn operational_key(byte: u8) -> SponsorshipOperationalKey {
        SponsorshipOperationalKey::from_bytes([byte; KEY_LEN])
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

        let duplicate_material =
            SponsorshipKeyring::new("current", key(1), vec![("previous".to_string(), key(1))])
                .unwrap_err();
        assert!(duplicate_material.contains("material"));

        let operational_duplicate = SponsorshipOperationalKeyring::new(
            "operational-current",
            operational_key(9),
            vec![("operational-previous".to_string(), operational_key(9))],
        )
        .unwrap_err();
        assert!(operational_duplicate.contains("material"));
    }

    #[test]
    fn encryption_keyring_exposes_only_sorted_retiring_key_ids() {
        let keyring = SponsorshipKeyring::new(
            "current",
            key(1),
            [("old-z".to_string(), key(2)), ("old-a".to_string(), key(3))],
        )
        .unwrap();

        assert_eq!(keyring.retiring_key_ids(), ["old-a", "old-z"]);
        assert!(!keyring.retiring_key_ids().iter().any(|id| id == "current"));
    }

    #[test]
    fn operational_subkeys_are_deployment_bound_and_preserve_rotation_candidates() {
        let old_keyring = SponsorshipOperationalKeyring::new(
            "operational-old.2026:07",
            operational_key(3),
            Vec::new(),
        )
        .unwrap();
        assert!(old_keyring.contains_key_id("operational-old.2026:07"));
        let old = old_keyring
            .pseudonymizer("deployment-a")
            .unwrap()
            .pseudonymize(
                SponsorshipOperationalDomain::UsageRequest,
                &["grant-a", "requester-a", "predictable-request"],
            )
            .unwrap()
            .current()
            .clone();

        let rotated = SponsorshipOperationalKeyring::new(
            "operational-new.2026:08",
            operational_key(9),
            vec![("operational-old.2026:07".to_string(), operational_key(3))],
        )
        .unwrap()
        .pseudonymizer("deployment-a")
        .unwrap()
        .pseudonymize(
            SponsorshipOperationalDomain::UsageRequest,
            &["grant-a", "requester-a", "predictable-request"],
        )
        .unwrap();
        assert!(rotated.matches(&old));
        assert_ne!(rotated.current(), &old);

        let other_deployment = old_keyring
            .pseudonymizer("deployment-b")
            .unwrap()
            .pseudonymize(
                SponsorshipOperationalDomain::UsageRequest,
                &["grant-a", "requester-a", "predictable-request"],
            )
            .unwrap();
        assert_ne!(other_deployment.current(), &old);
    }

    #[test]
    fn keyring_json_rejects_duplicate_key_ids() {
        let encoded = format!(
            r#"{{
                "version": 1,
                "current_key_id": "current",
                "keys": {{
                    "current": "{key}",
                    "current": "{key}"
                }}
            }}"#,
            key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        );
        assert!(SponsorshipKeyring::from_json(&encoded).is_err());
        assert!(SponsorshipOperationalKeyring::from_json(&encoded).is_err());

        let duplicate_material = format!(
            r#"{{
                "version": 1,
                "current_key_id": "current",
                "keys": {{
                    "current": "{key}",
                    "previous": "{key}"
                }}
            }}"#,
            key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        );
        assert!(SponsorshipKeyring::from_json(&duplicate_material).is_err());
        assert!(SponsorshipOperationalKeyring::from_json(&duplicate_material).is_err());
    }

    #[test]
    fn encryption_and_operational_keyrings_reject_shared_ids_or_bytes() {
        let encryption = SponsorshipKeyring::new("aead-current", key(1), Vec::new()).unwrap();
        let shared_id =
            SponsorshipOperationalKeyring::new("aead-current", operational_key(2), Vec::new())
                .unwrap();
        assert!(validate_independent_keyrings(&encryption, &shared_id)
            .unwrap_err()
            .contains("reuse key id"));

        let shared_bytes = SponsorshipOperationalKeyring::new(
            "operational-current",
            operational_key(1),
            Vec::new(),
        )
        .unwrap();
        assert!(validate_independent_keyrings(&encryption, &shared_bytes)
            .unwrap_err()
            .contains("reuse secret material"));

        let independent = SponsorshipOperationalKeyring::new(
            "operational-current",
            operational_key(2),
            Vec::new(),
        )
        .unwrap();
        validate_independent_keyrings(&encryption, &independent).unwrap();
    }

    #[test]
    fn intermediate_key_maps_zeroize_values_on_every_drop_path() {
        let mut values = UniqueStringMap(BTreeMap::from([
            ("current".to_string(), "current-secret-key".to_string()),
            ("previous".to_string(), "previous-secret-key".to_string()),
        ]));
        values.zeroize_values();
        assert!(values.0.values().all(String::is_empty));

        let invalid_after_keys = r#"{
            "keys": {"current": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="},
            "version": "not-an-integer",
            "current_key_id": "current"
        }"#;
        assert!(SponsorshipKeyring::from_json(invalid_after_keys).is_err());
        assert!(SponsorshipOperationalKeyring::from_json(invalid_after_keys).is_err());
    }

    #[test]
    fn ciphertext_is_bound_to_deployment_record_field_and_authority() {
        let cipher = cipher("current", 7, vec![]);
        let authority = "authority-generation-7";
        let envelope = cipher
            .seal(
                "credential",
                "credential-a",
                "current_secret",
                authority,
                "test material",
            )
            .unwrap();
        assert_eq!(
            cipher
                .open(
                    "credential",
                    "credential-a",
                    "current_secret",
                    authority,
                    &envelope,
                )
                .unwrap()
                .plaintext
                .as_str(),
            "test material"
        );
        for (deployment, record, field, candidate_authority) in [
            (
                "other-deployment",
                "credential-a",
                "current_secret",
                authority,
            ),
            (
                "test-deployment",
                "credential-b",
                "current_secret",
                authority,
            ),
            (
                "test-deployment",
                "credential-a",
                "pending_secret",
                authority,
            ),
            (
                "test-deployment",
                "credential-a",
                "current_secret",
                "authority-generation-8",
            ),
        ] {
            let other = SponsorshipCipher::new(
                deployment,
                SponsorshipKeyring::new("current", key(7), Vec::new()).unwrap(),
            )
            .unwrap();
            assert!(matches!(
                other.open("credential", record, field, candidate_authority, &envelope,),
                Err(SponsorshipStoreError::Malformed(_))
            ));
        }
    }

    #[test]
    fn previous_key_reads_and_rewraps_under_current_key() {
        let old = cipher("old", 3, vec![]);
        let authority = "immutable-grant-authority";
        let envelope = old
            .seal(
                "grant",
                "grant-a",
                "provider_grant_id",
                authority,
                "opaque handle",
            )
            .unwrap();
        let rotated = cipher("new", 9, vec![("old", 3)]);
        let opened = rotated
            .open(
                "grant",
                "grant-a",
                "provider_grant_id",
                authority,
                &envelope,
            )
            .unwrap();
        assert!(opened.needs_rewrap);
        assert_eq!(opened.plaintext.as_str(), "opaque handle");

        let replacement = rotated
            .rewrap(
                "grant",
                "grant-a",
                "provider_grant_id",
                authority,
                &envelope,
            )
            .unwrap()
            .expect("an old envelope is rewrapped");
        assert_eq!(replacement.key_id, "new");
        assert!(
            !rotated
                .open(
                    "grant",
                    "grant-a",
                    "provider_grant_id",
                    authority,
                    &replacement,
                )
                .unwrap()
                .needs_rewrap
        );
        assert!(rotated
            .rewrap(
                "grant",
                "grant-a",
                "provider_grant_id",
                authority,
                &replacement,
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn tampering_and_missing_keys_fail_closed_without_plaintext_diagnostics() {
        let active = cipher("current", 4, vec![]);
        let authority = "credential-generation-3";
        let mut envelope = active
            .seal(
                "credential",
                "credential-a",
                "current_secret",
                authority,
                "private fixture",
            )
            .unwrap();
        envelope.ciphertext.bytes[0] ^= 1;
        let error = active
            .open(
                "credential",
                "credential-a",
                "current_secret",
                authority,
                &envelope,
            )
            .unwrap_err();
        assert!(!error.to_string().contains("private fixture"));

        let old = cipher("old", 8, vec![])
            .seal(
                "credential",
                "credential-a",
                "current_secret",
                authority,
                "private fixture",
            )
            .unwrap();
        let error = active
            .open(
                "credential",
                "credential-a",
                "current_secret",
                authority,
                &old,
            )
            .unwrap_err();
        assert!(matches!(error, SponsorshipStoreError::Unavailable(_)));
        assert!(!error.to_string().contains("private fixture"));
        assert!(!format!("{old:?}").contains("private fixture"));
    }

    #[test]
    fn plaintext_and_ciphertext_size_limits_fail_before_unbounded_copy_or_decrypt() {
        let active = cipher("current", 4, vec![]);
        let oversized = "x".repeat(MAX_PLAINTEXT_BYTES + 1);
        assert!(matches!(
            active.seal(
                "credential",
                "credential-a",
                "current_secret",
                "credential-generation-3",
                &oversized,
            ),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut envelope = active
            .seal(
                "credential",
                "credential-a",
                "current_secret",
                "credential-generation-3",
                "private fixture",
            )
            .unwrap();
        envelope
            .ciphertext
            .bytes
            .resize(MAX_PLAINTEXT_BYTES + aead::AES_256_GCM.tag_len() + 1, 0);
        assert!(matches!(
            active.open(
                "credential",
                "credential-a",
                "current_secret",
                "credential-generation-3",
                &envelope,
            ),
            Err(SponsorshipStoreError::Malformed(_))
        ));
    }
}
