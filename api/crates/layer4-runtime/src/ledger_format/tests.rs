use super::*;
use crate::mutation_journal::Operation;
use layer3_adapter::{AgentCloseRequest, AgentSlot};
use std::path::PathBuf;

static FAIL: std::sync::Mutex<std::collections::BTreeMap<PathBuf, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
fn set_failure(config: &RuntimeConfig, step: &str) {
    FAIL.lock()
        .unwrap()
        .insert(config.state_root.clone(), step.into());
}
pub(super) fn checkpoint(root: &std::path::Path, step: &str) -> Result<()> {
    if FAIL
        .lock()
        .unwrap()
        .get(root)
        .is_some_and(|value| value == step)
    {
        return Err(hold());
    }
    Ok(())
}
struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-ledger-format-{}", uuid::Uuid::new_v4()));
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config.state_root = root.join("state");
        config.checkpoint_root = root.join("transfer/cma-checkpoints");
        config.home_identity = Some("a".repeat(43));
        config.process_env.insert(
            "TMPDIR".into(),
            root.join("transfer").to_str().unwrap().into(),
        );
        for path in [&config.home, &config.state_root, &root.join("transfer")] {
            std::fs::create_dir_all(path).unwrap();
        }
        Self { root, config }
    }
    async fn open(&self) -> Result<Option<Arc<Ledger>>> {
        open(&self.config).await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        FAIL.lock().unwrap().remove(&self.config.state_root);
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
async fn open(config: &RuntimeConfig) -> Result<Option<Arc<Ledger>>> {
    let _writer = crate::workspace_removal_writer::claim(config, "format-test")?;
    let selection = select(config)?;
    crate::persistent_home::initialize(config)?;
    crate::workspace_removal_writer::publish_claim(config, "format-test")?;
    selection.open(config).await
}
async fn close(ledger: Arc<Ledger>) {
    Arc::try_unwrap(ledger).ok().unwrap().close().await.unwrap();
}
fn operation() -> Operation {
    Operation::close_agent(&AgentCloseRequest::new(
        "agent:format:close:v1",
        AgentSlot::new("format"),
    ))
}

#[tokio::test]
async fn fresh_retained_home_commits_manifest_and_reopens_the_exact_store() {
    let fixture = Fixture::new();
    let ledger = fixture.open().await.unwrap().unwrap();
    let store = ledger.store_id().await.unwrap();
    ledger.admit(u64::MAX, vec![operation()]).await.unwrap();
    let record = ledger.begin(operation()).await.unwrap().record;
    let failure = crate::failure::RuntimeError::rejected("synthetic", "synthetic failure");
    ledger
        .finish(record.failed(&failure, false).unwrap())
        .await
        .unwrap();
    assert!(crate::install_state::inspect(&fixture.config)
        .unwrap()
        .is_none());
    close(ledger).await;
    let ledger = fixture.open().await.unwrap().unwrap();
    assert_eq!(ledger.store_id().await.unwrap(), store);
    assert_eq!(
        ledger
            .begin(operation())
            .await
            .unwrap()
            .record
            .error()
            .unwrap()
            .code,
        "synthetic"
    );
    close(ledger).await;
}

#[tokio::test]
async fn legacy_binding_nonempty_ledgers_and_no_home_never_select_fresh_from_an_empty_generation() {
    for kind in ["bound", "journal", "authority", "no-home"] {
        let mut fixture = Fixture::new();
        match kind {
            "bound" => {
                crate::persistent_home::initialize(&fixture.config).unwrap();
            }
            "no-home" => fixture.config.home_identity = None,
            directory => {
                std::fs::create_dir(fixture.config.state_root.join(directory)).unwrap();
            }
        }
        fixture.config.generation = 2;
        let selection = select(&fixture.config).unwrap();
        assert!(!selection.fresh);
        assert!(selection.open(&fixture.config).await.unwrap().is_none());
        assert!(!fixture.config.state_root.join(MANIFEST).exists());
    }
}

#[tokio::test]
async fn interrupted_publication_holds_until_manifest_commit_without_fallback() {
    for step in [
        "locked",
        "database",
        "closed",
        "barriers",
        "renamed",
        "temporary",
        "committed",
    ] {
        let fixture = Fixture::new();
        set_failure(&fixture.config, step);
        assert!(fixture.open().await.is_err(), "{step}");
        set_failure(&fixture.config, "");
        // The worker releases its lock on sender destruction; the outer lock
        // never authorizes a second writer before the connection is gone.
        if step == "committed" {
            close(fixture.open().await.unwrap().unwrap()).await;
        } else {
            assert!(fixture.open().await.is_err(), "partial {step} must hold");
        }
    }
}

#[tokio::test]
async fn generation_transition_preserves_persistent_tombstones_and_refuses_missing_or_partial_scope(
) {
    let mut fixture = Fixture::new();
    let ledger = fixture.open().await.unwrap().unwrap();
    let removal = Operation::remove_agent_home(&layer3_adapter::AgentHomeRemoveRequest::new(
        "agent:format:home:remove:v1",
        AgentSlot::new("format"),
    ));
    ledger.admit(1, vec![removal.clone()]).await.unwrap();
    let store = ledger.store_id().await.unwrap();
    close(ledger).await;
    fixture.config.generation = 2;
    let ledger = fixture.open().await.unwrap().unwrap();
    assert_eq!(ledger.store_id().await.unwrap(), store);
    assert_eq!(ledger.recovery_fence(removal).await.unwrap(), Some(1));
    assert_eq!(ledger.recovery_fence(operation()).await.unwrap(), None);
    let descriptor = Descriptor::load(&fixture.config).unwrap();
    std::fs::remove_file(&descriptor.markers(2)[0]).unwrap();
    assert!(ledger.admit(1, vec![operation()]).await.is_err());
    close(ledger).await;
    assert!(fixture.open().await.is_err());
    fixture.config.generation = 1;
    assert!(fixture.open().await.is_err());
}

#[tokio::test]
async fn competing_writer_missing_manifest_swapped_store_and_bad_markers_hold() {
    let fixture = Fixture::new();
    let ledger = fixture.open().await.unwrap().unwrap();
    assert_eq!(
        select(&fixture.config).err().unwrap().code,
        "mutation_ledger_writer_held"
    );
    let descriptor = Descriptor::load(&fixture.config).unwrap();
    let path = descriptor.install();
    let original = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{}").unwrap();
    assert!(ledger.snapshot().await.is_err());
    std::fs::write(&path, original).unwrap();
    close(ledger).await;
    let manifest = fixture.config.state_root.join(MANIFEST);
    let bytes = std::fs::read(&manifest).unwrap();
    std::fs::remove_file(&manifest).unwrap();
    assert!(fixture.open().await.is_err());
    io::create(&manifest, &bytes).unwrap();
    let mut changed = descriptor;
    changed.store = uuid::Uuid::new_v4().to_string();
    std::fs::write(&manifest, changed.bytes().unwrap()).unwrap();
    for path in changed.base_markers() {
        std::fs::write(path, changed.bytes().unwrap()).unwrap();
    }
    assert!(fixture.open().await.is_err());
}

#[tokio::test]
async fn unsupported_transfer_symlinks_and_unknown_installation_entries_hold() {
    let mut fixture = Fixture::new();
    fixture.config.process_env.remove("TMPDIR");
    assert!(fixture.open().await.is_err());
    fixture.config.process_env.insert(
        "TMPDIR".into(),
        fixture.root.join("transfer").to_str().unwrap().into(),
    );
    let ledger = fixture.open().await.unwrap().unwrap();
    close(ledger).await;
    let marker = fixture
        .config
        .checkpoint_root
        .join("runtime-state")
        .join(BARRIER);
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink(fixture.config.state_root.join(MANIFEST), &marker).unwrap();
    assert!(fixture.open().await.is_err());
    std::fs::remove_file(&marker).unwrap();
    let bytes = std::fs::read(fixture.config.state_root.join(MANIFEST)).unwrap();
    io::create(&marker, &bytes).unwrap();
    std::fs::write(marker.parent().unwrap().join("unexpected"), b"{}").unwrap();
    assert!(crate::install_state::inspect(&fixture.config).is_err());
}

#[tokio::test]
async fn frozen_v1_reader_refuses_before_installation_recovery_and_all_mutation_scopes_refuse() {
    let fixture = Fixture::new();
    let ledger = fixture.open().await.unwrap().unwrap();
    // The exact old install scan runs before desktop/Agent launch or workspace
    // recovery. Keep a Started install untouched to prove refusal precedes it.
    let request = layer3_adapter::InstallRequest::new("install-downgrade-test", vec![]);
    let record = crate::install_state::InstallRecord::started(&fixture.config, &request);
    crate::install_state::write_initial(&fixture.config, &record).unwrap();
    let path = fixture
        .config
        .checkpoint_root
        .join("runtime-state/install-v2.json");
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        v1_reader::recover(&fixture.config).unwrap_err().code,
        "install_state_invalid"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(crate::mutation_journal::started(&fixture.config).is_err());
    assert!(crate::mutation_authority::admit(&fixture.config, 1, &operation()).is_err());
    // Removing one defense-in-depth marker does not grant an old reader
    // permission in the other scope, nor a new reader permission to fall back.
    let descriptor = Descriptor::load(&fixture.config).unwrap();
    std::fs::remove_file(&descriptor.markers(1)[0]).unwrap();
    assert!(crate::mutation_authority::admit(&fixture.config, 1, &operation()).is_err());
    assert!(ledger.admit(1, vec![operation()]).await.is_err());
    close(ledger).await;
}

#[tokio::test]
async fn actual_startup_uses_v2_and_refuses_before_desktop_or_legacy_recovery() {
    let fixture = Fixture::new();
    let supervisor = crate::supervisor::Supervisor::start(
        fixture.config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .await
    .unwrap();
    assert!(fixture.config.state_root.join(MANIFEST).exists());
    let response = supervisor
        .dispatch(layer3_adapter::wire::RequestFrame::Install(
            layer3_adapter::InstallRequest::new("format-install", vec![]),
        ))
        .await;
    assert!(
        matches!(response, layer3_adapter::wire::ResponseFrame::Install(_)),
        "{response:?}"
    );
    let descriptor = Descriptor::load(&fixture.config).unwrap();
    std::fs::write(
        fixture
            .config
            .state_root
            .join("runtime-mutations/unexpected"),
        b"{}",
    )
    .unwrap();
    let response = supervisor
        .dispatch(layer3_adapter::wire::RequestFrame::Install(
            layer3_adapter::InstallRequest::new("format-install", vec![]),
        ))
        .await;
    assert!(
        matches!(response, layer3_adapter::wire::ResponseFrame::Failure(ref f) if f.code == "mutation_ledger_format_hold")
    );
    drop(supervisor);
    std::fs::remove_file(descriptor.install()).unwrap();
    let mut desktop = fixture.config.clone();
    desktop.machine_included = true;
    assert!(crate::supervisor::Supervisor::start(
        desktop,
        crate::writer_fence::SupervisorBoundary::default()
    )
    .await
    .is_err());
}

#[tokio::test]
async fn replacing_database_directory_cannot_bypass_the_outer_lock_or_keep_serving() {
    let fixture = Fixture::new();
    let ledger = fixture.open().await.unwrap().unwrap();
    let database = fixture.config.state_root.join(DATABASE);
    let moved = fixture.config.state_root.join("moved-database");
    std::fs::rename(&database, &moved).unwrap();
    std::fs::create_dir(&database).unwrap();
    assert_eq!(
        select(&fixture.config).err().unwrap().code,
        "mutation_ledger_writer_held"
    );
    assert_eq!(
        ledger.admit(1, vec![operation()]).await.unwrap_err().code,
        "mutation_ledger_format_hold"
    );
    close(ledger).await;
    assert!(fixture.open().await.is_err());
}

#[tokio::test]
async fn interrupted_new_generation_barriers_hold_without_rebinding_the_database() {
    let mut fixture = Fixture::new();
    let ledger = fixture.open().await.unwrap().unwrap();
    close(ledger).await;
    fixture.config.generation = 2;
    set_failure(&fixture.config, "barrier-0");
    assert!(fixture.open().await.is_err());
    set_failure(&fixture.config, "");
    assert!(fixture.open().await.is_err());
    let db = rusqlite::Connection::open(
        fixture
            .config
            .state_root
            .join(DATABASE)
            .join("ledger.sqlite3"),
    )
    .unwrap();
    let generation: Vec<u8> = db
        .query_row("SELECT generation FROM metadata", [], |row| row.get(0))
        .unwrap();
    assert_eq!(generation, 1u64.to_be_bytes());
}
