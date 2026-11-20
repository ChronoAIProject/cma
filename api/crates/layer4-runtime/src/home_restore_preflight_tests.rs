//! R1 V1/V3 probes exercise production startup, without a bridge or provider mock.
//! Passing refusal assertions reproduce a blocked design premise, not a restore.
use super::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "cma-home-restore-preflight-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config.home_identity = Some("a".repeat(43));
        config.state_root = root.join("state");
        config.checkpoint_root = root.join("transfer/cma-checkpoints");
        config.process_env.insert(
            "TMPDIR".into(),
            root.join("transfer").to_str().unwrap().into(),
        );
        let fixture = Self { root, config };
        for path in [
            &fixture.config.home,
            &fixture.config.state_root,
            &fixture.root.join("transfer"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        fixture
    }

    async fn start(&self) -> Result<Arc<Supervisor>> {
        Supervisor::start(self.config.clone(), SupervisorBoundary::default()).await
    }

    fn barrier(&self) -> PathBuf {
        self.config
            .checkpoint_root
            .join("runtime-state")
            .join(crate::ledger_format::BARRIER)
    }

    fn quarantine_internal_state(&mut self) {
        // Simulate R1 §3 with real rename operations on disposable local data.
        let quarantine = self.root.join("quarantine");
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::rename(&self.config.state_root, quarantine.join("state")).unwrap();
        std::fs::create_dir(&self.config.state_root).unwrap();
        let runtime_state = self.config.checkpoint_root.join("runtime-state");
        if runtime_state.exists() {
            std::fs::rename(&runtime_state, quarantine.join("runtime-state")).unwrap();
            let install = quarantine.join("runtime-state/install-v2.json");
            if install.exists() {
                std::fs::create_dir(&runtime_state).unwrap();
                std::fs::rename(install, runtime_state.join("install-v2.json")).unwrap();
            }
        }
        self.config.generation += 1;
        self.config.home_identity = Some("b".repeat(43));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn file_hash(path: &Path) -> [u8; 32] {
    Sha256::digest(std::fs::read(path).unwrap()).into()
}

#[tokio::test]
async fn v1_completed_install_without_binding_refuses_real_startup() {
    let mut fixture = Fixture::new();
    let supervisor = fixture.start().await.unwrap();
    let request = InstallRequest::new("restore-preflight-install", vec![]);
    let response = supervisor.dispatch(RequestFrame::Install(request)).await;
    assert!(
        matches!(response, ResponseFrame::Install(_)),
        "{response:?}"
    );
    drop(supervisor);

    // Preserve an existing agent home, native conversation and worktree shape.
    let agent_home = fixture.config.agent_home("existing-agent");
    std::fs::create_dir_all(agent_home.join("sessions")).unwrap();
    let session = agent_home.join("sessions/rollout.jsonl");
    let contents = b"{\"payload\":{\"id\":\"existing-native-conversation\"}}\n{torn";
    std::fs::write(&session, contents).unwrap();
    let worktree = fixture
        .config
        .home
        .join("project/.worktrees/existing-agent");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../../.git/worktrees/existing-agent\n",
    )
    .unwrap();
    let install = crate::install_state::inspect(&fixture.config)
        .unwrap()
        .unwrap();
    assert!(matches!(
        install,
        crate::install_state::InstallRecord::Completed { .. }
    ));
    let install_path = fixture
        .config
        .checkpoint_root
        .join("runtime-state/install-v2.json");
    let preserved_paths = [&session, &worktree.join(".git"), &install_path];
    let before = preserved_paths.map(|path| file_hash(path));
    fixture.quarantine_internal_state();
    assert_eq!(
        std::fs::read_dir(&fixture.config.state_root)
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        crate::install_state::inspect(&fixture.config).unwrap(),
        Some(install.clone())
    );

    let error = match fixture.start().await {
        Ok(supervisor) => {
            drop(supervisor);
            panic!("V1 premise changed: re-evaluate the restored-home design");
        }
        Err(error) => error,
    };
    eprintln!("V1 FAIL: {}: {}", error.code, error.detail);
    assert_eq!(error.code, "persistent_home_invalid");
    assert_eq!(
        error.detail,
        "an existing runtime home has no durable ownership binding"
    );
    assert!(!fixture
        .config
        .state_root
        .join("home-binding/binding.json")
        .exists());
    assert!(!fixture
        .config
        .state_root
        .join("mutation-ledger-v2.current.json")
        .exists());
    assert!(!fixture
        .config
        .state_root
        .join("mutation-ledger-v2")
        .exists());
    assert!(!fixture.barrier().exists());
    assert_eq!(
        crate::install_state::inspect(&fixture.config).unwrap(),
        Some(install)
    );
    assert_eq!(preserved_paths.map(|path| file_hash(path)), before);
    assert_eq!(std::fs::read(&session).unwrap(), contents);
    eprintln!("V1 boundary: binding/new ledger/agent startup validation not reached; no Codex process launched");
}

#[tokio::test]
async fn v3_transfer_barrier_without_state_refuses_real_startup() {
    let mut fixture = Fixture::new();
    let supervisor = fixture.start().await.unwrap();
    let barrier = std::fs::read(fixture.barrier()).unwrap();
    drop(supervisor);
    std::fs::rename(&fixture.config.state_root, fixture.root.join("old-state")).unwrap();
    std::fs::create_dir(&fixture.config.state_root).unwrap();
    fixture.config.generation += 1;
    fixture.config.home_identity = Some("b".repeat(43));

    let error = match fixture.start().await {
        Ok(supervisor) => {
            drop(supervisor);
            panic!("a retained transfer barrier must refuse a missing ledger");
        }
        Err(error) => error,
    };
    assert_eq!(error.code, "mutation_ledger_format_hold");
    assert_eq!(std::fs::read(fixture.barrier()).unwrap(), barrier);
    assert!(!fixture
        .config
        .state_root
        .join("home-binding/binding.json")
        .exists());
    eprintln!("V3 retained-barrier control: mutation_ledger_format_hold before new home binding");
}

#[tokio::test]
async fn v3_without_install_record_current_startup_recreates_all_five_barriers() {
    let mut fixture = Fixture::new();
    let supervisor = fixture.start().await.unwrap();
    let old_barrier = std::fs::read(fixture.barrier()).unwrap();
    drop(supervisor);
    std::fs::write(
        fixture.config.home.join("dirty-file"),
        b"preserve user content",
    )
    .unwrap();
    fixture.quarantine_internal_state();
    assert!(!fixture.barrier().exists());

    let supervisor = fixture.start().await.unwrap();
    let barrier = std::fs::read(fixture.barrier()).unwrap();
    assert_ne!(barrier, old_barrier);
    for scope in [
        String::new(),
        format!("process-generation-{}/", fixture.config.generation),
    ] {
        for directory in ["mutation-authority", "runtime-mutations"] {
            let path = fixture
                .config
                .state_root
                .join(format!("{scope}{directory}"))
                .join(crate::ledger_format::BARRIER);
            assert_eq!(std::fs::read(path).unwrap(), barrier);
        }
    }
    assert!(fixture
        .config
        .state_root
        .join("mutation-ledger-v2/ledger.sqlite3")
        .is_file());
    assert_eq!(
        std::fs::read(fixture.config.home.join("dirty-file")).unwrap(),
        b"preserve user content"
    );
    drop(supervisor);
    eprintln!("V3 control PASS: current Supervisor::start recreated transfer plus four state-root barriers; Completed-install scenario remains blocked by V1");
}
