use super::*;

fn status(grants: Vec<mongodb::bson::Bson>) -> Document {
    doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture"}],
    "authenticatedUserPrivileges": grants}}
}

#[test]
fn measured_runtime_grant_is_optional_and_append_only() {
    let legacy = required_role("fixture")
        .get_array("privileges")
        .unwrap()
        .clone();
    assert!(validate(&status(legacy.clone()), "fixture").is_ok());
    let mut expanded = legacy;
    expanded.push(
        doc! {
            "resource":{"db":"fixture","collection":MEASURED_CUSTODY},
            "actions":MEASURED_CUSTODY_ACTIONS,
        }
        .into(),
    );
    assert!(validate(&status(expanded.clone()), "fixture").is_ok());
    let grant = expanded.last_mut().unwrap().as_document_mut().unwrap();
    grant
        .get_array_mut("actions")
        .unwrap()
        .push("remove".into());
    assert!(validate(&status(expanded), "fixture").is_err());
}
