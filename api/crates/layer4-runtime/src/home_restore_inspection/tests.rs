use super::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;

#[path = "cli_tests.rs"]
mod cli;
#[path = "format_tests.rs"]
mod formats;
#[path = "installation_tests.rs"]
mod installation;
#[path = "io_tests.rs"]
mod io_contracts;

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
    roots: [PathBuf; 3],
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-home-assessment-{}", uuid::Uuid::new_v4()));
        let roots = [root.join("home"), root.join("state"), root.join("transfer")];
        for path in &roots {
            std::fs::create_dir_all(path).unwrap();
        }
        let config = request().config().unwrap();
        let fixture = Self {
            root,
            roots,
            config,
        };
        fixture.write(
            io::STATE,
            "home-binding/binding.json",
            &serde_json::to_vec(&json!({
                "version": 2, "sandbox_id": fixture.config.sandbox_id,
                "home_identity": fixture.config.home_identity,
                "runtime_generation": fixture.config.generation, "restore": null,
            }))
            .unwrap(),
        );
        fixture.install(false);
        fixture.write(io::HOME, "user-content", b"preserve original user content");
        fixture
    }

    fn source(&self) -> io::Source {
        io::Source::fixture(self.roots.clone(), budget()).unwrap()
    }
    fn report(&self) -> Value {
        serde_json::from_slice(
            &assess(
                &self.config,
                &mut self.source(),
                &sha256(b"request"),
                OUTPUT_BYTES,
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn write(&self, root: usize, relative: &str, bytes: &[u8]) {
        write(&self.roots[root].join(relative), bytes);
    }
    fn edit(&self, root: usize, relative: &str, change: impl FnOnce(&mut Value)) {
        let path = self.roots[root].join(relative);
        let mut value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        change(&mut value);
        write(&path, &serde_json::to_vec(&value).unwrap());
    }
    fn install(&self, pending: bool) {
        let request = layer3_adapter::InstallRequest::new("inspection-install", vec![]);
        let record = if pending {
            crate::install_state::InstallRecord::started(&self.config, &request)
        } else {
            crate::install_state::InstallRecord::completed(
                &self.config,
                &request,
                layer3_adapter::InstallReceipt {
                    operation_id: request.operation_id.clone(),
                    request_sha256: request.request_sha256.clone(),
                    commands: vec![],
                },
            )
        };
        self.write(
            io::TRANSFER,
            "runtime-state/install-v2.json",
            &serde_json::to_vec(&record).unwrap(),
        );
    }
    fn v2(&self) {
        let bytes = serde_json::to_vec(&json!({
            "version": 2, "store": "11111111-1111-4111-8111-111111111111",
            "sandbox": self.config.sandbox_id, "home": self.config.home_identity,
            "state_root": self.config.state_root, "home_root": self.config.home,
            "transfer_root": self.config.checkpoint_root, "born": 1, "admission": "fresh", "pages": 262144,
        })).unwrap();
        let (_, bytes) = crate::ledger_format::inspect_descriptor(&self.config, &bytes).unwrap();
        self.write(io::STATE, "mutation-ledger-v2.current.json", &bytes);
        self.write(io::STATE, "mutation-ledger-v2.lock", b"");
        for prefix in ["", "process-generation-1/", "process-generation-3/"] {
            for directory in ["mutation-authority", "runtime-mutations"] {
                self.write(
                    io::STATE,
                    &format!("{prefix}{directory}/{}", crate::ledger_format::BARRIER),
                    &bytes,
                );
            }
        }
        self.write(
            io::TRANSFER,
            &format!("runtime-state/{}", crate::ledger_format::BARRIER),
            &bytes,
        );
        self.store("mutation-ledger-v2");
    }
    fn retirement(&self, version: u8) {
        let active = std::fs::read(self.roots[io::STATE].join("mutation-ledger-v2.current.json"))
            .ok()
            .map(|bytes| sha256(&bytes));
        let bytes = serde_json::to_vec(&json!({
            "version": version, "archive": "22222222-2222-4222-8222-222222222222",
            "sandbox": self.config.sandbox_id, "home": self.config.home_identity,
            "state_root": self.config.state_root, "home_root": self.config.home,
            "transfer_root": self.config.checkpoint_root, "born": 1, "pages": 262144,
            "active_v2_sha256": active,
        }))
        .unwrap();
        self.write(io::STATE, "mutation-retirement-v1.current.json", &bytes);
        self.write(
            io::TRANSFER,
            &format!("runtime-state/{}", crate::retirement_format::BARRIER),
            &bytes,
        );
        self.store("mutation-retirement-v1");
    }
    fn store(&self, path: &str) {
        // Intentionally not a database: recognized metadata must not claim DB validation.
        self.write(
            io::STATE,
            &format!("{path}/ledger.sqlite3"),
            b"database contents unassessed",
        );
        self.write(io::STATE, &format!("{path}/writer.lock"), b"");
        self.write(io::STATE, &format!("{path}/emergency.reserve"), b"");
    }
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let mut entries = std::fs::read_dir(root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    out.insert(path.clone(), Vec::new());
                    collect(&path, out);
                } else {
                    out.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        let mut result = BTreeMap::new();
        collect(&self.root, &mut result);
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn request() -> Request {
    Request {
        version: 1,
        sandbox_id: "sandbox-inspection".into(),
        home_identity: "a".repeat(43),
        generation: 3,
        layout: "native-runtime-v1".into(),
    }
}
fn held(report: &Value) {
    assert_eq!(report["complete"], true);
    for field in [
        "writer_exclusion",
        "provider_identity",
        "external_immutability",
    ] {
        assert_eq!(report[field], "UNVERIFIED");
    }
    for field in [
        "database_contents",
        "retirement_transition_readiness",
        "native_session_recovery",
        "full_home_hash",
        "legacy_record_contents",
    ] {
        assert_eq!(report[field], "NOT_ASSESSED");
    }
    assert_eq!(report["restore_admission"], "HELD");
    assert_eq!(report["semantic_reconciliation"], "NOT_RUN");
}
