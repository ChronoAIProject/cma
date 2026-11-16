use super::*;
use crate::store::bindings::admission_tests;
use crate::store::bindings::mongo_tests::{binding, NOW};
use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};
use base64::Engine;
use layer2_core::sandbox::store::{SandboxStore, WorkAuthorityProtocol, WorkspaceStore};
use layer2_core::sandbox::{
    RuntimeBinding, Sandbox, SandboxStatus, SandboxWorkStep, Workspace, WorkspaceStatus,
    WorkspaceWorkStep,
};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use ring::rand::{SecureRandom, SystemRandom};
use std::sync::Mutex;

#[derive(Clone, Default)]
pub(super) struct Trace {
    pub starts: usize,
    pub commits: usize,
    pub sessions: Vec<Document>,
    pub timestamps: Vec<String>,
}

pub(super) struct Fixture {
    pub admin: Client,
    pub database: String,
    pub app: String,
    pub controller: Arc<MongoBindingCredentials>,
    pub custody: Arc<MongoSandboxCredentialStore>,
    pub binding: ProviderBinding,
    pub key: ProvidedCredential,
    pub sandbox: Sandbox,
    pub workspace: Workspace,
    pub trace: Arc<Mutex<Trace>>,
}

impl Fixture {
    pub async fn new(failpoints: bool, schema: i64) -> Self {
        let uri = std::env::var(if failpoints {
            "CHRONO_TEST_MONGODB_FAILPOINT_URI"
        } else {
            "CHRONO_TEST_MONGODB_URI"
        })
        .expect("a disposable MongoDB replica set is required");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let database = format!("cma_binding_retry_{suffix}");
        let app = format!("cma-binding-retry-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let observed = trace.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let CommandEvent::Started(event) = event else {
                return;
            };
            let mut trace = observed.lock().unwrap();
            if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                trace.starts += 1;
                let session = event.command.get_document("lsid").unwrap().clone();
                if !trace.sessions.contains(&session) {
                    trace.sessions.push(session);
                }
            }
            if event.command_name == "commitTransaction" {
                trace.commits += 1;
            }
            if matches!(event.command.get_str("update"), Ok("provider_bindings")) {
                for update in event.command.get_array("updates").unwrap() {
                    let at = update
                        .as_document()
                        .and_then(|value| value.get_document("u").ok())
                        .and_then(|value| value.get_document("$set").ok())
                        .and_then(|value| value.get_document("record").ok())
                        .and_then(|value| value.get_str("updated_at").ok());
                    if let Some(at) = at {
                        trace.timestamps.push(at.to_owned());
                    }
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
        let mut bytes = [0; 32];
        SystemRandom::new().fill(&mut bytes).unwrap();
        let encryption = AuthEncryptionKey::from_base64(
            &base64::engine::general_purpose::STANDARD.encode(bytes),
        )
        .unwrap();
        let custody = Arc::new(
            MongoSandboxCredentialStore::open(
                &client,
                &database,
                "binding-retry-test",
                &encryption,
            )
            .await
            .unwrap(),
        );
        let controller = MongoBindingCredentials::new(store.clone(), custody.clone());
        let key =
            ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
        let mut binding = binding();
        binding.key_sha256 = key.sha256();
        store.save_binding(None, binding.clone()).await.unwrap();
        controller.retain_binding(&binding, &key).await.unwrap();
        let material = custody.get(&binding.sandbox_id, 1).await.unwrap().unwrap();
        binding.generation = 3;
        binding.state = BindingState::Failed {
            reason: "provisioning failed".into(),
        };
        binding.work = BindingWork::Idle;
        binding.advance_revision(NOW.into()).unwrap();
        store
            .bindings
            .records
            .replace_one(
                doc! {"_id": &binding.binding_id},
                binding_document(&binding).unwrap(),
            )
            .await
            .unwrap();
        let mut sandbox = admission_tests::sandbox(&binding);
        sandbox.status = SandboxStatus::Active;
        sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
        sandbox.runtime = Some(RuntimeBinding {
            boot_id: None,
            runtime_id: "retained-runtime".into(),
            runtime_version: "runtime-v1".into(),
            provision_generation: Some(3),
            bound_at: NOW.into(),
        });
        sandbox.lifecycle.runtime_generation_high_water = 3;
        sandbox.credential.current = Some(material.authority);
        sandbox.credential.key_generation = 1;
        sandbox.credential.delivered = true;
        let mut workspace = admission_tests::workspace(&binding);
        workspace.status = WorkspaceStatus::Ready;
        workspace.resolved_commit = Some("a".repeat(40));
        workspace.ready_at = Some(NOW.into());
        workspace.work.schedule(WorkspaceWorkStep::None, NOW.into());
        if schema >= 3 {
            sandbox.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
            workspace.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
        }
        if schema == 4 {
            sandbox.work.prepared_installation_digest = Some("b".repeat(64));
        }
        store.create_sandbox(sandbox.clone()).await.unwrap();
        store.create_workspace(workspace.clone()).await.unwrap();
        *trace.lock().unwrap() = Trace::default();
        Self {
            admin,
            database,
            app,
            controller,
            custody,
            binding,
            key,
            sandbox,
            workspace,
            trace,
        }
    }

    pub async fn fault(&self, command: &str, failures: i32, unknown: bool) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand",
                "mode": if failures == 0 { mongodb::bson::Bson::String("alwaysOn".into()) }
                    else { mongodb::bson::Bson::Document(doc! {"times": failures}) },
                "data": {"appName": &self.app, "failCommands": [command],
                    "errorCode": if unknown {1_i32} else {112_i32},
                    "errorLabels": [if unknown {"UnknownTransactionCommitResult"}
                        else {"TransientTransactionError"}]},
            })
            .await
            .unwrap();
    }

    pub async fn release_fault(&self) -> Trace {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .expect("disable the test fault before assertions");
        let trace = self.trace.lock().unwrap().clone();
        if !trace.sessions.is_empty() {
            // A refused commit may leave an uncommitted server transaction after
            // the driver marks its session committed. Release only our sessions.
            self.admin
                .database("admin")
                .run_command(doc! {
                    "killSessions": trace.sessions.clone(),
                })
                .await
                .expect("release only this fixture's injected transactions");
        }
        trace
    }

    pub async fn snapshot(&self) -> (ProviderBinding, Sandbox, Workspace, i64, i64) {
        let store = &self.controller.store;
        let binding = store
            .binding(&self.binding.binding_id)
            .await
            .unwrap()
            .unwrap();
        let sandbox = store.get_sandbox(&self.sandbox.id).await.unwrap().unwrap();
        let workspace = store
            .get_workspace(&self.workspace.id)
            .await
            .unwrap()
            .unwrap();
        let row = store
            .sandboxes
            .find_one(doc! {"_id": self.sandbox.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let parent_fence = row.get_i64("binding_recovery_admission_fence").unwrap_or(0);
        let row = store
            .bindings
            .records
            .find_one(doc! {"_id": &self.binding.binding_id})
            .await
            .unwrap()
            .unwrap();
        (
            binding,
            sandbox,
            workspace,
            parent_fence,
            row.get_i64("admission_fence").unwrap_or(0),
        )
    }

    pub async fn cleanup(&self) {
        self.admin
            .database(&self.database)
            .drop()
            .await
            .expect("drop fixture before assertions");
    }
}
