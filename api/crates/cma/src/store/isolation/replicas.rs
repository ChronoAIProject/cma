//! Additive grants may be installed before enablement; only enabled managers
//! require them. The legacy role and switch-off verification stay unchanged.
use super::*;
use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};

pub(super) const COLLECTIONS: &[&str] = &[
    crate::store::agent_ownership::MEMBERS,
    crate::store::agent_ownership::OWNERS,
];

pub(crate) fn role(database: &str, enabled: bool) -> Document {
    let mut role = super::role(database);
    if enabled {
        let grants = role.get_array_mut("privileges").unwrap();
        for collection in COLLECTIONS {
            grants.push(
                doc! {
                    "resource": {"db": database, "collection": *collection},
                    "actions": ACTIONS,
                }
                .into(),
            );
        }
    }
    role
}

pub(crate) async fn verify(
    database: &Database,
    authority: MongoAuthorityMode,
    enabled: bool,
) -> Result<(), StoreError> {
    if !enabled {
        return super::verify_with_authority(database, authority).await;
    }
    let status = database
        .run_command(doc! {"connectionStatus": 1, "showPrivileges": true})
        .await
        .map_err(|_| {
            StoreError::Unavailable("cannot verify Agent Manager schema authority".into())
        })?;
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
    verify_shared_schema(status, database, &role(database, true))
        .map_err(|error| StoreError::Unavailable(error.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_grants_are_required_only_when_enabled_and_never_grant_foreign_access() {
        let status = |enabled| {
            doc! {"authInfo": {
                "authenticatedUsers": [{"user": "manager", "db": "cma"}],
                "authenticatedUserPrivileges": role("cma", enabled).get_array("privileges").unwrap().clone(),
            }}
        };
        assert_eq!(role("cma", false), super::super::role("cma"));
        for enabled in [false, true] {
            let status = status(enabled);
            super::super::validate(&status, "cma").unwrap();
            for mode in [
                MongoAuthorityMode::Isolated,
                MongoAuthorityMode::SharedSchema,
            ] {
                assert_eq!(validate(&status, "cma", mode).is_ok(), enabled);
            }
        }
        let mut foreign = status(true);
        foreign
            .get_document_mut("authInfo")
            .unwrap()
            .get_array_mut("authenticatedUserPrivileges")
            .unwrap()
            .push(
                doc! {"resource": {"db": "cma", "collection": "im_images"}, "actions": ["find"]}
                    .into(),
            );
        assert!(validate(&foreign, "cma", MongoAuthorityMode::Isolated).is_err());
    }
}
