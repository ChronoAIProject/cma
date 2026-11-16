//! Pause the first transaction fence only after the majority durable claim exists.
use super::*;
use mongodb::options::ReadConcern;
use std::sync::atomic::{AtomicBool, Ordering};

type Takeover = Arc<Mutex<Option<(Sandbox, Document)>>>;

pub(super) async fn install(f: &mut Fixture, old: &Sandbox) -> Takeover {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let database = f.store.agents.namespace().db;
    let parent_id = old.id.clone();
    let agent_id = f.agent.id.clone();
    let result: Takeover = Arc::default();
    let observed = result.clone();
    let fired = AtomicBool::new(false);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        if event.db != database
            || event.command.get_str("update").ok() != Some("sandboxes")
            || event.command.get_bool("startTransaction").ok() != Some(true)
            || fired.swap(true, Ordering::SeqCst)
        {
            return;
        }
        // A synchronous callback holds the original fence before send. The
        // independent runtime cannot inherit the old task-local WorkAuthority.
        let uri = uri.clone();
        let database = database.clone();
        let parent_id = parent_id.clone();
        let agent_id = agent_id.clone();
        let takeover = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(8), async {
                        let client = Client::with_uri_str(uri).await.unwrap();
                        let store = MongoSandboxStore::open(&client, &database)
                            .await
                            .unwrap()
                            .with_renewable_work(true);
                        let row = store
                            .agents
                            .find_one(doc! {"_id":agent_id.as_str()})
                            .read_concern(ReadConcern::majority())
                            .await
                            .unwrap()
                            .unwrap();
                        let marker = row
                            .get_document("keyed_publication_attempt_v1")
                            .expect("claim must already be majority durable at the barrier")
                            .clone();
                        let mut parent = store.get_sandbox(&parent_id).await.unwrap().unwrap();
                        let revision = parent.revision;
                        // Release the captured lease through the store, then acquire a real successor.
                        // Saving a shorter same-holder expiry would preserve the live renewal.
                        parent.work.lease = None;
                        parent.revision += 1;
                        store.save_sandbox(revision, parent).await.unwrap();
                        let successor = store
                            .claim_sandbox_work(&parent_id, "successor", now(), deadline())
                            .await
                            .unwrap()
                            .unwrap();
                        client.shutdown().await;
                        (successor, marker)
                    })
                    .await
                    .expect("takeover must finish before the bounded callback returns")
                })
        })
        .join()
        .expect("independent takeover completed");
        *observed.lock().unwrap() = Some(takeover);
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &f.store.agents.namespace().db)
        .await
        .unwrap()
        .with_renewable_work(true);
    f.client = client;
    f.store = store;
    result
}
