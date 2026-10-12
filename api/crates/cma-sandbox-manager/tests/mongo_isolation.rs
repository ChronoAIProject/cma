//! Run explicitly against a disposable authenticated MongoDB using
//! CMA_SM_TEST_MONGODB_URI. Every fixture lives in one uniquely named database.

use cma_sandbox_manager::{
    model::{Change, CreateSandbox, Phase, Resources},
    schema,
    store::{SandboxStore, StoreError},
};
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client,
};

#[tokio::test]
#[ignore = "requires an authenticated disposable MongoDB; run the sandbox-manager Mongo contract script"]
async fn real_credentials_enforce_isolation_idempotency_and_restart_fences() {
    let uri =
        std::env::var("CMA_SM_TEST_MONGODB_URI").expect("CMA_SM_TEST_MONGODB_URI is required");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_sm_contract_{}", uuid::Uuid::new_v4().simple());
    let database = admin.database(&name);
    let result = exercise(&uri, &admin, &name).await;
    database
        .drop()
        .await
        .expect("remove the exact isolated test database");
    result.unwrap();
}

async fn exercise(uri: &str, admin: &Client, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let database = admin.database(name);
    database.run_command(schema::runtime_role(name)).await?;
    database.run_command(schema::migration_role(name)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    for (user, role) in [
        ("sandbox-runtime", "cma_sm_runtime"),
        ("sandbox-migrator", "cma_sm_migrator"),
    ] {
        database
            .run_command(doc! {
                "createUser": user, "pwd": &password, "roles": [{ "role": role, "db": name }]
            })
            .await?;
    }
    let connect = |user: &'static str| {
        let password = password.clone();
        async move {
            let mut options = ClientOptions::parse(uri).await?;
            options.credential = Some(
                Credential::builder()
                    .username(user.to_owned())
                    .password(password)
                    .source(name.to_owned())
                    .build(),
            );
            Client::with_options(options)
        }
    };
    let migrator = connect("sandbox-migrator").await?;
    let runtime = connect("sandbox-runtime").await?;
    // Runtime cannot silently initialize its schema, and a global admin URI
    // cannot be reused as this service's runtime credential.
    assert!(SandboxStore::open(&runtime, name).await.is_err());
    assert!(SandboxStore::open(admin, name).await.is_err());
    schema::migrate(&migrator.database(name)).await?;
    schema::migrate(&migrator.database(name)).await?;
    let store = SandboxStore::open(&runtime, name).await?;
    for foreign in [
        "am_agents",
        "am_messages",
        "eg_channels",
        "sandboxes",
        "agents",
    ] {
        let collection = database.collection::<Document>(foreign);
        collection.insert_one(doc! { "_id": "private" }).await?;
        let read = runtime
            .database(name)
            .collection::<Document>(foreign)
            .find_one(doc! {})
            .await;
        assert!(
            unauthorized(read.unwrap_err()),
            "read was not denied for {foreign}"
        );
        let write = runtime
            .database(name)
            .collection::<Document>(foreign)
            .insert_one(doc! { "_id": "attempt" })
            .await;
        assert!(
            unauthorized(write.unwrap_err()),
            "write was not denied for {foreign}"
        );
        let migrate_read = migrator
            .database(name)
            .collection::<Document>(foreign)
            .find_one(doc! {})
            .await;
        assert!(
            unauthorized(migrate_read.unwrap_err()),
            "migrator read was not denied for {foreign}"
        );
    }
    assert!(unauthorized(
        runtime
            .database(name)
            .run_command(doc! {
                "collMod": schema::SANDBOXES, "validationLevel": "off"
            })
            .await
            .unwrap_err()
    ));

    let request = CreateSandbox {
        name: "plain-linux".into(),
        image: "alpine:3.22".into(),
        entrypoint: vec!["sleep".into(), "60".into()],
        resources: Resources {
            cpu_millis: 250,
            memory_mib: 256,
        },
        env: Default::default(),
        client_ref: None,
    };
    let results = futures_util::future::join_all((0..12).map(|_| {
        let store = store.clone();
        let request = request.clone();
        async move { store.admit("alice", "same-request", request, 1).await }
    }))
    .await;
    let mut accepted = Vec::new();
    for result in results {
        accepted.push(result?);
    }
    let first = accepted.first().unwrap();
    assert!(accepted.iter().all(|s| s.id == first.id));
    assert_eq!(store.list("alice", None, 100).await?.len(), 1);
    assert!(matches!(
        store.get("bob", &first.id).await,
        Err(StoreError::NotFound)
    ));
    let bob = store
        .admit("bob", "same-request", request.clone(), 1)
        .await?;
    assert_ne!(first.id, bob.id);
    let mut caller_copy = bob.clone();
    caller_copy.spec.image = "different/image:changed".into();
    let unchanged = store
        .transition(
            &caller_copy,
            Change::Dispatch {
                id: "bob-dispatch".into(),
            },
            2,
        )
        .await?;
    assert_eq!(
        unchanged.spec, bob.spec,
        "transition trusted caller-supplied immutable fields"
    );
    let mut changed = request;
    changed.resources.cpu_millis = 500;
    assert!(matches!(
        store.admit("alice", "same-request", changed, 1).await,
        Err(StoreError::IdempotencyConflict)
    ));

    let results = futures_util::future::join_all((0..12).map(|i| {
        let store = store.clone();
        let first = first.clone();
        async move {
            store
                .transition(
                    &first,
                    Change::Dispatch {
                        id: format!("dispatch-{i}"),
                    },
                    2,
                )
                .await
        }
    }))
    .await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::Conflict)))
            .count(),
        11
    );
    let recovered = SandboxStore::open(&runtime, name).await?;
    let durable = recovered.get("alice", &first.id).await?;
    assert_eq!(durable.phase, Phase::Dispatching);
    assert!(recovered
        .transition(
            &durable,
            Change::Dispatch {
                id: "new-dispatch".into()
            },
            3
        )
        .await
        .is_err());
    assert!(recovered
        .transition(&durable, Change::Terminate, 3)
        .await
        .is_err());
    assert!(recovered
        .pending(100)
        .await?
        .iter()
        .any(|s| s.id == first.id));

    let malformed = runtime
        .database(name)
        .collection::<Document>(schema::SANDBOXES)
        .update_one(
            doc! { "_id": &first.id },
            doc! { "$set": { "agent_model": "must-not-belong-here" } },
        )
        .await;
    assert!(
        malformed.is_err(),
        "schema accepted Agent configuration on a Sandbox"
    );
    let malformed = runtime
        .database(name)
        .collection::<Document>(schema::SANDBOXES)
        .update_one(
            doc! { "_id": &first.id },
            doc! { "$set": { "spec.model": "must-not-belong-here" } },
        )
        .await;
    assert!(
        malformed.is_err(),
        "schema accepted Agent configuration inside the environment spec"
    );
    Ok(())
}

fn unauthorized(error: mongodb::error::Error) -> bool {
    matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(e) if e.code == 13)
}
