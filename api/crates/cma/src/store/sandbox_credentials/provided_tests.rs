use super::*;
use base64::Engine;
use layer2_core::sandbox::{SandboxCredentialAuthority, SandboxCredentialSource};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn provided_custody_encrypts_round_trips_and_rejects_hash_or_source_tampering() {
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    let key =
        AuthEncryptionKey::from_base64(&base64::engine::general_purpose::STANDARD.encode(bytes))
            .unwrap();
    // Serialization/cipher checks need collection handles, without connecting.
    let client = Client::with_uri_str("mongodb://127.0.0.1:1").await.unwrap();
    let db = client.database("custody_serialization");
    let store = MongoSandboxCredentialStore {
        client: client.clone(),
        sandboxes: majority_collection(&db, "sandboxes"),
        intents: majority_collection(&db, issuance::COLLECTION),
        collection: majority_collection(&db, SANDBOX_CREDENTIALS),
        revocations: majority_collection(&db, SANDBOX_CREDENTIAL_REVOCATIONS),
        cipher: AuthCipher::new("test", &key).unwrap(),
    };
    let secret = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let hash = format!("{:x}", Sha256::digest(secret.as_bytes()));
    let original = CredentialMaterial {
        source: SandboxCredentialSource::Provided {
            key_sha256: hash.clone(),
        },
        sandbox_id: SandboxId::generate(),
        owner: SubjectId::new("custody-owner"),
        authority: SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: format!("provided-{hash}"),
            provider_state_version: 1,
            issued_at: layer2_core::now_rfc3339(),
            expires_at: String::new(),
            grant_fingerprint: "a".repeat(64),
        },
        value: crate::auth::persistence::SecretValue::new(secret.clone()),
    };
    let document = store.document(&original).unwrap();
    assert!(!serde_json::to_string(&document).unwrap().contains(&secret));
    let recovered = store.material(document).unwrap();
    assert_eq!(recovered.source, original.source);
    assert_eq!(recovered.authority, original.authority);
    assert_eq!(recovered.value.expose(), secret);
    let mut tampered = store.document(&original).unwrap();
    tampered.source = SandboxCredentialSource::Managed;
    assert!(store.material(tampered).is_err());
    let mut tampered = store.document(&original).unwrap();
    let wrong = "b".repeat(64);
    tampered.source = SandboxCredentialSource::Provided {
        key_sha256: wrong.clone(),
    };
    tampered.provider_key_id = format!("provided-{wrong}");
    assert!(store.material(tampered).is_err());
}
