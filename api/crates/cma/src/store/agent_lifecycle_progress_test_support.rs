use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};

use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use tokio::sync::Notify;

#[derive(Default)]
pub(super) struct Probe {
    pub commands: Mutex<Vec<(String, mongodb::bson::Document)>>,
    armed: AtomicBool,
    entered: Notify,
    released: Mutex<bool>,
    resume: Condvar,
}

impl Probe {
    pub fn arm(&self) {
        *self.released.lock().unwrap() = false;
        self.armed.store(true, Ordering::SeqCst);
    }

    pub async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.notified())
            .await
            .unwrap();
    }

    pub fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.resume.notify_one();
    }

    pub fn take_commands(&self) -> Vec<(String, mongodb::bson::Document)> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }
}

pub(super) struct Fixture {
    pub db: Database,
    pub store: MongoAgentLifecycleStore,
    pub lease: LifecycleLease,
    pub probe: Arc<Probe>,
}

impl Fixture {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set URI");
        let probe = Arc::new(Probe::default());
        let observe = probe.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| match event {
            CommandEvent::Started(event)
                if matches!(event.command_name.as_str(), "find" | "update") =>
            {
                observe
                    .commands
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
            }
            CommandEvent::Succeeded(event)
                if event.command_name == "find" && observe.armed.swap(false, Ordering::SeqCst) =>
            {
                observe.entered.notify_one();
                let (_released, timeout) = observe
                    .resume
                    .wait_timeout_while(
                        observe.released.lock().unwrap(),
                        Duration::from_secs(10),
                        |released| !*released,
                    )
                    .unwrap();
                assert!(
                    !timeout.timed_out(),
                    "test must release completed Mongo read"
                );
            }
            _ => {}
        }));
        let client = Client::with_options(options).unwrap();
        let name = format!("cma_progress_batch_{}", uuid::Uuid::new_v4().simple());
        // Independent control client can expire or fence the paused writer.
        let db = Client::with_uri_str(uri).await.unwrap().database(&name);
        let store = MongoAgentLifecycleStore::open(&client.database(&name))
            .await
            .unwrap();
        let owner = SubjectId::new("owner");
        let input = LifecycleInput {
            behavior: LifecycleBehavior::PromptRepository,
            managed_agent_instructions: None,
            setup_message: None,
            request: serde_json::from_value(serde_json::json!({"name":"Progress test"})).unwrap(),
            resolved_image: None,
            capacity: None,
            sandbox_availability: Default::default(),
            capabilities: vec![],
            actor: None,
        };
        let CreateResult::Created(id) = store.create(&owner, "test", input).await.unwrap() else {
            panic!("fresh operation")
        };
        let lease = store.claim(&id).await.unwrap().unwrap().lease().unwrap();
        probe.take_commands();
        Self {
            db,
            store,
            lease,
            probe,
        }
    }

    pub async fn read(&self) -> LifecycleRecord {
        self.db
            .collection::<LifecycleRecord>(COLLECTION)
            .find_one(doc! {"_id": &self.lease.id})
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn set(&self, fields: mongodb::bson::Document) {
        self.db
            .collection::<mongodb::bson::Document>(COLLECTION)
            .update_one(doc! {"_id": &self.lease.id}, doc! {"$set": fields})
            .await
            .unwrap();
    }
}
