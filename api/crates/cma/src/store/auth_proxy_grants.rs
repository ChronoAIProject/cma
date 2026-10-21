//! Authenticated immutable proxy admission, separate from browser and API keys.
use super::*;
use crate::auth::nyxid_proxy::ProxyGrant;

const KIND: &str = "nyxid-proxy-admission";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProxyGrantDoc {
    #[serde(rename = "_id")]
    id: Binary,
    document_schema_version: i64,
    grant: CipherEnvelope,
}

impl MongoAuthStore {
    pub(super) async fn insert_proxy_grant_record(
        &self,
        grant: ProxyGrant,
    ) -> Result<bool, StoreError> {
        let document = encode_grant(&self.cipher, grant)?;
        match self.proxy_grants.insert_one(document).await {
            Ok(_) => Ok(true),
            Err(error) if super::super::sandbox_mongo::is_duplicate_key(&error) => Ok(false),
            Err(_) => Err(StoreError::Unavailable("insert proxy grant".into())),
        }
    }

    pub(super) async fn read_proxy_grant_record(
        &self,
        id: &[u8; 32],
    ) -> Result<Option<ProxyGrant>, StoreError> {
        let Some(document) = self
            .proxy_grants
            .find_one(doc! {"_id":binary(*id)})
            .await
            .map_err(|_| StoreError::Unavailable("read proxy grant".into()))?
        else {
            return Ok(None);
        };
        decode_grant(&self.cipher, id, document).map(Some)
    }
}

fn encode_grant(cipher: &AuthCipher, grant: ProxyGrant) -> Result<ProxyGrantDoc, StoreError> {
    grant.validate()?;
    let encoded = serde_json::to_string(&grant)
        .map_err(|_| StoreError::Malformed("proxy grant encoding".into()))?;
    Ok(ProxyGrantDoc {
        id: binary(grant.id),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        grant: cipher.seal(KIND, &grant.id, "grant", &encoded)?,
    })
}

fn decode_grant(
    cipher: &AuthCipher,
    id: &[u8; 32],
    document: ProxyGrantDoc,
) -> Result<ProxyGrant, StoreError> {
    schema::require_current(document.document_schema_version)?;
    if digest(&document.id, "proxy grant")?.as_bytes() != id {
        return Err(StoreError::Malformed(
            "proxy grant identity mismatch".into(),
        ));
    }
    let plaintext = cipher.open(KIND, id, "grant", document.grant)?;
    let grant: ProxyGrant = serde_json::from_str(&plaintext)
        .map_err(|_| StoreError::Malformed("proxy grant encoding".into()))?;
    grant.validate()?;
    if &grant.id != id {
        return Err(StoreError::Malformed(
            "proxy grant identity mismatch".into(),
        ));
    }
    Ok(grant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use layer2_core::Capability;
    use sha2::{Digest, Sha256};

    fn cipher(deployment: &str, key_byte: u8) -> AuthCipher {
        let key = AuthEncryptionKey::from_base64(&STANDARD.encode([key_byte; 32])).unwrap();
        AuthCipher::new(deployment, &key).unwrap()
    }

    fn grant(assertion: &str) -> ProxyGrant {
        let issuer = "https://nyx.example";
        let service = "cma-service";
        let subject = SubjectId::new("alice");
        let id = Sha256::digest(
            serde_json::to_vec(&(
                "nyxid-proxy-admission-v1",
                issuer,
                service,
                &subject,
                assertion,
            ))
            .unwrap(),
        )
        .into();
        ProxyGrant {
            id,
            issuer: issuer.into(),
            service_id: service.into(),
            assertion_id: assertion.into(),
            subject,
            authorization: Authorization::new([Capability::UseOwnSandboxes]),
            owner_incarnation: "owner-sign-in-a".into(),
        }
    }

    #[test]
    fn proxy_grant_persistence_round_trips_exact_admitted_authority() {
        let cipher = cipher("deployment-a", 11);
        let grant = grant("assertion-a");
        let document = encode_grant(&cipher, grant.clone()).unwrap();
        let serialized = mongodb::bson::serialize_to_document(&document).unwrap();
        let persisted = mongodb::bson::deserialize_from_document(serialized).unwrap();
        assert_eq!(decode_grant(&cipher, &grant.id, persisted).unwrap(), grant);
    }

    #[test]
    fn proxy_grant_rebinding_and_cross_deployment_reads_fail_closed() {
        let original_cipher = cipher("deployment-a", 11);
        let original = grant("assertion-a");
        let other = grant("assertion-b");
        let document = encode_grant(&original_cipher, original.clone()).unwrap();
        assert!(decode_grant(&original_cipher, &other.id, document.clone()).is_err());
        let mut rebound = document.clone();
        rebound.id = binary(other.id);
        assert!(decode_grant(&original_cipher, &other.id, rebound).is_err());
        assert!(decode_grant(&cipher("deployment-b", 11), &original.id, document.clone()).is_err());
        assert!(decode_grant(&cipher("deployment-a", 12), &original.id, document).is_err());
    }

    #[test]
    fn proxy_grant_ciphertext_tampering_cannot_increase_authority() {
        let cipher = cipher("deployment-a", 11);
        let grant = grant("assertion-a");
        let document = encode_grant(&cipher, grant.clone()).unwrap();
        let mut envelope = mongodb::bson::serialize_to_document(&document.grant).unwrap();
        // Tamper through the envelope's storage representation, as a database
        // writer could, without access to the authentication encryption key.
        let mongodb::bson::Bson::Binary(ciphertext) = envelope.get_mut("ciphertext").unwrap()
        else {
            panic!("ciphertext must be BSON binary");
        };
        ciphertext.bytes[0] ^= 1;
        let tampered = ProxyGrantDoc {
            grant: mongodb::bson::deserialize_from_document(envelope).unwrap(),
            ..document
        };
        assert!(decode_grant(&cipher, &grant.id, tampered).is_err());
    }

    #[test]
    fn proxy_grant_schema_and_inner_identity_are_validated() {
        let cipher = cipher("deployment-a", 11);
        let grant = grant("assertion-a");
        let mut document = encode_grant(&cipher, grant.clone()).unwrap();
        document.document_schema_version += 1;
        assert!(decode_grant(&cipher, &grant.id, document).is_err());

        let mut invalid = grant.clone();
        invalid.subject = SubjectId::new("bob");
        assert!(encode_grant(&cipher, invalid.clone()).is_err());
        // Even an authenticated envelope containing an internally inconsistent
        // payload cannot establish a different principal or assertion identity.
        let encoded = serde_json::to_string(&invalid).unwrap();
        let document = ProxyGrantDoc {
            id: binary(grant.id),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            grant: cipher.seal(KIND, &grant.id, "grant", &encoded).unwrap(),
        };
        assert!(decode_grant(&cipher, &grant.id, document).is_err());
    }
}
