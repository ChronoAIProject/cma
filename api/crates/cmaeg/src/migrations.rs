//! Event Gateway migration entry points and immutable service-local history.
use cma_service_migrations::{Manifest, Release, Result, Runner};
use mongodb::{
    bson::{doc, Document},
    Database,
};

pub const LEDGER: &str = "eg_migration_ledger";
const BASE: &str = "gateway-base-v2";
const CHANNEL_FLOOR: &str = "gateway-channel-create-fence-floor-v1";
const MANIFESTS: &[Manifest] = &[
    Manifest {id:BASE, prerequisites:&[], schema_fingerprint:[0;32], definition:"browser-session expiry; sealed channel, connection and interaction validators; owner lookup and interaction expiry; schema v2 and channel guard v1"},
    Manifest {id:"gateway-replicas-v1", prerequisites:&[BASE], schema_fingerprint:[0;32], definition:"base v2; connection and interaction partial replica discovery indexes; replica marker v1"},
    Manifest {id:"gateway-activity-history-v1", prerequisites:&[], schema_fingerprint:[0;32], definition:"activity events, chunks, scopes and control validators; expiry, scope order and expiry indexes; capture marker v1"},
    Manifest {id:"gateway-console-sessions-v1", prerequisites:&[], schema_fingerprint:[0;32], definition:"console session and quota validator; expiry, subject creation and activity indexes; quota marker v1"},
    Manifest {id:"gateway-automation-v6", prerequisites:&[BASE], schema_fingerprint:[0;32], definition:"automation validator; owner kind, receipt expiry and dispatch recovery indexes; missing delivery expiry and dispatch deadline backfill; marker v6"},
    Manifest {id:"gateway-worker-credentials-v1", prerequisites:&[BASE], schema_fingerprint:[0;32], definition:"worker use and cutover validators; no TTL or secondary indexes; marker v1"},
    Manifest {id:"gateway-thread-fences-v1", prerequisites:&[BASE], schema_fingerprint:[0;32], definition:"thread fence validator and owner ID index; marker v1"},
    Manifest {id:"gateway-media-continuations-v1", prerequisites:&[BASE], schema_fingerprint:[0;32], definition:"media continuation validator, owner ID and expiry indexes; marker v1"},
    Manifest {id:CHANNEL_FLOOR, prerequisites:&[BASE], schema_fingerprint:[0;32], definition:"Channel writer-floor authority; guarded validator cutover; retained Channel revision backfill and marker v1"},
    Manifest {id:"gateway-whatsapp-forms-v1", prerequisites:&[BASE], schema_fingerprint:[0;32], definition:"WhatsApp form validator and owner ID index; marker v1"},
];
async fn runner(db: &Database) -> Result<Runner> {
    let mut manifests = MANIFESTS.to_vec();
    for manifest in &mut manifests {
        manifest.schema_fingerprint =
            cma_service_migrations::describe(converge(db, *manifest)).await?;
    }
    Ok(Runner::new(db, LEDGER, "cma-event-gateway", &manifests))
}
pub fn migration_role(role: Document, database: &str) -> Document {
    cma_service_migrations::migration_role(role, database, LEDGER)
}
fn id(command: &str) -> Result<&'static str> {
    match command {
        "--migrate" => Ok(BASE),
        "--migrate-replicas" => Ok("gateway-replicas-v1"),
        "--migrate-activity-history" => Ok("gateway-activity-history-v1"),
        "--migrate-console-sessions" => Ok("gateway-console-sessions-v1"),
        "--migrate-automation" => Ok("gateway-automation-v6"),
        "--migrate-worker-credentials" => Ok("gateway-worker-credentials-v1"),
        "--migrate-thread-fences" => Ok("gateway-thread-fences-v1"),
        "--migrate-media-continuations" => Ok("gateway-media-continuations-v1"),
        "--migrate-channel-create-fence-floor" => Ok(CHANNEL_FLOOR),
        "--migrate-whatsapp-forms" => Ok("gateway-whatsapp-forms-v1"),
        _ => Err("unknown legacy migration command"),
    }
}
async fn converge(db: &Database, manifest: Manifest) -> Result<()> {
    converge_with_issuer(db, manifest, None).await
}

