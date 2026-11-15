use super::*;

fn authority() -> DesktopOwnership {
    DesktopOwnership {
        sandbox_id: "sandbox".into(),
        session_id: "session".into(),
        runtime_id: "runtime".into(),
        boot_id: Some("boot".into()),
        epoch: 1,
        issued_at_ms: 1_000,
        expires_at_ms: 16_000,
    }
}

#[test]
fn authority_accepts_bounded_claims_and_rejects_invalid_identity_epoch_and_time() {
    assert!(authority().validate().is_ok());
    let invalid: [fn(&mut DesktopOwnership); 10] = [
        |v| v.sandbox_id.clear(),
        |v| v.runtime_id = " ".into(),
        |v| v.session_id = "x".repeat(129),
        |v| v.boot_id = Some(String::new()),
        |v| v.epoch = 0,
        |v| v.epoch = u64::MAX,
        |v| v.issued_at_ms = 0,
        |v| v.expires_at_ms = v.issued_at_ms,
        |v| v.expires_at_ms = 16_001,
        |v| v.expires_at_ms = i64::MIN,
    ];
    for change in invalid {
        let mut value = authority();
        change(&mut value);
        assert!(matches!(
            value.validate(),
            Err(crate::AdapterError::Rejected(_))
        ));
    }
    let mut value = serde_json::to_value(authority()).unwrap();
    value["unexpected"] = true.into();
    assert!(serde_json::from_value::<DesktopOwnership>(value).is_err());
}

#[test]
fn challenge_requires_a_bounded_nonce_and_original_receiver_deadline() {
    for (nonce, duration, valid) in [
        ("nonce".into(), 10_000, true),
        ("nonce".into(), 1, true),
        ("".into(), 1, false),
        (" ".into(), 1, false),
        ("n".repeat(129), 1, false),
        ("nonce".into(), 0, false),
        ("nonce".into(), 10_001, false),
    ] {
        assert_eq!(
            DesktopOwnershipChallenge {
                nonce,
                valid_for_ms: duration
            }
            .validate()
            .is_ok(),
            valid
        );
    }
    assert!(serde_json::from_str::<DesktopOwnershipChallenge>(
        r#"{"nonce":"n","valid_for_ms":1,"unexpected":true}"#
    )
    .is_err());
}

struct Unsupported;
#[async_trait::async_trait]
impl DesktopOwnershipControl for Unsupported {}

#[tokio::test]
async fn unimplemented_control_ports_fail_closed() {
    let control = Unsupported;
    let challenge = DesktopOwnershipChallenge {
        nonce: "nonce".into(),
        valid_for_ms: 10_000,
    };
    assert!(matches!(
        control.ownership_challenge().await,
        Err(crate::AdapterError::Unsupported(_))
    ));
    assert!(matches!(
        control.renew_ownership(&authority(), &challenge).await,
        Err(crate::AdapterError::Unsupported(_))
    ));
    assert!(matches!(
        control.revoke_ownership(&authority()).await,
        Err(crate::AdapterError::Unsupported(_))
    ));
}
