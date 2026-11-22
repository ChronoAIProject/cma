use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::{ClientOptions, Credential},
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

fn test_executable_digest() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let path = std::env::current_exe().unwrap();
        assert!(fs::metadata(&path).unwrap().len() <= 1024 * 1024 * 1024);
        let script = "const fs=require('node:fs'), crypto=require('node:crypto'); const hash=crypto.createHash('sha256'); const stream=fs.createReadStream(process.argv[1]); stream.on('data', b=>hash.update(b)); stream.on('end',()=>process.stdout.write(hash.digest('hex'))); stream.on('error',()=>process.exit(1));";
        let out = std::process::Command::new("node").args(["-e", script]).arg(path).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }).clone()
}

pub(super) struct ExportFixture {
    pub fixture: Fixture,
    pub reader: Client,
    pub app_name: String,
    pub commands: Arc<Mutex<Vec<(String, Document)>>>,
    pub directory: PathBuf,
}

impl ExportFixture {
    pub async fn new() -> Self {
        let fixture = Fixture::new().await;
        let db = fixture.client.database(&fixture.database);
        db.create_collection("sandboxes").await.unwrap();
        db.run_command(export::authority::role(&fixture.database))
            .await
            .unwrap();
        let password = uuid::Uuid::new_v4().to_string();
        db.run_command(doc! {"createUser": "catalog-reader", "pwd": &password,
        "roles": ["cma_home_backup_catalog_reader"]})
            .await
            .unwrap();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.credential = Some(
            Credential::builder()
                .username("catalog-reader".to_owned())
                .password(password)
                .source(fixture.database.clone())
                .build(),
        );
        options.retry_reads = Some(false);
        options.retry_writes = Some(false);
        let app_name = format!("catalog-reader-{}", uuid::Uuid::new_v4());
        options.app_name = Some(app_name.clone());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                observed
                    .lock()
                    .unwrap()
                    .push((event.command_name, event.command));
            }
        }));
        let reader = Client::with_options(options).unwrap();
        let directory =
            std::env::temp_dir().join(format!("catalog-export-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = fs::canonicalize(directory).unwrap();
        Self {
            fixture,
            reader,
            app_name,
            commands,
            directory,
        }
    }
    pub async fn insert_home(&self, id: &str) {
        let parent = discovery_tests::retained(id, false);
        self.fixture
            .client
            .database(&self.fixture.database)
            .collection("sandboxes")
            .insert_one(crate::store::sandboxes::sandbox_document(&parent, 0).unwrap())
            .await
            .unwrap();
    }
    pub async fn ready_history(&self, id: &str) {
        let db = self.fixture.client.database(&self.fixture.database);
        let row = db
            .collection("sandboxes")
            .find_one(doc! {"_id": id})
            .await
            .unwrap()
            .unwrap();
        let parent = crate::store::sandboxes::read_sandbox(&row).unwrap();
        let home = parent
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap();
        let mut schedule = schedule();
        schedule.id = id.into();
        schedule.home.mode = "legacy_snapshot".into();
        schedule.home.home_identity = home.identity().unwrap();
        schedule.home.home_source_id = home.receipt.storage_generation;
        schedule.home.source_reference = Some(home.receipt.storage_reference);
        let observed = SystemTime::now();
        for (epoch, seconds) in [(1, 3000), (2, 1200)] {
            let mut backup = standalone_backup(HomeBackupState::Ready);
            backup.sandbox_id = id.into();
            backup.epoch = epoch;
            backup.id = format!("hb-{id}-{epoch}");
            backup.source.home_identity = schedule.home.home_identity.clone();
            backup.storage.source_reference = Some(format!("snapshot-fixture-{id}-{epoch}"));
            let requested = observed - Duration::from_secs(seconds);
            backup.cut.requested_at = layer2_core::rfc3339_at(requested);
            backup.cut.source_creation_time = Some(backup.cut.requested_at.clone());
            backup.cut.ready_at = Some(layer2_core::rfc3339_at(requested + Duration::from_secs(1)));
            backup.retention.expires_at =
                layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(86400));
            schedule.last_ready_at = backup.cut.ready_at.clone();
            schedule.last_ready_backup_id = Some(backup.id.clone());
            db.collection(BACKUPS_COLLECTION)
                .insert_one(backup_document(&backup).unwrap())
                .await
                .unwrap();
        }
        db.collection(SCHEDULES_COLLECTION)
            .insert_one(schedule_document(&schedule).unwrap())
            .await
            .unwrap();
    }
    pub async fn export(
        &self,
        limits: Limits,
        hook: &dyn export::PageHook,
    ) -> export::Result<Vec<u8>> {
        let source = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(source.status.success());
        export::observe(
            &self.reader,
            &self.reader.database(&self.fixture.database),
            Identity {
                source_revision_claim: String::from_utf8(source.stdout).unwrap().trim().into(),
                executable_sha256: test_executable_digest(),
            },
            limits,
            hook,
        )
        .await
    }
    pub fn assert_read_only(&self) {
        let commands = self.commands.lock().unwrap();
        assert!(!commands.is_empty());
        for (name, _) in commands.iter() {
            assert!(
                [
                    "connectionStatus",
                    "listCollections",
                    "listIndexes",
                    "find",
                    "getMore",
                    "killCursors",
                    "endSessions"
                ]
                .contains(&name.as_str()),
                "unexpected command {name}"
            );
        }
    }
    pub fn coverage(&self, bytes: &[u8]) -> Value {
        let artifact = self
            .directory
            .join(format!("{}.json", uuid::Uuid::new_v4()));
        let receipt = export::cli::publish(&artifact, bytes).unwrap();
        let receipt_path = artifact.with_extension("receipt");
        crate::store::ledger_census::files::publish(&receipt_path, receipt.as_bytes()).unwrap();
        let script = r#"
import { coverage } from './.github/scripts/home-backup/coverage.mjs';
import { loadCatalogExport } from './.github/scripts/home-backup/catalog-export.mjs';
const paths = {artifact_path: process.env.CATALOG_ARTIFACT, receipt_path: process.env.CATALOG_RECEIPT};
const catalog = loadCatalogExport(paths), now = process.env.CATALOG_NOW;
const rows = catalog.home_backups.filter(b => b.state === 'ready');
const cloud = {mode: 'fixture', observed_at: now, snapshots: rows.map(b => ({
 handle: b.storage.storage_handle, status: 'READY', created_at: b.cut.provider_creation_time,
 ready_observed_at: b.cut.ready_at, storage_locations: ['fixture-region'], cmek_key_version_id: b.custody.cmek_key_version_id}))};
const dependencies = {atlas: {earliest_at:'2020-01-01T00:00:00Z', latest_at: now},
 available_key_ids: [...new Set(rows.flatMap(b => [b.custody.cmek_key_version_id,...b.custody.auth_key_ids]))],
 objects: {status:'replicated', consistent_at:now, receipt:'synthetic-object-receipt'}};
console.log(JSON.stringify(coverage({mode:'fixture', now, catalog_export: paths, cloud, dependencies})));
"#;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let out = std::process::Command::new("node")
            .args(["--input-type=module", "-e", script])
            .current_dir(root)
            .env("CATALOG_ARTIFACT", &artifact)
            .env("CATALOG_RECEIPT", &receipt_path)
            .env("CATALOG_NOW", layer2_core::now_rfc3339())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "Node consumer failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    pub async fn cleanup(self) {
        let db = self.fixture.client.database(&self.fixture.database);
        db.run_command(doc! {"dropUser": "catalog-reader"})
            .await
            .unwrap();
        db.run_command(doc! {"dropRole": "cma_home_backup_catalog_reader"})
            .await
            .unwrap();
        fs::remove_dir_all(&self.directory).unwrap();
        self.fixture.cleanup().await;
    }
}

pub(super) fn payload(bytes: &[u8]) -> Value {
    let value: Value = serde_json::from_slice(bytes).unwrap();
    serde_json::from_str(value["payload_json"].as_str().unwrap()).unwrap()
}
