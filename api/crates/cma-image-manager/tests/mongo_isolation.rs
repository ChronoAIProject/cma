//! Actual authorization failures against a disposable authenticated MongoDB.
use cma_image_manager::{
    model::{BuildInput, BuildPhase, CreateImage, Error},
    schema,
    store::Store,
};
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client,
};
#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn isolated_credentials_preserve_owner_idempotency_shares_and_build_fences() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").expect("CMA_IM_TEST_MONGODB_URI");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_im_contract_{}", uuid::Uuid::new_v4().simple());
    let result = exercise(&uri, &admin, &name).await;
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}
async fn exercise(uri: &str, admin: &Client, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db = admin.database(name);
    db.run_command(schema::runtime_role(name)).await?;
    db.run_command(schema::migration_role(name)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    for (user, role) in [
        ("image-runtime", "cma_im_runtime"),
        ("image-migrator", "cma_im_migrator"),
    ] {
        db.run_command(doc! {"createUser":user,"pwd":&password,"roles":[{"role":role,"db":name}]})
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
    let runtime = connect("image-runtime").await?;
    let migrator = connect("image-migrator").await?;
    assert!(Store::open(runtime.database(name)).await.is_err());
    assert!(Store::open(db.clone()).await.is_err());
    schema::migrate(&migrator.database(name)).await?;
    schema::migrate(&migrator.database(name)).await?;
    let store = Store::open(runtime.database(name)).await?;
    for foreign in [
        "agents",
        "sandboxes",
        "auth_subjects",
        "gateway_channel_records",
        "eg_channel_records",
        "eg_auth_sessions",
    ] {
        db.collection::<Document>(foreign)
            .insert_one(doc! {"_id":"private"})
            .await?;
        for client in [&runtime, &migrator] {
            let collection = client.database(name).collection::<Document>(foreign);
            assert!(unauthorized(
                collection.find_one(doc! {}).await.unwrap_err()
            ));
            assert!(unauthorized(
                collection
                    .insert_one(doc! {"_id":"attempt"})
                    .await
                    .unwrap_err()
            ));
        }
    }
    assert!(unauthorized(
        runtime
            .database(name)
            .run_command(doc! {"collMod":schema::IMAGES,"validationLevel":"off"})
            .await
            .unwrap_err()
    ));
    let input = CreateImage {
        name: "test-image".into(),
        description: "".into(),
    };
    let accepted = futures_util::future::join_all(
        (0..12).map(|_| store.create_image("alice", "one", input.clone())),
    )
    .await;
    let images = accepted.into_iter().collect::<Result<Vec<_>, _>>()?;
    let image = &images[0];
    assert!(images.iter().all(|i| i.id == image.id));
    assert!(matches!(
        store.image("bob", &image.id, false).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store
            .create_image(
                "alice",
                "one",
                CreateImage {
                    name: "changed".into(),
                    description: "".into()
                }
            )
            .await,
        Err(Error::Conflict)
    ));
    let image = store
        .share("alice", &image.id, image.revision, "bob", true)
        .await?;
    store.image("bob", &image.id, false).await?;
    let source = BuildInput {
        repository: "https://github.com/example/runtime".into(),
        commit: "a".repeat(40),
        dockerfile: "Dockerfile".into(),
        context: ".".into(),
        dockerfile_content: None,
    };
    assert!(store
        .create_build("bob", &image.id, "build", source.clone())
        .await
        .is_err());
    let build = store
        .create_build("alice", &image.id, "build", source.clone())
        .await?;
    assert_eq!(
        build.id,
        store
            .create_build("alice", &image.id, "build", source.clone())
            .await?
            .id
    );
    let claims = futures_util::future::join_all((0..12).map(|_| store.claim_build())).await;
    let claims = claims
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 1);
    let restarted = Store::open(runtime.database(name)).await?;
    assert!(restarted.claim_build().await?.is_none());
    assert_eq!(restarted.pending().await?[0].id, build.id);
    let cloud = uuid::Uuid::new_v4().to_string();
    assert!(
        store
            .observe_build(&claims[0], &cloud, BuildPhase::Running, None)
            .await?
    );
    assert!(
        !store
            .observe_build(&claims[0], &cloud, BuildPhase::Running, None)
            .await?
    );
    let running = store.build("alice", &build.id).await?;
    let mut forged = running.clone();
    forged.cloud_id = None;
    assert!(
        !store
            .observe_build(
                &forged,
                &uuid::Uuid::new_v4().to_string(),
                BuildPhase::Running,
                None
            )
            .await?
    );
    assert!(
        store
            .observe_build(
                &running,
                &cloud,
                BuildPhase::Succeeded,
                Some(&format!("sha256:{}", "a".repeat(64)))
            )
            .await?
    );
    store.resolve("bob", &image.id, &build.id).await?;
    let image = store.image("alice", &image.id, true).await?;
    let image = store
        .share("alice", &image.id, image.revision, "bob", false)
        .await?;
    assert!(store.resolve("bob", &image.id, &build.id).await.is_err());
    let rejected = store
        .create_build("alice", &image.id, "rejected", source.clone())
        .await?;
    let dispatch = store.claim_build().await?.unwrap();
    assert_eq!(dispatch.id, rejected.id);
    store.defer_observation(&dispatch).await?;
    assert!(!store.reject_dispatch(&dispatch).await?);
    let dispatch = store.build("alice", &rejected.id).await?;
    assert!(store.reject_dispatch(&dispatch).await?);
    assert_eq!(
        store.build("alice", &rejected.id).await?.phase,
        BuildPhase::Failed
    );
    assert!(store.claim_build().await?.is_none());
    let image = store.image("alice", &image.id, true).await?;
    let image = store.retire("alice", &image.id, image.revision).await?;
    assert!(store.resolve("alice", &image.id, &build.id).await.is_err());
    assert!(store
        .create_build("alice", &image.id, "new", source)
        .await
        .is_err());
    assert_eq!(
        store
            .image_builds("alice", &image.id, true, "")
            .await?
            .len(),
        1
    );
    Ok(())
}
fn unauthorized(error: mongodb::error::Error) -> bool {
    matches!(error.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code==13)
}
