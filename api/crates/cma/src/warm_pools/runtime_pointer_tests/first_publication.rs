use super::*;
use layer1_public::rest::admin::warm_pools::WarmPoolError;
use layer2_core::released_runtime::ReleasedLaneRecord;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const CRASH_DATABASE: &str = "CMA_TEST_POINTER_CRASH_DATABASE";
const CRASH_APP: &str = "CMA_TEST_POINTER_CRASH_APP";
const CRASH_TEST: &str = "warm_pools::tests::runtime_pointer::promotion::controller::first_publication::publisher_process_loss_allows_two_fixed_releases";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn publisher_process_loss_allows_two_fixed_releases() {
    if let Ok(name) = std::env::var(CRASH_DATABASE) {
        crash_publisher(&name).await;
        return;
    }
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    original.seed_defaults().await;
    let first = original
        .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
        .await;
    let pending = original
        .pools
        .candidate_target(&image(Lane::Agent, 'd'), &first)
        .unwrap();
    let pending = original.store.insert(pending).await.unwrap();
    let publisher = bind_controller(&original, |_| panic!("bootstrap reads MongoDB"), true).await;
    publisher.controller.register_boot().await.unwrap();
    let app = format!("pointer-crash-{}", uuid::Uuid::new_v4().simple());
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap().database("admin");
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
        .env_clear()
        .env("CHRONO_TEST_MONGODB_URI", &uri)
        .env("RUST_MIN_STACK", "33554432")
        .env(CRASH_DATABASE, fixture.database.name())
        .env(CRASH_APP, &app)
        .env("CMA_TEST_POINTER_TARGET", &first.id)
        .env(
            "CMA_TEST_POINTER_WRITER",
            publisher.writer.incarnation.to_string(),
        )
        .env("CMA_TEST_POINTER_POD", publisher.writer.pod_uid.to_string())
        .env("CMA_TEST_POINTER_BUILD", &publisher.writer.build_digest)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(DEADLINE, async {
        while let Some(line) = output.next_line().await.unwrap() {
            if line.contains("publisher ready for final insert") {
                return;
            }
        }
        panic!("publisher exited before its publication barrier");
    })
    .await
    .unwrap();
    let configured = admin.run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1},
        "data":{"failCommands":["insert"],"appName":&app,"blockConnection":true,"blockTimeMS":5000},
    }).await.unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    child.stdin.take().unwrap().write_all(b"!").await.unwrap();
    let reached = admin
        .run_command(
            doc! {"waitForFailPoint":"failCommand", "timesEntered":count+1,"maxTimeMS":10_000},
        )
        .await;
    child.start_kill().unwrap();
    let status = tokio::time::timeout(DEADLINE, child.wait())
        .await
        .unwrap()
        .unwrap();
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    reached.expect("publisher must reach the final lane insert");
    assert!(!status.success());
    drop(publisher);
    drop(original);

    // The publisher cannot return after this point. Withdrawal must wait for
    // the publication transaction holding the same authorization to resolve.
    let withdrew = tokio::time::timeout(
        Duration::from_secs(90),
        fixture
            .database
            .collection::<Document>("platform_settings")
            .update_one(
                doc! {"_id":format!("runtime_pointer_cutover:{DEPLOYMENT}"),"state":"fenced"},
                doc! {"$set":{"state":"withdrawn"}},
            ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(withdrew.modified_count, 1);
    assert_first_publication_absent(&fixture.database).await;
    assert_successive_fixed_releases(&fixture, &pending).await;
    fixture.database.drop().await.unwrap();
}

async fn crash_publisher(name: &str) {
    assert!(name.starts_with("cma_runtime_pointer_"));
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    assert!(options.credential.is_none() && options.repl_set_name.is_some());
    assert!(options.hosts.iter().all(
        |host| matches!(host, mongodb::options::ServerAddress::Tcp { host, .. }
        if ["127.0.0.1","localhost","::1"].contains(&host.as_str()))
    ));
    options.app_name = Some(std::env::var(CRASH_APP).unwrap());
    let database = Client::with_options(options).unwrap().database(name);
    let store = MongoNativeWarmPools::open(&database, DEPLOYMENT)
        .await
        .unwrap();
    let lanes = crate::store::released_runtimes::MongoReleasedRuntimes::open(&database, DEPLOYMENT);
    let target = store
        .find(&std::env::var("CMA_TEST_POINTER_TARGET").unwrap())
        .await
        .unwrap()
        .unwrap();
    let writer = BootWriter {
        incarnation: std::env::var("CMA_TEST_POINTER_WRITER")
            .unwrap()
            .parse()
            .unwrap(),
        pod_uid: std::env::var("CMA_TEST_POINTER_POD")
            .unwrap()
            .parse()
            .unwrap(),
        build_digest: std::env::var("CMA_TEST_POINTER_BUILD").unwrap(),
    };
    let record = ReleasedLaneRecord::bootstrap(
        ReleasedLane::Agent,
        target.plan.spec.image.uri.clone(),
        &layer2_core::now_rfc3339(),
    )
    .unwrap();
    println!("publisher ready for final insert");
    use std::io::{Read, Write};
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0_u8]).unwrap();
    store
        .publish_bootstrap(&target, &lanes, &writer, &record, uuid::Uuid::from_u128(1))
        .await
        .unwrap();
    panic!("publication unexpectedly completed before process loss");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_publication_requires_the_current_controller_cutover() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    original.seed_defaults().await;
    let publisher = bind_controller(&original, |_| panic!("bootstrap reads MongoDB"), true).await;
    publisher.controller.register_boot().await.unwrap();
    let settings = fixture.database.collection::<Document>("platform_settings");
    let key = doc! {"_id":format!("runtime_pointer_cutover:{DEPLOYMENT}")};
    let first = original
        .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
        .await;
    let record = ReleasedLaneRecord::bootstrap(
        ReleasedLane::Agent,
        image(Lane::Agent, 'a'),
        &layer2_core::now_rfc3339(),
    )
    .unwrap();
    assert!(original
        .store
        .verify_cutover(uuid::Uuid::from_u128(1), &publisher.writer)
        .await
        .is_ok());
    settings
        .update_one(key.clone(), doc! {"$set":{"state":"withdrawing"}})
        .await
        .unwrap();
    assert!(publisher.controller.bootstrap().await.is_err());
    // A publisher that already passed the preflight still cannot commit.
    assert!(original
        .store
        .publish_bootstrap(
            &first,
            &publisher.controller.release.lanes,
            &publisher.writer,
            &record,
            uuid::Uuid::from_u128(1)
        )
        .await
        .is_err());
    assert_first_publication_absent(&fixture.database).await;
    // A compatible fixed-pin writer still serves during the first rollout.
    let newer = fixture.replica_pinned_to('b').await;
    assert!(!newer.pools.ensure_fixed_defaults().await.unwrap());
    newer
        .store
        .verify_bootstrap(false, &image(Lane::Agent, 'b'))
        .await
        .unwrap();
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_pointer_orders_compatible_activation_in_both_commit_orders() {
    for fixed_first in [true, false] {
        let fixture = Fixture::new().await;
        let original = fixture.replica().await;
        original.seed_defaults().await;
        let first = original
            .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
            .await;
        fixture
            .database
            .collection::<Document>("native_warm_pool_targets")
            .update_one(
                doc! {"_id": &first.id},
                doc! {"$set":{"activated_at":DateTime::from_millis(1)}},
            )
            .await
            .unwrap();
        let publisher =
            bind_controller(&original, |_| panic!("bootstrap reads MongoDB"), true).await;
        publisher.controller.register_boot().await.unwrap();
        let record = ReleasedLaneRecord::bootstrap(
            ReleasedLane::Agent,
            image(Lane::Agent, 'a'),
            &layer2_core::now_rfc3339(),
        )
        .unwrap();
        let newer = fixture.replica_pinned_to('b').await;
        if fixed_first {
            assert!(!newer.pools.ensure_fixed_defaults().await.unwrap());
        }
        let published = original
            .store
            .publish_bootstrap(
                &first,
                &publisher.controller.release.lanes,
                &publisher.writer,
                &record,
                uuid::Uuid::from_u128(1),
            )
            .await;
        if fixed_first {
            assert!(matches!(published, Err(WarmPoolError::Conflict)));
            assert_first_publication_absent(&fixture.database).await;
            newer
                .store
                .verify_bootstrap(false, &image(Lane::Agent, 'b'))
                .await
                .unwrap();
        } else {
            published.unwrap();
            assert!(newer.pools.ensure_fixed_defaults().await.unwrap());
            let second = newer
                .target_for(&fixture.plan(Lane::Agent, 'b', capacity()))
                .await;
            assert!(second.activated_at.is_none());
            original
                .store
                .verify_bootstrap(false, &image(Lane::Agent, 'a'))
                .await
                .unwrap();
            let reopened = StoreReleasedRuntime::open(
                &newer.database,
                DEPLOYMENT,
                PinnedReleasedRuntime::new(
                    image(Lane::Agent, 'b'),
                    Some(image(Lane::Machine, '1')),
                )
                .snapshot(),
                false,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(reopened.snapshot().agent, record.active.image);
        }
        fixture.database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn refused_first_lane_insert_leaves_successive_fixed_releases_available() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    original.seed_defaults().await;
    let first = original
        .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
        .await;
    let pending = original
        .pools
        .candidate_target(&image(Lane::Agent, 'd'), &first)
        .unwrap();
    let pending = original.store.insert(pending).await.unwrap();
    let publisher = bind_controller(&original, |_| panic!("bootstrap reads MongoDB"), true).await;
    publisher.controller.register_boot().await.unwrap();
    fixture
        .database
        .run_command(doc! {"collMod":COLLECTION,
        "validator":{"_id":{"$ne":format!("{DEPLOYMENT}:agent")}}})
        .await
        .unwrap();
    assert!(publisher.controller.bootstrap().await.is_err());
    drop(publisher);
    drop(original);
    assert_first_publication_absent(&fixture.database).await;
    assert_successive_fixed_releases(&fixture, &pending).await;
    fixture.database.drop().await.unwrap();
}

async fn assert_successive_fixed_releases(fixture: &Fixture, pending: &PoolTarget) {
    for release in ['b', 'c'] {
        let fixed = fixture.replica_pinned_to(release).await;
        let pin = image(Lane::Agent, release);
        assert!(StoreReleasedRuntime::open(
            &fixed.database,
            DEPLOYMENT,
            PinnedReleasedRuntime::new(pin.clone(), Some(image(Lane::Machine, '1'))).snapshot(),
            false
        )
        .await
        .unwrap()
        .is_none());
        assert!(!fixed.pools.ensure_fixed_defaults().await.unwrap());
        fixed.store.verify_bootstrap(false, &pin).await.unwrap();
        assert!(fixed
            .store
            .find(&pending.id)
            .await
            .unwrap()
            .unwrap()
            .activated_at
            .is_none());
    }
}

async fn assert_first_publication_absent(database: &Database) {
    assert!(database
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id":format!("{DEPLOYMENT}:agent")})
        .await
        .unwrap()
        .is_none());
    assert!(database
        .collection::<Document>("native_warm_pool_targets")
        .find_one(doc! {
        "plan.deployment":DEPLOYMENT,"machine":{"$ne":true},"durable_release_authority":true})
        .await
        .unwrap()
        .is_none());
}
