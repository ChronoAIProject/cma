use super::*;

pub(super) const KINDS: [&str; 3] = ["agent", "workspace", "sandbox"];
pub(super) fn id(kind: &str, number: usize) -> String {
    let prefix = match kind {
        "agent" => "agp",
        "workspace" => "wkp",
        _ => "sbp",
    };
    format!("{prefix}_{number:032x}")
}
pub(super) fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8000,
        min_memory_mib: 128,
        max_memory_mib: 16384,
    }
}
pub(super) fn collections<'a>(
    store: &'a MongoProfileStore,
    kind: &str,
) -> (
    &'a mongodb::Collection<Document>,
    &'a mongodb::Collection<Document>,
) {
    match kind {
        "agent" => (&store.agent_profiles, &store.agent_revisions),
        "workspace" => (&store.workspace_profiles, &store.workspace_revisions),
        "sandbox" => (&store.sandbox_profiles, &store.sandbox_revisions),
        _ => unreachable!(),
    }
}

pub(super) async fn seed(
    store: &MongoProfileStore,
    kind: &str,
    count: usize,
    revisions: u32,
    large: bool,
) {
    let (profiles, versions) = collections(store, kind);
    let mut rows = Vec::new();
    for number in 0..count {
        let agent = agent_profile(&id(kind, number));
        let mut value = serde_json::to_value(agent).unwrap();
        value["name"] = json!(format!("Profile {number}"));
        if large {
            value["draft"]["system_prompt"] = json!("x".repeat(32 * 1024));
        }
        if kind == "workspace" {
            value["draft"] = json!({"location":{"type":"directory","name":"work"},"name":"work","setup_commands":[],"agent_profiles":[]});
        } else if kind == "sandbox" {
            value["draft"] = json!({"cpu_millis":1000,"memory_mib":1024,"nyxid_services":[],"agent_provider":"codex","model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":[],"environment_variables":[]});
        }
        let (mut row, revision) = match kind {
            "agent" => {
                let profile: AgentProfile = serde_json::from_value(value).unwrap();
                (
                    crate::store::agent_profiles::document(&profile).unwrap(),
                    serde_json::to_value(profile.publish(1, NOW.into()).unwrap()).unwrap(),
                )
            }
            "workspace" => {
                let profile: WorkspaceProfile = serde_json::from_value(value).unwrap();
                (
                    crate::store::workspace_profiles::document(&profile).unwrap(),
                    serde_json::to_value(profile.publish(1, NOW.into()).unwrap()).unwrap(),
                )
            }
            _ => {
                let profile: SandboxProfile = serde_json::from_value(value).unwrap();
                (
                    crate::store::sandbox_profiles::document(&profile).unwrap(),
                    serde_json::to_value(
                        profile
                            .publish(1, NOW.into(), bounds(), &([] as [String; 0]))
                            .await
                            .unwrap(),
                    )
                    .unwrap(),
                )
            }
        };
        row.insert("last_revision", i64::from(revisions));
        if revisions > 0 {
            row.insert("latest_published_at", NOW);
        }
        rows.push(row);
        if revisions > 0 {
            let records = (1..=revisions)
                .map(|n| {
                    let mut revision = revision.clone();
                    revision["revision"] = json!(n);
                    crate::store::profile_documents::revision_document(
                        &id(kind, number),
                        n,
                        &revision,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            versions.insert_many(records).await.unwrap();
        }
    }
    if !rows.is_empty() {
        profiles.insert_many(rows).await.unwrap();
    }
}

#[derive(Default)]
pub(super) struct Counts {
    pub commands: usize,
    pub bytes: usize,
    pub failed: usize,
}
#[derive(Default)]
pub(super) struct Meter(Mutex<Option<Counts>>);
impl Meter {
    pub fn start(&self) {
        assert!(self.0.lock().unwrap().replace(Counts::default()).is_none());
    }
    pub fn finish(&self) -> Counts {
        self.0.lock().unwrap().take().unwrap()
    }
}
pub(super) async fn measured_store() -> (Client, String, Arc<MongoProfileStore>, Arc<Meter>) {
    observed_store(|_| {}).await
}

pub(super) async fn observed_store(
    handler: impl Fn(CommandEvent) + Send + Sync + 'static,
) -> (Client, String, Arc<MongoProfileStore>, Arc<Meter>) {
    let (_, name) = database("profile_summaries").await;
    let meter = Arc::new(Meter::default());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(name.clone());
    let observed = meter.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut held = observed.0.lock().unwrap();
        if let Some(count) = held.as_mut() {
            match &event {
                CommandEvent::Started(_) => count.commands += 1,
                CommandEvent::Succeeded(event) => {
                    count.bytes += event.reply.to_vec().unwrap().len()
                }
                CommandEvent::Failed(_) => count.failed += 1,
                _ => {}
            }
        }
        drop(held);
        handler(event);
    }));
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(MongoProfileStore::open(&client, &name).await.unwrap());
    (client, name, store, meter)
}

struct Unused;
#[async_trait::async_trait]
impl layer2_core::OperationStore for Unused {
    async fn reserve(
        &self,
        _: &layer2_core::OperationClaim,
    ) -> Result<layer2_core::OperationReservation, layer2_core::StoreError> {
        panic!("read reserved operation")
    }
    async fn complete(
        &self,
        _: &layer2_core::OperationClaim,
        _: Vec<u8>,
    ) -> Result<(), layer2_core::StoreError> {
        panic!("read completed operation")
    }
}
#[async_trait::async_trait]
impl layer2_core::PlatformSettingsStore for Unused {
    async fn read(&self) -> Result<layer2_core::StoredPlatformSettings, layer2_core::StoreError> {
        panic!("profile list consulted settings")
    }
    async fn replace(
        &self,
        _: u64,
        _: &layer2_core::PlatformSettings,
    ) -> Result<layer2_core::StoredPlatformSettings, layer2_core::StoreError> {
        panic!("profile list changed settings")
    }
}
#[async_trait::async_trait]
impl layer2_core::ActivityStore for Unused {
    async fn append(&self, _: &layer2_core::ActivityRecord) -> Result<(), layer2_core::StoreError> {
        panic!("profile list wrote activity")
    }
    async fn list(
        &self,
        _: &layer2_core::ActivityQuery,
    ) -> Result<layer2_core::Page<layer2_core::ActivityRecord>, layer2_core::StoreError> {
        panic!("profile list read activity")
    }
}
pub(super) fn routes(store: Arc<MongoProfileStore>) -> Router {
    layer1_public::rest::profiles_router(layer1_public::rest::ProfilesState::new(
        store,
        Arc::new(Unused),
        Arc::new(layer2_core::PlatformSettingsService::new(
            Arc::new(Unused),
            layer2_core::ActivityService::new(Arc::new(Unused)),
        )),
        layer1_public::rest::ProfileRoutePolicy {
            capacity_bounds: bounds(),
            public_value_scanner: Arc::new([] as [String; 0]),
        },
    ))
}
pub(super) async fn get(router: &Router, owner: &str, uri: &str) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId(owner.into())));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::ManageOwnProfiles]));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
