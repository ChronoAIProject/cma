//! Synthetic custody regression; no service initialization or database access.
use super::*;

#[test]
fn restore_evidence_auth() {
    let key_spec = std::env::var("HOME_BACKUP_EVIDENCE_KEYS").unwrap_or_else(|_| {
        let random = SystemRandom::new();
        (0..3)
            .map(|_| {
                let mut bytes = [0; 32];
                random.fill(&mut bytes).unwrap();
                STANDARD.encode(bytes)
            })
            .collect::<Vec<_>>()
            .join(",")
    });
    let keys: Vec<_> = key_spec.split(',').collect();
    assert!(keys.len() == 3, "synthetic key count");
    let sentinel = std::env::var("HOME_BACKUP_EVIDENCE_SENTINEL")
        .unwrap_or_else(|_| "synthetic-auth-custody-payload".into());
    let phase = std::env::var("HOME_BACKUP_EVIDENCE_PHASE").unwrap_or_else(|_| "both".into());
    assert!(matches!(phase.as_str(), "write" | "read" | "both"));
    let directory = std::env::var_os("HOME_BACKUP_EVIDENCE_ARTIFACTS");
    for (index, encoded) in keys[..2].iter().enumerate() {
        for field in ["oauth_token", "api_bearer", "actor_grant"] {
            let key = AuthEncryptionKey::from_base64(encoded).unwrap();
            let writer = AuthCipher::new("isolated-custody-fixture", &key).unwrap();
            let path = directory
                .as_ref()
                .map(|root| std::path::Path::new(root).join(format!("auth-{index}-{field}.json")));
            let bytes = if phase == "read" {
                std::fs::read(path.as_ref().expect("artifact directory")).unwrap()
            } else {
                let envelope = writer
                    .seal("custody-fixture", b"synthetic-record", field, &sentinel)
                    .unwrap();
                let bytes = serde_json::to_vec(&envelope).unwrap();
                if let Some(path) = &path {
                    std::fs::write(path, &bytes).unwrap();
                }
                bytes
            };
            drop(writer);
            drop(key);
            if phase == "write" {
                continue;
            }
            let restored: CipherEnvelope = serde_json::from_slice(&bytes).unwrap();
            let reader = AuthCipher::new(
                "isolated-custody-fixture",
                &AuthEncryptionKey::from_base64(encoded).unwrap(),
            )
            .unwrap();
            let opened = reader
                .open(
                    "custody-fixture",
                    b"synthetic-record",
                    field,
                    restored.clone(),
                )
                .unwrap();
            // Avoid assert_eq!: a failure must not print plaintext.
            assert!(opened == sentinel, "restored auth payload mismatch");
            let wrong = AuthCipher::new(
                "isolated-custody-fixture",
                &AuthEncryptionKey::from_base64(keys[2]).unwrap(),
            )
            .unwrap();
            let Err(error) = wrong.open(
                "custody-fixture",
                b"synthetic-record",
                field,
                restored.clone(),
            ) else {
                panic!("wrong key must refuse");
            };
            println!("auth custody version {index}, field {field}: {restored:?}; {error}");
        }
    }
}
