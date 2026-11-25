//! Image Manager's independently owned migration history and command surface.
use cma_service_migrations::{Manifest, Release, Result, Runner};
use mongodb::{bson::doc, Database};

pub const LEDGER: &str = "im_migration_ledger";
const MANIFEST: Manifest = Manifest {
    id: "image-manager-schema-v2",
    prerequisites: &[],
    schema_fingerprint:[0;32],
    definition: "browser session expiry; image and build validators; owner request uniqueness, owner ID, build phase and image ID, image visibility and shares indexes; schema v2 marker",
};
async fn converge(db: &Database) -> Result<()> {
    cma_service_migrations::marker_history(
        db,
        crate::schema::MIGRATIONS,
        vec![doc! {"_id":"schema","version":{"$in":[1,2]}}],
    )
    .await
    .map_err(|_| "Image Manager legacy migration history unavailable or unsupported")?;
    crate::schema::migrate(db)
        .await
        .map_err(|_| "Image Manager schema operation failed")
}
pub async fn cli(db: &Database, args: &[String]) -> Result<()> {
    let manifest = Manifest {
        schema_fingerprint: cma_service_migrations::describe(converge(db)).await?,
        ..MANIFEST
    };
    let runner = Runner::new(db, LEDGER, "cma-image-manager", &[manifest]);
    let arguments: Vec<_> = args.iter().map(String::as_str).collect();
    match arguments.as_slice() {
        ["--migrate"] => {
            runner
                .legacy(
                    manifest,
                    || cma_service_migrations::inspect(converge(db)),
                    converge(db),
                )
                .await?
        }
        ["--migration-plan", id] => {
            let manifest = runner.manifest(id)?;
            println!(
                "{}",
                runner
                    .plan(
                        manifest,
                        cma_service_migrations::inspect(converge(db)).await?
                    )
                    .await?
            );
        }
        ["--migration-status", id] => println!("{}", runner.status(runner.manifest(id)?).await?),
        ["--migration-activate", evidence] => runner.activate(evidence).await?,
        ["--migration-apply", id, checksum, image] => {
            let manifest = runner.manifest(id)?;
            let release = Release {
                source: option_env!("CHRONO_BUILD_SHA").unwrap_or("development"),
                image,
            };
            println!(
                "{}",
                runner
                    .apply(
                        manifest,
                        checksum,
                        release,
                        || cma_service_migrations::inspect(converge(db)),
                        || converge(db)
                    )
                    .await?
            );
        }
        ["--migration-reconcile", id, attempt, fence, command, evidence] => {
            runner
                .reconcile(
                    runner.manifest(id)?,
                    attempt,
                    fence.parse().map_err(|_| "invalid fence")?,
                    command,
                    evidence,
                )
                .await?
        }
        _ => return Err("unknown migration operation"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::{
        bson::Document,
        options::{ClientOptions, Credential},
        Client,
    };
    #[tokio::test]
    #[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
    async fn adoption_refuses_unknown_validator_and_startup_incompatible_marker() {
        let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let db = admin.database(&format!(
            "image_migration_fixture_{}",
            uuid::Uuid::new_v4().simple()
        ));
        crate::schema::migrate(&db).await.unwrap();
        db.run_command(crate::schema::runtime_role(db.name()))
            .await
            .unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        db.run_command(doc! {"createUser":"fixture-runtime","pwd":&password,"roles":[{"role":"cma_im_runtime","db":db.name()}]}).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.credential = Some(
            Credential::builder()
                .username("fixture-runtime".to_owned())
                .password(password)
                .source(db.name().to_owned())
                .build(),
        );
        let runtime = Client::with_options(options).unwrap().database(db.name());
        crate::schema::verify_runtime(&runtime).await.unwrap();
        let manifest = Manifest {
            schema_fingerprint: cma_service_migrations::describe(converge(&db))
                .await
                .unwrap(),
            ..MANIFEST
        };
        db.run_command(cma_service_migrations::operator_role(db.name(), LEDGER))
            .await
            .unwrap();
        let message = db.run_command(doc! {"connectionStatus":1}).await.unwrap();
        let user = message
            .get_document("authInfo")
            .unwrap()
            .get_array("authenticatedUsers")
            .unwrap()[0]
            .as_document()
            .unwrap();
        db.client().database(user.get_str("db").unwrap()).run_command(doc! {"grantRolesToUser":user.get_str("user").unwrap(),"roles":[{"role":format!("{LEDGER}_operator"),"db":db.name()}]}).await.unwrap();
        let runner = Runner::new(&db, LEDGER, "cma-image-manager", &[manifest]);
        runner.activate("fixture reviewed drain").await.unwrap();
        let release = || {
            Release{source:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",image:"example.invalid/cma-image-manager@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
        };
        let rows = db.collection::<Document>(crate::schema::IMAGES);
        let retained = doc! {"_id":format!("img_{}",uuid::Uuid::new_v4().simple()),"schema_version":1,"owner":"fixture","request_key_hash":"a".repeat(64),"request_sha256":"b".repeat(64),"revision":1_i64,"created_at_ms":0_i64,"spec":{"name":"fixture","description":"retained"},"shares":[],"retired":false};
        rows.insert_one(retained.clone()).await.unwrap();
        let unfamiliar = doc! {"owner":{"$type":"string"}};
        db.run_command(doc! {"collMod":crate::schema::IMAGES,"validator":unfamiliar.clone()})
            .await
            .unwrap();
        assert!(runner
            .apply(
                manifest,
                &manifest.checksum(),
                release(),
                || cma_service_migrations::inspect(converge(&db)),
                || async { panic!("unrecognized validator must refuse before DDL") }
            )
            .await
            .is_err());
        let catalog = db
            .list_collections()
            .filter(doc! {"name":crate::schema::IMAGES})
            .await
            .unwrap();
        use futures_util::TryStreamExt;
        let observed: Vec<_> = catalog.try_collect().await.unwrap();
        assert_eq!(observed[0].options.validator.as_ref(), Some(&unfamiliar));
        assert_eq!(
            rows.find_one(doc! {"_id":retained.get_str("_id").unwrap()})
                .await
                .unwrap(),
            Some(retained.clone())
        );
        db.run_command(
            doc! {"collMod":crate::schema::IMAGES,"validator":crate::schema::validator(false)},
        )
        .await
        .unwrap();
        // Interrupt an adoption after an acknowledged partial schema effect, then
        // introduce an unfamiliar validator before the exact manifest resumes.
        rows.drop_index("shares_1__id_1").await.unwrap();
        assert!(runner
            .apply(
                manifest,
                &manifest.checksum(),
                release(),
                || cma_service_migrations::inspect(converge(&db)),
                || async {
                    cma_service_migrations::indexes(
                        &db,
                        crate::schema::IMAGES,
                        vec![mongodb::IndexModel::builder()
                            .keys(doc! {"shares":1,"_id":1})
                            .build()],
                    )
                    .await
                    .map_err(|_| "fixture index")?;
                    Err("fixture interruption")
                }
            )
            .await
            .is_err());
        db.run_command(doc! {"collMod":crate::schema::IMAGES,"validator":unfamiliar.clone()})
            .await
            .unwrap();
        assert!(runner
            .apply(
                manifest,
                &manifest.checksum(),
                release(),
                || cma_service_migrations::inspect(converge(&db)),
                || async { panic!("interrupted adoption must refuse unknown validator") }
            )
            .await
            .is_err());
        assert_eq!(
            rows.find_one(doc! {"_id":retained.get_str("_id").unwrap()})
                .await
                .unwrap(),
            Some(retained)
        );
        db.run_command(
            doc! {"collMod":crate::schema::IMAGES,"validator":crate::schema::validator(false)},
        )
        .await
        .unwrap();
        db.collection::<Document>(crate::schema::MIGRATIONS)
            .update_one(doc! {"_id":"schema"}, doc! {"$set":{"version":2_i64}})
            .await
            .unwrap();
        assert!(matches!(
            crate::schema::verify_runtime(&runtime).await,
            Err(crate::schema::SchemaError::MigrationRequired)
        ));
        assert!(runner
            .apply(
                manifest,
                &manifest.checksum(),
                release(),
                || cma_service_migrations::inspect(converge(&db)),
                || async { panic!("invalid marker must refuse before DDL") }
            )
            .await
            .is_err());
        assert_eq!(runner.status(manifest).await.unwrap()["completed"], false);
        db.drop().await.unwrap();
    }
}
