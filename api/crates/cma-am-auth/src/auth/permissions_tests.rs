use serde_json::json;

use super::{introspection_claim, permissions_claim, PermissionEvidence};

#[test]
fn permissions_require_a_complete_string_array() {
    for body in [
        json!({}),
        json!({"permissions": null}),
        json!({"permissions": "cma:sandbox:create"}),
        json!({"permissions": ["cma:sandbox:create", 1]}),
        json!({"permissions": [false]}),
    ] {
        assert!(permissions_claim(&body).is_err(), "accepted {body}");
    }
    assert_eq!(permissions_claim(&json!({"permissions": []})), Ok(vec![]));
    assert_eq!(
        permissions_claim(&json!({"permissions": ["cma:sandbox:create"]})),
        Ok(vec!["cma:sandbox:create".into()])
    );
}

#[test]
fn introspection_requires_boolean_activity_and_distinguishes_inactive() {
    for body in [
        json!({"permissions": []}),
        json!({"active": null, "permissions": []}),
        json!({"active": "true", "permissions": []}),
        json!({"active": 1, "permissions": []}),
        json!({"active": true}),
    ] {
        assert!(
            introspection_claim(&body, "owner").is_err(),
            "accepted {body}"
        );
    }
    assert_eq!(
        introspection_claim(&json!({"active": false}), "owner"),
        Ok(PermissionEvidence::Inactive)
    );
    assert_eq!(
        introspection_claim(&json!({"active": true, "permissions": []}), "owner"),
        Ok(PermissionEvidence::Active(vec![]))
    );
}

#[test]
fn optional_introspection_subject_is_exactly_bound_to_userinfo() {
    for subject in [json!("other"), json!(" owner "), json!(null), json!(5)] {
        assert!(introspection_claim(
            &json!({"active": true, "permissions": [], "sub": subject}),
            "owner"
        )
        .is_err());
    }
    assert_eq!(
        introspection_claim(
            &json!({"active": true, "permissions": [], "sub": "owner"}),
            "owner"
        ),
        Ok(PermissionEvidence::Active(vec![]))
    );
}
