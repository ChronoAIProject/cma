use super::*;
use layer2_core::sandbox::Agent;
use mongodb::Collection;

pub(super) struct Fixture {
    pub admin: Client,
    pub database: Database,
    pub reader: Client,
    pub app_name: String,
    commands: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_turn_inventory_{}", uuid::Uuid::new_v4().simple());
        let database = admin.database(&name);
        for collection in COLLECTIONS {
            database.create_collection(collection).await.unwrap();
        }
        database
            .run_command(authority::test_role(&name))
            .await
            .unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        database
            .run_command(doc! {"createUser": "inventory-reader", "pwd": &password,
            "roles": ["turn-inventory-reader"]})
            .await
            .unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.credential = Some(
            Credential::builder()
                .username("inventory-reader".to_owned())
                .password(password)
                .source(name)
                .build(),
        );
        options.retry_reads = Some(false);
        options.retry_writes = Some(false);
        let app_name = format!("turn-inventory-{}", uuid::Uuid::new_v4());
        options.app_name = Some(app_name.clone());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured = commands.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                captured.lock().unwrap().push(event.command_name);
            }
        }));
        let reader = Client::with_options(options).unwrap();
        Self {
            admin,
            database,
            reader,
            app_name,
            commands,
        }
    }
    pub fn budget(&self) -> Collection<Document> {
        self.database.collection(super::super::super::COLLECTION)
    }
    pub async fn insert(&self, agent: &Agent, saved: &super::super::super::TurnRecord) {
        self.budget().insert_one(document(saved)).await.unwrap();
        self.database
            .collection("agents")
            .insert_one(crate::store::agents::agent_document(agent).unwrap())
            .await
            .unwrap();
    }
    pub async fn observe(&self, limits: Limits, hook: &dyn PageHook) -> Result<CompleteInventory> {
        observe(
            &self.reader,
            &self.reader.database(self.database.name()),
            limits,
            hook,
        )
        .await
    }
    pub fn assert_reads(&self) {
        let commands = self.commands.lock().unwrap();
        assert!(!commands.is_empty());
        for command in commands.iter() {
            assert!(
                [
                    "connectionStatus",
                    "listCollections",
                    "find",
                    "getMore",
                    "killCursors",
                    "endSessions"
                ]
                .contains(&command.as_str()),
                "unexpected command {command}"
            );
        }
    }
    pub async fn cleanup(self) {
        self.database
            .run_command(doc! {"dropUser": "inventory-reader"})
            .await
            .unwrap();
        self.database
            .run_command(doc! {"dropRole": "turn-inventory-reader"})
            .await
            .unwrap();
        self.database.drop().await.unwrap();
    }
}
