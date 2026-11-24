use super::*;

#[test]
fn wire_and_proof_vectors_keep_utf8_lengths_and_exact_json_spelling() {
    let bytes = r#"{ "title" : "你好 \u0061" }"#.as_bytes();
    let wire = wire_digest(bytes);
    assert_eq!(
        wire,
        "c0486b7300c06afb4a72e04fdfe88855df403d3bac3cdb6e8bc93fe17f7f4af5"
    );
    assert_ne!(wire, wire_digest(r#"{"title":"你好 a"}"#.as_bytes()));
    let owner = SubjectId::new("owner-甲");
    let deployment = "a".repeat(64);
    let epoch = "1".repeat(32);
    let nonce = "2".repeat(32);
    let key = format!("cma1.{epoch}.20000.{nonce}.b1d6fe598fa05bc4c09ced190765f504195211e00ab53d2ee12d459c4bd44c18");
    let identity = Identity {
        owner: &owner,
        deployment: &deployment,
        operation: Operation::Sandbox,
        key: &key,
        algorithm: Algorithm::NyxidWire,
        wire_digest: &wire,
    };
    let period = identity.period().unwrap().unwrap();
    let digest = identity.digest();
    assert_eq!(
        digest,
        "974e68b370dafd6ecc05f5e9a9fb940f668c33dbd02482ee01d4b4b9ef7b8aff"
    );
    assert_eq!(
        Outcome::never_received(&digest, &period),
        Outcome::NeverReceived {
            proof_id: "0bfa8d4de9367ff5efe253c1172c95c70d37606a38be33fc3bbb3f088402f1ad".into(),
        }
    );
    assert!(matches!(
        Identity {
            operation: Operation::AgentBootstrap,
            ..identity
        }
        .period(),
        Err(IdentityError::Mismatch)
    ));
    assert!(matches!(
        Identity {
            owner: &SubjectId::new("another-owner"),
            ..identity
        }
        .period(),
        Err(IdentityError::Mismatch)
    ));
}

#[test]
fn covered_key_classification_is_independent_of_the_request_binding() {
    let owner = SubjectId::new("owner");
    let deployment = "d".repeat(64);
    let digest = wire_digest(b"{}");
    let opaque = Identity {
        owner: &owner,
        deployment: &deployment,
        operation: Operation::AgentBootstrap,
        key: "cma1.retry-17",
        algorithm: Algorithm::NyxidWire,
        wire_digest: &digest,
    };
    assert!(!is_covered_key(opaque.key));
    assert!(opaque.period().unwrap().is_none());

    let key = format!(
        "cma1.{}.20000.{}.{}",
        "a".repeat(32),
        "b".repeat(32),
        "c".repeat(64)
    );
    let covered = Identity {
        key: &key,
        ..opaque
    };
    assert!(is_covered_key(covered.key));
    assert!(matches!(covered.period(), Err(IdentityError::Mismatch)));
}
