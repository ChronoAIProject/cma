#[path = "github_integrations/authority.rs"]
mod authority;
#[path = "github_integrations/registration.rs"]
mod registration;
#[path = "github_integrations/rotation.rs"]
mod rotation;
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
            admin.run_command(mongodb::bson::doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_automation_runtime","db":&database.name}]}).await.unwrap();
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
        };
        fixture.base = fixture.replica().await;
        fixture
    }

    async fn replica(&mut self) -> String {
        self.replica_with_flag(Some(true)).await
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
        database.run_command(mongodb::bson::doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_automation_migrator","db":&self.database.name}]}).await.unwrap();
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
