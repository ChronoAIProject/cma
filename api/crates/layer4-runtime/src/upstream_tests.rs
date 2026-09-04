use layer3_adapter::{CredentialFrame, RuntimeCredential};

use super::*;

#[tokio::test]
async fn holder_refuses_generation_rollback_and_conflict() {
    let holder = CredentialHolder::new();
    holder
        .install(&CredentialFrame {
            operation_id: "one".into(),
            value: RuntimeCredential::new("secret-one"),
            custody_generation: Some(2),
        })
        .await
        .unwrap();
    let rollback = holder
        .install(&CredentialFrame {
            operation_id: "two".into(),
            value: RuntimeCredential::new("secret-two"),
            custody_generation: Some(1),
        })
        .await
        .unwrap_err();
    assert_eq!(rollback.code, "credential_generation_rollback");
    let conflict = holder
        .install(&CredentialFrame {
            operation_id: "three".into(),
            value: RuntimeCredential::new("secret-three"),
            custody_generation: Some(2),
        })
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "credential_generation_conflict");
}
