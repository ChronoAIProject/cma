#[path = "github_integrations/authority.rs"]
mod authority;
#[path = "github_integrations/bindings.rs"]
mod bindings;
#[path = "github_integrations/contention.rs"]
mod contention;
#[path = "github_integrations/dispatch.rs"]
mod dispatch;
#[path = "github_integrations/legacy.rs"]
mod legacy;
#[path = "github_integrations/lifecycle.rs"]
mod lifecycle;
#[path = "github_integrations/registration.rs"]
mod registration;
#[path = "github_integrations/retirement.rs"]
mod retirement;
#[path = "github_integrations/retirement_rebinding.rs"]
mod retirement_rebinding;
#[path = "github_integrations/rotation.rs"]
mod rotation;
#[path = "github_integrations/sources.rs"]
mod sources;
#[path = "github_integrations/startup.rs"]
mod startup;
#[path = "github_integrations/webhooks.rs"]
mod webhooks;

use super::binary_support::{free_address, spawn_ready, MountedSecrets};
use super::*;

struct Fixture {
    provider: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    files: MountedSecrets,
    database: database::DatabaseFixture,
    children: Vec<tokio::process::Child>,
    base: String,
    client: Client,
    shared_schema: bool,
    authoring: bool,
    routed_records: Vec<String>,
}

impl Fixture {
    async fn new(migrated: bool) -> Self {
        let database =
            database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
        if migrated {
            let admin = database.admin.database(&database.name);
            cmaeg::github::migrate(&admin).await.unwrap();
            admin
                .run_command(cmaeg::github::runtime_role(&database.name))
                .await
                .unwrap();
            admin
                .run_command(mongodb::bson::doc! {
                    "grantRolesToUser":"runtime",
                    "roles":[
                        {"role":"cma_eg_automation_runtime","db":&database.name},
                        {"role":"cma_eg_runtime","db":&database.name}
                    ]
                })
                .await
                .unwrap();
        }
        let mut fixture = Self {
            provider: FakeProvider::start(FakeProviderConfig::default()).await,
            cma: FakeCma::start(false).await,
            nyxid: FakeNyxid::start().await,
            files: MountedSecrets::new(),
            database,
            children: Vec::new(),
            base: String::new(),
            client: Client::new(),
            shared_schema: false,
            authoring: false,
            routed_records: Vec::new(),
        };
        fixture.base = fixture.replica_with_flag(Some(true)).await;
        if migrated {
            fixture.wait_for_github(&fixture.base).await;
        }
        fixture
    }

    async fn replica(&mut self) -> String {
        let base = self.replica_with_flag(Some(true)).await;
        self.wait_for_github(&base).await;
        base
    }

    async fn wait_for_github(&self, base: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let capabilities = self
                    .read(base, "/v1/github/capabilities", "token-alice")
                    .await
                    .json::<serde_json::Value>()
                    .await
                    .unwrap();
                if capabilities["store_ready"] == true {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("GitHub integration storage becomes available");
    }

    async fn replica_with_flag(&mut self, enabled: Option<bool>) -> String {
        let address = free_address();
        let base = format!("http://{address}");
        let mut command = self
            .files
            .command(&address, &self.cma, &self.nyxid, &self.provider);
        self.database.configure(&mut command);
        command.env(
            "CMAEG_MONGODB_AUTHORITY_MODE",
            if self.shared_schema {
                "shared-schema"
            } else {
                "isolated"
            },
        );
        command
            .env("CMAEG_CMA_BASE_URL", self.provider.base_url())
            .env(
                "CMAEG_PUBLIC_API_URL",
                if self.base.is_empty() {
                    &base
                } else {
                    &self.base
                },
            )
            .env_remove("CMAEG_GITHUB_CONFIG_FILE");
        match enabled {
            Some(enabled) => {
                command.env("CMAEG_GITHUB_ENABLED", enabled.to_string());
            }
            None => {
                command.env_remove("CMAEG_GITHUB_ENABLED");
            }
        }
        if self.authoring {
            command.env("CMAEG_DYNAMIC_SOURCES_WRITES_ENABLED", "true")
                .env("CMAEG_MULTI_CONSUMER_WRITES_ENABLED", "true")
                .env("CMAEG_CUSTOM_DURABLE_EXECUTION_ENABLED", "true")
                .env("CMAEG_ROUTED_RECOVERY_ENABLED", "true")
                .env("CMAEG_ROUTED_ADMISSION_ENABLED", "true")
                .env("CMAEG_NON_HUMAN_PROVIDERS", "cma_codex")
                .env("CMAEG_PROVIDERS", json!([{
                    "slug":"cma_codex","base_url":self.provider.base_url(),"kind":"responses_http",
                    "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                    "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses",
                    "recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}},"max_inflight":8
                }]).to_string());
            if !self.routed_records.is_empty() {
                command.env(
                    "CMAEG_ROUTED_CHANNEL_RECORDS",
                    self.routed_records.join(","),
                );
            }
        }
        let log = self
            .files
            .dir
            .join(format!("github-{}.log", self.children.len()));
        self.children
            .push(spawn_ready(command, &address, &log).await);
        base
    }

    async fn allow_startup_migration(&mut self) {
        let database = self.database.admin.database(&self.database.name);
        database
            .run_command(cmaeg::github::migration_role(&self.database.name))
            .await
            .unwrap();
        database
            .run_command(cmaeg::records::mongo::role(&self.database.name, true))
            .await
            .unwrap();
        database.run_command(mongodb::bson::doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_automation_migrator","db":&self.database.name},{"role":"cma_eg_migrator","db":&self.database.name}]}).await.unwrap();
        self.shared_schema = true;
    }

    fn registration(
        &self,
        base: &str,
        token: &str,
        key: &str,
        body: &serde_json::Value,
    ) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{base}/v1/github/apps"))
            .bearer_auth(token)
            .header("Idempotency-Key", key)
            .json(body)
    }

    async fn read(&self, base: &str, path: &str, token: &str) -> reqwest::Response {
        self.client
            .get(format!("{base}{path}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    fn webhook(
        &self,
        url: &str,
        secret: &str,
        delivery_id: &str,
        event: &str,
        body: &str,
    ) -> reqwest::RequestBuilder {
        let signature = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()),
            body.as_bytes(),
        );
        let signature: String = signature
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.client
            .post(url)
            .header("Content-Type", "application/json")
            .header("X-GitHub-Delivery", delivery_id)
            .header("X-GitHub-Event", event)
            .header("X-Hub-Signature-256", format!("sha256={signature}"))
            .body(body.to_owned())
    }

    async fn register(&self, token: &str, key: &str, secret: Option<&str>) -> serde_json::Value {
        let response = self
            .registration(
                &self.base,
                token,
                key,
                &json!({"app_id":42,"display_name":"Test integration","webhook_secret":secret}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        response.json().await.unwrap()
    }
}
