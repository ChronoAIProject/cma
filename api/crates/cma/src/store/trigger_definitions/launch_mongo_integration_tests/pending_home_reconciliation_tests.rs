//! Repair only provable terminal leaks; preserve live and missing Launch receipts.
use super::home_capacity::{configure, home, pending, receipt, request, resources};
use super::*;
use crate::store::{
    launch_home_capacity::{fence_key, receipt_key},
    pending_launch_homes::{reserved_filter, PendingLaunchHomes, BATCH_SIZE, INDEX},
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};
use futures_util::TryStreamExt;
use layer2_core::admission::{AdmissionCeilings, AdmissionCeilingsStore};
use layer2_core::sandbox::store::SandboxAdmissionStore;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::Mutex;

async fn setup(prefix: &str) -> (Seeded, PublicTrigger, PendingLaunchHomes) {
    let seed = seeded(prefix).await;
    let _resources = resources(&seed).await;
    let public = published(&seed, prefix).await;
    configure(
        &seed,
        AdmissionCeilings {
            retained_homes: Some(1000),
            ..Default::default()
        },
    )
    .await;
    prepare_index(&seed.client, &seed.database).await;
    let reconciler = enabled(&seed.client, &seed.database).await.unwrap();
    (seed, public, reconciler)
}
fn admissions(seed: &Seeded) -> mongodb::Collection<Document> {
    seed.client
        .database(&seed.database)
        .collection("sandbox_admissions")
}
fn cancel_commit(mut launch: Launch) -> LaunchCommit {
    let expected_revision = launch.revision;
    let from = launch.state;
    launch.cancel(LATER.into()).unwrap();
    LaunchCommit {
        expected_revision,
        launch,
        event_at: LATER.into(),
        events: vec![
            LaunchEventKind::StateChanged {
                from,
                to: LaunchState::Cancelled,
            },
            LaunchEventKind::Cancelled,
        ],
    }
}
async fn leak(seed: &Seeded, public: &PublicTrigger, index: usize) -> Launch {
    let launch = seed
        .store
        .admit_launch(request(public, index))
        .await
        .unwrap()
        .launch()
        .clone();
    // Reproduce a predecessor whole-record replacement discarding this marker.
    seed.client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES)
        .update_one(
            doc! {"_id":launch.id.as_str()},
            doc! {"$unset":{"home_capacity_admission":""}},
        )
        .await
        .unwrap();
    seed.store
        .commit_launch(cancel_commit(launch))
        .await
        .unwrap()
}
async fn monitored_client() -> (Client, Arc<Mutex<Vec<Document>>>) {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let trace = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            trace.lock().unwrap().push(event.command);
        }
    }));
    (Client::with_options(options).unwrap(), commands)
}
fn writes(commands: &[Document]) -> Vec<&Document> {
    commands
        .iter()
        .filter(|command| {
            ["insert", "update", "delete", "findAndModify"]
                .iter()
                .any(|name| command.contains_key(*name))
        })
        .collect()
}

async fn enabled(
    client: &Client,
    database: &str,
) -> Result<PendingLaunchHomes, layer2_core::StoreError> {
    let admissions = client
        .database(database)
        .collection::<Document>("sandbox_admissions");
    let active = crate::store::registration_capacity::activation::read(&admissions, None)
        .await
        .map_err(crate::store::registration_capacity::activation::Error::into_store_error)?;
    Ok(PendingLaunchHomes::new(client, database)
        .with_enabled(true)
        .with_activation(Arc::new(std::sync::atomic::AtomicBool::new(active))))
}
async fn prepare_index(client: &Client, database: &str) {
    client
        .database(database)
        .collection::<Document>("sandbox_admissions")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"state":1,"_id":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name(INDEX.to_owned())
                        .partial_filter_expression(reserved_filter())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
}

#[path = "pending_home_reconciliation/repair.rs"]
mod repair;

#[path = "pending_home_reconciliation/observation.rs"]
mod observation;

#[path = "pending_home_reconciliation/correction.rs"]
mod correction;

#[path = "pending_home_reconciliation/activation.rs"]
mod activation;

#[path = "pending_home_reconciliation/faults.rs"]
mod faults;

#[path = "pending_home_reconciliation/terminal.rs"]
mod terminal;

#[path = "pending_home_reconciliation/events.rs"]
mod events;

#[path = "pending_home_reconciliation/policy.rs"]
mod policy;
