use cma_sdk::resource_access::{EffectiveAccess, ResourceOwnership};
use serde_json::json;

#[test]
fn effective_ownership_is_safe_and_older_servers_remain_readable() {
    let policy = json!({"capabilities":["view_details"],"revalidate_at":null,"policy_revision":1});
    assert!(
        serde_json::from_value::<EffectiveAccess>(policy.clone())
            .unwrap()
            .owner
            .is_none()
    );
    for (owner, expected) in [
        (
            json!({"kind":"person","is_current_person":false}),
            ResourceOwnership::Person {
                is_current_person: false,
            },
        ),
        (
            json!({"kind":"organization","organization_id":"org"}),
            ResourceOwnership::Organization {
                organization_id: "org".into(),
            },
        ),
    ] {
        let mut response = policy.clone();
        response["owner"] = owner;
        assert_eq!(
            serde_json::from_value::<EffectiveAccess>(response)
                .unwrap()
                .owner,
            Some(expected)
        );
    }
    for owner in [
        json!({"kind":"person","subject":"private-person"}),
        json!({"kind":"organization"}),
        json!({"kind":"future"}),
    ] {
        let mut response = policy.clone();
        response["owner"] = owner;
        assert!(serde_json::from_value::<EffectiveAccess>(response).is_err());
    }
}
