//! Authenticated disposable database for the actual gateway executable.
use mongodb::{bson::doc, Client};
pub struct DatabaseFixture {
    #[allow(dead_code)] // Used by the signed-ingress fixture; Bot tests use only the limited URI.
    pub admin: Client,
    pub name: String,
    pub uri: String,
    pub cookie_key: std::path::PathBuf,
}
impl Drop for DatabaseFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.cookie_key);
    }
}
impl DatabaseFixture {
    pub async fn new(binary: &std::path::Path) -> Self {
        Self::with_migration(binary, "--migrate").await
    }
    #[allow(dead_code)] // Only the enabled multi-process fixture needs the additive migration.
    pub async fn replicas(binary: &std::path::Path) -> Self {
        Self::with_migration(binary, "--migrate-replicas").await
    }
    async fn with_migration(binary: &std::path::Path, migration: &str) -> Self {
        let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI")
            .expect("run the authenticated service Mongo contract");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_eg_process_{}", uuid::Uuid::new_v4().simple());
        let db = admin.database(&name);
        let password = uuid::Uuid::new_v4().to_string();
        for (flag, user, role) in [
            ("--print-runtime-role", "runtime", "cma_eg_runtime"),
            ("--print-migration-role", "migrator", "cma_eg_migrator"),
        ] {
            let output = tokio::process::Command::new(binary)
                .arg(flag)
                .env("CMAEG_MONGODB_DATABASE", &name)
                .output()
                .await
                .unwrap();
            assert!(output.status.success());
            let role_doc: mongodb::bson::Document = serde_json::from_slice(&output.stdout).unwrap();
            db.run_command(role_doc).await.unwrap();
            db.run_command(
                doc! {"createUser":user,"pwd":&password,"roles":[{"role":role,"db":&name}]},
            )
            .await
            .unwrap();
        }
        let mut url = reqwest::Url::parse(&uri).unwrap();
        url.set_username("migrator").unwrap();
        url.set_password(Some(&password)).unwrap();
        url.set_path(&format!("/{name}"));
        url.set_query(Some("directConnection=true"));
        let output = tokio::process::Command::new(binary)
            .arg(migration)
            .env("CMAEG_MONGODB_DATABASE", &name)
            .env("CMAEG_MONGODB_URI", url.as_str())
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "gateway fixture migration failed");
        url.set_username("runtime").unwrap();
        let cookie_key = std::env::temp_dir().join(format!("cma-cookie-{}", uuid::Uuid::new_v4()));
        std::fs::write(&cookie_key, [7_u8; 32]).unwrap();
        Self {
            admin,
            name,
            uri: url.to_string(),
            cookie_key,
        }
    }
    pub fn configure(&self, command: &mut tokio::process::Command) {
        command
            .env("CMAEG_MONGODB_URI", &self.uri)
            .env("CMAEG_MONGODB_DATABASE", &self.name)
            .env("CMAEG_NYXID_CLIENT_ID", "gateway-management")
            .env("CMAEG_NYXID_CLIENT_SECRET", "gateway-management-fixture")
            .env("CMAEG_WEB_ORIGIN", "https://cma.example")
            .env("CMAEG_COOKIE_KEY_FILE", &self.cookie_key);
    }
}
