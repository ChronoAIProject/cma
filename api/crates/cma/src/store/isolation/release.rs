//! Optional release protocol grants are complete as a group and never include DDL.
use super::*;
use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};

pub(super) const ACTIONS: &[&str] = &["find", "insert", "update"];

pub(crate) fn role(database: &str) -> Document {
    let mut role = super::role(database);
    let grants = role
        .get_array_mut("privileges")
        .expect("role contains privileges");
    for name in crate::release_authority::COLLECTIONS {
        grants.push(
            doc! {"resource": {"db": database, "collection": *name}, "actions": ACTIONS}.into(),
        );
    }
    role
}

pub(super) fn complete(grants: &BTreeMap<String, BTreeSet<String>>) -> bool {
    let names = crate::release_authority::COLLECTIONS;
    !names.iter().any(|name| grants.contains_key(*name))
        || names.iter().all(|name| {
            grants.get(*name).is_some_and(|actions| {
                actions.len() == ACTIONS.len()
                    && ACTIONS.iter().all(|action| actions.contains(*action))
            })
        })
}

pub(crate) async fn verify(
    database: &Database,
    authority: MongoAuthorityMode,
) -> Result<(), StoreError> {
    let status = database
        .run_command(doc! {"connectionStatus": 1, "showPrivileges": true})
        .await
        .map_err(|_| StoreError::Unavailable("cannot inspect runtime release role".into()))?;
    validate(&status, database.name(), authority)
}

fn validate(
    status: &Document,
    database: &str,
    authority: MongoAuthorityMode,
) -> Result<(), StoreError> {
    if authority == MongoAuthorityMode::Isolated {
        super::validate(status, database)?;
    }
    let mut required = role(database);
    required
        .get_array_mut("privileges")
        .expect("role contains privileges")
        .retain(|grant| {
            !super::OPTIONAL_COLLECTIONS.contains(
                &grant
                    .as_document()
                    .expect("role grant")
                    .get_document("resource")
                    .expect("role resource")
                    .get_str("collection")
                    .expect("role collection"),
            )
        });
    verify_shared_schema(status, database, &required)
        .map_err(|reason| StoreError::Unavailable(reason.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status(role: Document) -> Document {
        doc! {"authInfo": {"authenticatedUsers": [{"user": "local", "db": "local"}],
        "authenticatedUserPrivileges": role.get_array("privileges").unwrap().clone()}}
    }
    #[test]
    fn ordinary_role_stays_valid_but_cannot_enable_release_authority() {
        let ordinary = status(super::super::role("local"));
        super::super::validate(&ordinary, "local").unwrap();
        assert!(validate(&ordinary, "local", MongoAuthorityMode::Isolated).is_err());
        validate(
            &status(role("local")),
            "local",
            MongoAuthorityMode::Isolated,
        )
        .unwrap();
    }
    #[test]
    fn partial_or_destructive_release_grants_are_refused() {
        let mut partial = role("local");
        partial
            .get_array_mut("privileges")
            .unwrap()
            .retain(|grant| {
                grant
                    .as_document()
                    .unwrap()
                    .get_document("resource")
                    .unwrap()
                    .get_str("collection")
                    .unwrap()
                    != crate::release_authority::EFFECTS
            });
        assert!(super::super::validate(&status(partial), "local").is_err());
        let mut destructive = role("local");
        destructive.get_array_mut("privileges").unwrap().push(
            doc! {
            "resource": {"db": "local", "collection": crate::release_authority::HEADS},
            "actions": ["remove"]}
            .into(),
        );
        assert!(super::super::validate(&status(destructive), "local").is_err());
    }
}
