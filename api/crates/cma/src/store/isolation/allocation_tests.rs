use super::*;

fn status(grants: Vec<mongodb::bson::Bson>) -> Document {
    doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture"}],"authenticatedUserPrivileges":grants}}
}

#[test]
fn allocation_namespace_grants_are_optional_but_exact_and_confined() {
    let full = role("cma").get_array("privileges").unwrap().clone();
    let legacy = required_role("cma")
        .get_array("privileges")
        .unwrap()
        .clone();
    // Separately activated collections do not change existing runtime authority.
    assert_eq!(full.len(), legacy.len() + OPTIONAL_COLLECTIONS.len());
    validate(&status(full.clone()), "cma").unwrap();
    validate(&status(legacy.clone()), "cma").unwrap();
    cma_service_auth::mongodb_authority::verify_shared_schema(
        &status(legacy),
        "cma",
        &required_role("cma"),
    )
    .unwrap();
    for action in ACTIONS {
        let mut partial = full.clone();
        let grant = partial
            .iter_mut()
            .find(|grant| {
                grant
                    .as_document()
                    .unwrap()
                    .get_document("resource")
                    .unwrap()
                    .get_str("collection")
                    .unwrap()
                    == super::super::allocation_buckets::COLLECTION
            })
            .unwrap()
            .as_document_mut()
            .unwrap();
        grant
            .get_array_mut("actions")
            .unwrap()
            .retain(|value| value.as_str() != Some(action));
        assert!(validate(&status(partial), "cma").is_err());
    }
    let mut excessive = full;
    excessive.push(doc! {"resource":{"db":"cma","collection":super::super::allocation_buckets::COLLECTION},"actions":["dropCollection"]}.into());
    assert!(validate(&status(excessive), "cma").is_err());
}

pub(super) async fn verify_runtime() {
    use crate::store::usage_reports::ReportingStore;
    use mongodb::{
        options::{ClientOptions, Credential},
        Client,
    };
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("allocation_authority_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(required_role(&name)).await.unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"allocation-runtime","pwd":&password,
    "roles":[{"role":"cma_am_runtime","db":&name}]})
        .await
        .unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("allocation-runtime".to_owned())
            .password(password.clone())
            .source(name.clone())
            .build(),
    );
    let client = Client::with_options(options).unwrap();
    let runtime_db = client.database(&name);
    verify(&runtime_db).await.unwrap();
    let mut runtime_uri = url::Url::parse(&uri).unwrap();
    runtime_uri.set_username("allocation-runtime").unwrap();
    runtime_uri.set_password(Some(&password)).unwrap();
    runtime_uri.set_path(&name);
    runtime_uri.set_query(Some("directConnection=true"));
    let store = ReportingStore::connect(runtime_uri.as_str(), &name, &[37; 32])
        .await
        .unwrap();
    store.initialize_allocation_buckets(false).await.unwrap();
    assert!(!db
        .list_collection_names()
        .await
        .unwrap()
        .contains(&super::super::allocation_buckets::COLLECTION.into()));
    assert!(store.initialize_allocation_buckets(true).await.is_err());
    let grants = role(&name);
    let optional = grants
        .get_array("privileges")
        .unwrap()
        .iter()
        .find(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                == super::super::allocation_buckets::COLLECTION
        })
        .unwrap()
        .clone();
    db.run_command(doc! {"grantPrivilegesToRole":"cma_am_runtime","privileges":[optional]})
        .await
        .unwrap();
    verify(&runtime_db).await.unwrap();
    store.initialize_allocation_buckets(true).await.unwrap();
    assert!(
        store
            .maintain_allocation_buckets(true, super::super::allocation_buckets::Lane::Backfill)
            .await
            .unwrap()
            .claimed
    );
    let error = runtime_db
        .collection::<Document>("im_images")
        .find_one(doc! {})
        .await
        .unwrap_err();
    assert!(matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code==13));
    db.drop().await.unwrap();
}

#[test]
fn index_operations_do_not_require_the_disabled_allocation_namespace() {
    use cma_service_auth::mongodb_authority::MongoAuthorityMode;
    for build in [false, true] {
        let role = role_with_index_operation("cma", &["activity"], build, false);
        let grants = role.get_array("privileges").unwrap().clone();
        for authority in [
            MongoAuthorityMode::Isolated,
            MongoAuthorityMode::SharedSchema,
        ] {
            verify_index_operation_authority(
                &status(grants.clone()),
                "cma",
                authority,
                &["activity"],
                build,
            )
            .unwrap();
        }
        // Optional allocation grants do not make operation grants optional.
        assert!(verify_index_operation_authority(
            &status(
                required_role("cma")
                    .get_array("privileges")
                    .unwrap()
                    .clone()
            ),
            "cma",
            MongoAuthorityMode::Isolated,
            &["activity"],
            build,
        )
        .is_err());
    }
}