async fn converge_with_issuer(
    db: &Database,
    manifest: Manifest,
    writer_issuer: Option<&str>,
) -> Result<()> {
    let unavailable = |_| "Gateway schema migration failed; details withheld";
    use crate::records::mongo;
    if !matches!(
        manifest.id,
        "gateway-activity-history-v1" | "gateway-console-sessions-v1"
    ) {
        cma_service_migrations::marker_history(db,mongo::MIGRATIONS,vec![
            doc! {"_id":"schema","version":{"$in":[1,2]}},
            doc! {"_id":{"$in":["replicas","thread_fences","whatsapp_forms","media_continuations","worker_credentials","worker_tracking_cutover","github_channel_guard"]},"version":1},
            doc! {"_id":"github_integrations","version":{"$in":[1,2,3,4,5,6]}},
            doc! {"_id":"channel_create_fences","version":1},
            doc! {"_id":"channel_validator_change","version":{"$in":[1,2,3]}},
        ]).await.map_err(|_|"Gateway legacy migration history unavailable or unsupported")?;
    }
    match manifest.id {
        BASE => mongo::migrate(db).await.map_err(unavailable),
        "gateway-replicas-v1" => mongo::migrate_replicas(db).await.map_err(unavailable),
        "gateway-activity-history-v1" => crate::activity::history::schema::migrate(db)
            .await
            .map_err(|_| "Gateway activity history migration failed"),
        "gateway-console-sessions-v1" => crate::console::session_backend::schema::migrate(db)
            .await
            .map_err(|_| "Gateway Console session migration failed"),
        "gateway-automation-v6" => crate::github::migrate(db)
            .await
            .map_err(|_| "Gateway automation migration failed"),
        "gateway-worker-credentials-v1" => crate::worker_credentials::mongo::migrate(db)
            .await
            .map_err(unavailable),
        "gateway-thread-fences-v1" => mongo::migrate_thread_fences(db).await.map_err(unavailable),
        "gateway-media-continuations-v1" => mongo::migrate_media_continuations(db)
            .await
            .map_err(unavailable),
        CHANNEL_FLOOR => {
            let issuer = if cma_service_migrations::describing() {
                "https://migration.invalid".to_owned()
            } else if let Some(issuer) = writer_issuer {
                issuer.to_owned()
            } else {
                channel_floor_issuer()?
            };
            mongo::migrate_create_fence_floor(db, &issuer)
                .await
                .map_err(|_| {
                    "Gateway Channel create fence floor migration failed; details withheld"
                })
        }
        "gateway-whatsapp-forms-v1" => mongo::migrate_whatsapp_flows(db).await.map_err(unavailable),
        _ => Err("unknown Gateway migration"),
    }
}
fn channel_floor_issuer() -> Result<String> {
    std::env::var(crate::CMAEG_NYXID_BASE_URL)
        .map(|issuer| issuer.trim_end_matches('/').to_owned())
        .map_err(|_| "CMAEG_NYXID_BASE_URL is required for the Channel create fence floor")
}

async fn inspect(db: &Database, manifest: Manifest) -> Result<bool> {
    if manifest.id == CHANNEL_FLOOR {
        return inspect_channel_floor(db, &channel_floor_issuer()?).await;
    }
    cma_service_migrations::inspect(converge(db, manifest)).await
}

async fn inspect_channel_floor(db: &Database, issuer: &str) -> Result<bool> {
    crate::records::mongo::inspect_create_fence_floor_migration(db, issuer)
        .await
        .map_err(|_| "Gateway Channel create fence floor inspection failed")
}

async fn apply_channel_floor(
    runner: &Runner,
    db: &Database,
    checksum: &str,
    release: Release<'_>,
    issuer: &str,
    acknowledgement: &[&str],
) -> Result<serde_json::Value> {
    if acknowledgement != ["--all-writers-drained"] {
        return Err("Channel create fence floor requires --all-writers-drained");
    }
    let manifest = runner.manifest(CHANNEL_FLOOR)?;
    runner
        .apply(
            manifest,
            checksum,
            release,
            || inspect_channel_floor(db, issuer),
            || converge_with_issuer(db, manifest, Some(issuer)),
        )
        .await
}

