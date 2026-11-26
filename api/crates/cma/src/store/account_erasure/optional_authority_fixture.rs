use super::*;

pub(super) struct Fixture {
    pub(super) admin: Database,
    pub(super) runtime: Database,
    pub(super) commands: Arc<Mutex<Vec<Document>>>,
    collection: &'static str,
}
impl Fixture {
    pub(super) async fn new(optional: bool, omit: Option<&str>) -> Self {
        Self::with_missing_action(optional, omit, None).await
    }
    pub(super) async fn with_missing_action(
        optional: bool,
        omit: Option<&str>,
        missing: Option<&str>,
    ) -> Self {
        Self::for_collection(OPTIONAL, optional, omit, missing).await
    }
    pub(super) async fn for_collection(
        collection: &'static str,
        optional: bool,
        omit: Option<&str>,
        missing: Option<&str>,
    ) -> Self {
        let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
        let client = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("erasure_authority_{}", uuid::Uuid::new_v4().simple());
        let admin = client.database(&name);
        let mut role = crate::store::isolation::replicas::role(&name, true);
        role.get_array_mut("privileges").unwrap().retain(|grant| {
            let name = grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap();
            (optional || name != collection)
                && omit != Some(name)
                && !crate::store::sandbox_usage_export::COLLECTIONS.contains(&name)
        });
        if let Some(missing) = missing {
            for grant in role.get_array_mut("privileges").unwrap() {
                let grant = grant.as_document_mut().unwrap();
                if grant
                    .get_document("resource")
                    .unwrap()
                    .get_str("collection")
                    .unwrap()
                    == collection
                {
                    grant
                        .get_array_mut("actions")
                        .unwrap()
                        .retain(|action| action.as_str() != Some(missing));
                }
            }
        }
        admin.run_command(role).await.unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        admin
            .run_command(doc! {"createUser":"erasure-runtime","pwd":&password,
            "roles":[{"role":"cma_am_runtime","db":&name}]})
            .await
            .unwrap();
        let commands = Arc::new(Mutex::new(vec![]));
        let captured = commands.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.credential = Some(
            Credential::builder()
                .username("erasure-runtime".to_owned())
                .password(password)
                .source(name.clone())
                .build(),
        );
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                captured.lock().unwrap().push(event.command);
            }
        }));
        let runtime = Client::with_options(options).unwrap().database(&name);
        Self {
            admin,
            runtime,
            commands,
            collection,
        }
    }
    pub(super) async fn absent(&self) {
        assert!(!self
            .admin
            .list_collection_names()
            .await
            .unwrap()
            .iter()
            .any(|name| name == self.collection));
    }
}
