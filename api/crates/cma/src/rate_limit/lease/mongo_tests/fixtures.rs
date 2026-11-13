use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};

pub(super) struct Fixture {
    pub database: mongodb::Database,
    pub store: Arc<MongoRequestLimits>,
    pub commands: Arc<Mutex<Vec<Document>>>,
}

impl Fixture {
    pub async fn new() -> Self {
        let uri =
            std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
        let mut options = ClientOptions::parse(uri).await.unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured = commands.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if ["findAndModify", "aggregate", "insert"]
                    .iter()
                    .any(|name| event.command.get_str(name).ok() == Some(COLLECTION))
                {
                    captured.lock().unwrap().push(event.command);
                }
            }
        }));
        let database = Client::with_options(options).unwrap().database(&format!(
            "cma_default_lease_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = Arc::new(MongoRequestLimits::open(&database).await.unwrap());
        Self {
            database,
            store,
            commands,
        }
    }

    pub fn take_commands(&self) -> Vec<Document> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }

    pub async fn boundary(&self, key: &str, offset_ms: i64) {
        self.database
            .collection::<Document>(COLLECTION)
            .update_one(
                doc! {"_id": key},
                vec![doc! {"$set": {
                    "document_schema_version": 1,
                    "tat": {"$add": [{"$toLong": "$$NOW"}, offset_ms]},
                    "expires_at": {"$add": ["$$NOW", 120_000]},
                }}],
            )
            .upsert(true)
            .await
            .unwrap();
    }
}

pub(super) fn normalized(mut commands: Vec<Document>) -> Vec<Document> {
    for command in &mut commands {
        for field in ["lsid", "txnNumber", "$clusterTime", "$db"] {
            command.remove(field);
        }
        if let Some(mongodb::bson::Bson::Array(documents)) = command.get_mut("documents") {
            for document in documents {
                if let mongodb::bson::Bson::Document(document) = document {
                    document.insert("expires_at", mongodb::bson::DateTime::from_millis(0));
                }
            }
        }
    }
    commands
}

pub(super) fn config(enabled: bool) -> crate::config::RateLimitConfig {
    let n = |value| std::num::NonZeroU32::new(value).unwrap();
    crate::config::RateLimitConfig {
        admission_per_minute: n(6000),
        health_per_minute: n(120),
        default_per_minute: n(600),
        create_per_minute: n(10),
        input_per_minute: n(120),
        control_per_minute: n(120),
        control_tier_enabled: true,
        response_input_tier_enabled: true,
        anonymous_public: crate::config::AnonymousPublicConfig {
            per_minute: n(1200),
            concurrency: n(16),
        },
        default_lease_enabled: enabled,
    }
}

pub(super) async fn history(table: &LeaseTable, key: &str, count: u32) {
    table.entry(key).unwrap().lock().await.previous_arrivals = count;
}