pub async fn legacy(db: &Database, command: &str) -> Result<()> {
    let runner = runner(db).await?;
    let manifest = runner.manifest(id(command)?)?;
    runner
        .legacy(manifest, || inspect(db, manifest), converge(db, manifest))
        .await
}
pub(crate) async fn legacy_startup(db: &Database, command: &str) -> Result<()> {
    let runner = runner(db).await?;
    let manifest = runner.manifest(id(command)?)?;
    runner
        .legacy_startup(manifest, || inspect(db, manifest), converge(db, manifest))
        .await
}
pub async fn cli(db: &Database, args: &[&str], source: &str) -> Result<()> {
    let runner = runner(db).await?;
    match args {
        ["--migration-plan", id] => {
            let manifest = runner.manifest(id)?;
            println!(
                "{}",
                runner.plan(manifest, inspect(db, manifest).await?).await?
            );
        }
        ["--migration-status", id] => println!("{}", runner.status(runner.manifest(id)?).await?),
        ["--migration-activate", evidence] => runner.activate(evidence).await?,
        ["--migration-apply", CHANNEL_FLOOR, checksum, image, acknowledgement @ ..] => {
            let release = Release { source, image };
            println!(
                "{}",
                apply_channel_floor(
                    &runner,
                    db,
                    checksum,
                    release,
                    &channel_floor_issuer()?,
                    acknowledgement,
                )
                .await?
            );
        }
        ["--migration-apply", id, checksum, image] => {
            let manifest = runner.manifest(id)?;
            let release = Release { source, image };
            println!(
                "{}",
                runner
                    .apply(
                        manifest,
                        checksum,
                        release,
                        || inspect(db, manifest),
                        || converge(db, manifest)
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
    #[tokio::test]
    async fn manifests_are_described_without_database_io() {
        let client =
            mongodb::Client::with_uri_str("mongodb://127.0.0.1:1/?serverSelectionTimeoutMS=1")
                .await
                .unwrap();
        let db = client.database("fixture");
        let first = runner(&db).await.unwrap();
        let second = runner(&db).await.unwrap();
        for manifest in MANIFESTS {
            let actual = first.manifest(manifest.id).unwrap();
            assert_ne!(actual.schema_fingerprint, [0; 32]);
            assert_eq!(
                actual.checksum(),
                second.manifest(manifest.id).unwrap().checksum()
            );
        }
    }
}

#[cfg(test)]
mod mongo_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
    async fn legacy_gateway_command_and_nested_replica_refuse_after_cutover() {
        let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let db = client.database(&format!(
            "gateway_migration_fixture_{}",
            uuid::Uuid::new_v4().simple()
        ));
        legacy(&db, "--migrate").await.unwrap();
        let runner = runner(&db).await.unwrap();
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
        runner
            .activate("fixture legacy command drained and old execution authority retired")
            .await
            .unwrap();
        assert!(legacy(&db, "--migrate").await.is_err());
        assert!(legacy(&db, "--migrate-replicas").await.is_err());
        let markers = db.collection::<Document>(crate::records::mongo::MIGRATIONS);
        assert!(markers
            .find_one(doc! {"_id":"replicas"})
            .await
            .unwrap()
            .is_none());
        let base = runner.manifest(BASE).unwrap();
        let release = || {
            Release{source:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",image:"example.invalid/cma-event-gateway@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
        };
        let issuer = "https://issuer.example";
        let floor = runner.manifest(CHANNEL_FLOOR).unwrap();
        let checksum = floor.checksum();
        assert_eq!(
            apply_channel_floor(&runner, &db, &checksum, release(), issuer, &[])
                .await
                .unwrap_err(),
            "Channel create fence floor requires --all-writers-drained"
        );
        let drained = ["--all-writers-drained"];
        assert_eq!(
            apply_channel_floor(&runner, &db, &checksum, release(), issuer, &drained)
                .await
                .unwrap_err(),
            "migration prerequisite is incomplete"
        );
        assert!(markers
            .find_one(doc! {"_id":"channel_validator_change"})
            .await
            .unwrap()
            .is_none());
        runner
            .apply(
                base,
                &base.checksum(),
                release(),
                || cma_service_migrations::inspect(converge(&db, base)),
                || converge(&db, base),
            )
            .await
            .unwrap();
        let applied = apply_channel_floor(&runner, &db, &checksum, release(), issuer, &drained)
            .await
            .unwrap();
        assert_eq!(applied["outcome"], "completed");
        let replay = apply_channel_floor(&runner, &db, &checksum, release(), issuer, &drained)
            .await
            .unwrap();
        assert_eq!(replay["outcome"], "already_completed");
        let replicas = runner.manifest("gateway-replicas-v1").unwrap();
        runner
            .apply(
                replicas,
                &replicas.checksum(),
                release(),
                || cma_service_migrations::inspect(converge(&db, replicas)),
                || converge(&db, replicas),
            )
            .await
            .unwrap();
        assert!(markers
            .find_one(doc! {"_id":"replicas","version":1})
            .await
            .unwrap()
            .is_some());
        db.run_command(doc! {
            "collMod":"eg_channel_create_fences",
            "validator":{"revision":{"$exists":true}},
            "validationLevel":"strict",
            "validationAction":"error",
        })
        .await
        .unwrap();
        assert_eq!(
            apply_channel_floor(&runner, &db, &checksum, release(), issuer, &drained)
                .await
                .unwrap_err(),
            "Gateway Channel create fence floor inspection failed"
        );
        db.drop().await.unwrap();
    }
}

#[cfg(test)]
#[path = "migrations_drift_tests.rs"]
mod drift_tests;
