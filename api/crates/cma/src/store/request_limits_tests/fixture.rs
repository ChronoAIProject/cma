use super::super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

pub(super) struct Fixture {
    pub database: Database,
    pub admin: Database,
    pub left: Arc<MongoRequestLimits>,
    pub right: Arc<MongoRequestLimits>,
    pub app: String,
    commands: Arc<Mutex<Vec<Document>>>,
}
impl Fixture {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let app = format!("request-limit-{}", uuid::Uuid::new_v4().simple());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let captured = commands.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                captured.lock().unwrap().push(event.command);
            }
        }));
        // Separate clients and pools, as well as independent store handles.
        let left_client = Client::with_options(options.clone()).unwrap();
        let right_client = Client::with_options(options).unwrap();
        let name = format!("cma_request_upsert_{}", uuid::Uuid::new_v4().simple());
        let database = left_client.database(&name);
        let left = Arc::new(MongoRequestLimits::open(&database).await.unwrap());
        let right = Arc::new(
            MongoRequestLimits::open(&right_client.database(&name))
                .await
                .unwrap(),
        );
        let admin = Client::with_uri_str(uri).await.unwrap().database("admin");
        assert_eq!(
            admin
                .run_command(doc! {"buildInfo":1})
                .await
                .unwrap()
                .get_str("version")
                .unwrap(),
            "8.0.13"
        );
        let fixture = Self {
            database,
            admin,
            left,
            right,
            app,
            commands,
        };
        fixture.take();
        fixture
    }
    pub fn take(&self) -> Vec<Document> {
        std::mem::take(&mut self.commands.lock().unwrap())
    }
    pub async fn seed(&self, key: &str, tat: mongodb::bson::Bson) {
        self.left.limits.delete_one(doc! {"_id":key}).await.unwrap();
        // Retained preimages must survive TTL cleanup until the command under test.
        let expires_at = mongodb::bson::DateTime::from_millis(
            mongodb::bson::DateTime::now().timestamp_millis() + 86_400_000,
        );
        // Missing schema marker and Int32 values are retained legacy formats.
        self.left
            .limits
            .insert_one(doc! {"_id":key,"tat":tat,"expires_at":expires_at})
            .await
            .unwrap();
    }
    pub async fn fail(&self, code: i32, times: i32) {
        self.admin
            .run_command(
                doc! {"configureFailPoint":"failCommand","mode":{"times":times},"data":{
                    "failCommands":["findAndModify"],"appName":&self.app,"errorCode":code,"errorExtraInfo":{"keyPattern":{"_id":1},"keyValue":{"_id":"request-limit"}},"keyPattern":{"_id":1},"keyValue":{"_id":"request-limit"}
                }},
            )
            .await
            .unwrap();
    }
    pub async fn clear_failpoint(&self) {
        self.admin
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
    }
    pub async fn close(self) {
        self.database.drop().await.unwrap();
    }
}

pub(super) fn replay(mut command: Document, key: &str) -> Document {
    for name in [
        "lsid",
        "txnNumber",
        "$clusterTime",
        "$db",
        "$readPreference",
        "readConcern",
        "writeConcern",
    ] {
        command.remove(name);
    }
    if let Ok(query) = command.get_document_mut("query") {
        query.insert("_id", key);
    }
    command
}
