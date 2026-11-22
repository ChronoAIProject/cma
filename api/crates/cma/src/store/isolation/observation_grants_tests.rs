use super::*;
#[test]
fn observation_grants_are_optional_when_closed_and_required_when_enabled() {
    let mut grants = role("cma").get_array("privileges").unwrap().clone();
    let status = |grants| doc! {"authInfo": {"authenticatedUsers": [{"user": "manager", "db": "cma"}], "authenticatedUserPrivileges": grants}};
    validate_observation_grants(&status(grants.clone()), "cma").unwrap();
    grants.retain(|grant| {
        grant
            .as_document()
            .unwrap()
            .get_document("resource")
            .unwrap()
            .get_str("collection")
            .unwrap()
            != super::super::agent_observation::COLLECTION
    });
    validate(&status(grants.clone()), "cma").unwrap();
    assert!(validate_observation_grants(&status(grants), "cma").is_err());
}
