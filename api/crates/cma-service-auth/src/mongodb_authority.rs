//! Database credential policy is explicit. Shared-schema deployments retain
//! service-owned collections but do not claim database account isolation.
use mongodb::bson::Document;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MongoAuthorityMode {
    #[default]
    Isolated,
    SharedSchema,
}

impl MongoAuthorityMode {
    pub fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        match value {
            None | Some("isolated") => Ok(Self::Isolated),
            Some("shared-schema") => Ok(Self::SharedSchema),
            Some(_) => Err("MongoDB authority mode must be isolated or shared-schema"),
        }
    }

    pub fn from_env(key: &str) -> Result<Self, &'static str> {
        match std::env::var(key) {
            Ok(value) => Self::parse(Some(&value)),
            Err(std::env::VarError::NotPresent) => Self::parse(None),
            Err(_) => Err("MongoDB authority mode must be valid UTF-8"),
        }
    }
}

/// Verify required operations against actual effective grants. A shared account
/// may access other collections in the selected database; cluster/global and
/// other-database authorities are still refused. This does not rewrite grants.
pub fn verify_shared_schema(
    status: &Document,
    database: &str,
    required_role: &Document,
) -> Result<(), &'static str> {
    const INVALID: &str = "shared-schema MongoDB credential lacks the required database authority";
    let info = status.get_document("authInfo").map_err(|_| INVALID)?;
    if info
        .get_array("authenticatedUsers")
        .map_err(|_| INVALID)?
        .len()
        != 1
    {
        return Err(INVALID);
    }
    let mut grants = BTreeMap::<String, BTreeSet<String>>::new();
    for grant in info
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| INVALID)?
    {
        let grant = grant.as_document().ok_or(INVALID)?;
        let resource = grant.get_document("resource").map_err(|_| INVALID)?;
        if resource.len() != 2 || resource.get_str("db").ok() != Some(database) {
            return Err(INVALID);
        }
        let collection = resource.get_str("collection").map_err(|_| INVALID)?;
        for action in grant.get_array("actions").map_err(|_| INVALID)? {
            grants
                .entry(collection.into())
                .or_default()
                .insert(action.as_str().ok_or(INVALID)?.into());
        }
    }
    for grant in required_role.get_array("privileges").map_err(|_| INVALID)? {
        let grant = grant.as_document().ok_or(INVALID)?;
        let resource = grant.get_document("resource").map_err(|_| INVALID)?;
        if resource.len() != 2 || resource.get_str("db").ok() != Some(database) {
            return Err(INVALID);
        }
        let collection = resource.get_str("collection").map_err(|_| INVALID)?;
        for action in grant.get_array("actions").map_err(|_| INVALID)? {
            let action = action.as_str().ok_or(INVALID)?;
            if ![collection, ""]
                .iter()
                .any(|name| grants.get(*name).is_some_and(|set| set.contains(action)))
            {
                return Err(INVALID);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn shared_schema_requires_explicit_mode_and_actual_database_grants() {
        assert_eq!(
            MongoAuthorityMode::parse(None),
            Ok(MongoAuthorityMode::Isolated)
        );
        assert_eq!(
            MongoAuthorityMode::parse(Some("shared-schema")),
            Ok(MongoAuthorityMode::SharedSchema)
        );
        for value in ["", "true", "shared", "shared-schema "] {
            assert!(MongoAuthorityMode::parse(Some(value)).is_err());
        }
        let role = doc! {"privileges":[{"resource":{"db":"cma","collection":"im_images"},"actions":["find","insert"]}]};
        let status = |resource: Document, actions: Vec<&str>| doc! {"authInfo":{"authenticatedUsers":[{"user":"runtime","db":"admin"}],"authenticatedUserPrivileges":[{"resource":resource,"actions":actions}]}};
        assert!(verify_shared_schema(
            &status(
                doc! {"db":"cma","collection":""},
                vec!["find", "insert", "collMod"]
            ),
            "cma",
            &role
        )
        .is_ok());
        assert!(verify_shared_schema(
            &status(
                doc! {"db":"cma","collection":"im_images"},
                vec!["find", "insert"]
            ),
            "cma",
            &role
        )
        .is_ok());
        for (resource, actions) in [
            (doc! {"db":"cma","collection":""}, vec!["find"]),
            (doc! {"db":"other","collection":""}, vec!["find", "insert"]),
            (
                doc! {"db":"cma","collection":"eg_channel_records"},
                vec!["find", "insert"],
            ),
            (doc! {"anyResource":true}, vec!["find", "insert"]),
        ] {
            assert!(verify_shared_schema(&status(resource, actions), "cma", &role).is_err());
        }
        let mut unauthenticated = status(doc! {"db":"cma","collection":""}, vec!["find", "insert"]);
        unauthenticated
            .get_document_mut("authInfo")
            .unwrap()
            .insert("authenticatedUsers", Vec::<Document>::new());
        assert!(verify_shared_schema(&unauthenticated, "cma", &role).is_err());
    }
}
