use super::*;
use io::RecoveryIo;
use std::cell::RefCell;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    File(PathBuf),
    Rename(PathBuf, PathBuf),
    Directory(PathBuf),
    Remove(PathBuf),
}

impl From<RecoveryIo<'_>> for Step {
    fn from(operation: RecoveryIo<'_>) -> Self {
        match operation {
            RecoveryIo::SyncFile(path) => Self::File(path.into()),
            RecoveryIo::Rename(source, target) => Self::Rename(source.into(), target.into()),
            RecoveryIo::SyncDirectory(path) => Self::Directory(path.into()),
            RecoveryIo::Remove(path) => Self::Remove(path.into()),
        }
    }
}

#[derive(Default)]
struct Trace {
    fail: Option<Step>,
    attempted: Vec<Step>,
    completed: Vec<Step>,
}

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

pub(super) struct FaultGuard;
impl FaultGuard {
    pub(super) fn arm(fail: Option<Step>) -> Self {
        TRACE.with_borrow_mut(|trace| {
            assert!(trace.is_none(), "journal fault scopes must not nest");
            *trace = Some(Trace {
                fail,
                ..Trace::default()
            });
        });
        Self
    }

    pub(super) fn attempted(&self) -> Vec<Step> {
        TRACE.with_borrow(|trace| trace.as_ref().unwrap().attempted.clone())
    }

    pub(super) fn completed(&self) -> Vec<Step> {
        TRACE.with_borrow(|trace| trace.as_ref().unwrap().completed.clone())
    }
}
impl Drop for FaultGuard {
    fn drop(&mut self) {
        TRACE.with_borrow_mut(|trace| *trace = None);
    }
}

pub(in crate::mutation_journal) fn before_io(operation: RecoveryIo<'_>) -> std::io::Result<()> {
    TRACE.with_borrow_mut(|trace| {
        let Some(trace) = trace else { return Ok(()) };
        let step = Step::from(operation);
        trace.attempted.push(step.clone());
        if trace.fail.as_ref() == Some(&step) {
            Err(std::io::Error::other("injected journal durability failure"))
        } else {
            Ok(())
        }
    })
}

pub(in crate::mutation_journal) fn after_io(operation: RecoveryIo<'_>) {
    TRACE.with_borrow_mut(|trace| {
        if let Some(trace) = trace {
            trace.completed.push(operation.into());
        }
    });
}

pub(super) struct Fixture {
    pub config: RuntimeConfig,
    pub operation: Operation,
    pub path: PathBuf,
    pub temporary: PathBuf,
    pub started_bytes: Vec<u8>,
    pub terminal_bytes: Vec<u8>,
    pub terminal: Record,
}
impl Fixture {
    pub(super) fn new(name: &str, status: Status, promote: bool) -> Self {
        Self::with_config(config(name), status, promote)
    }

    pub(super) fn with_config(config: RuntimeConfig, status: Status, promote: bool) -> Self {
        let operation = Operation::add_worktree(&request());
        let ticket = begin_ticket::<WorktreeReceipt>(&config, &operation);
        let started_bytes = std::fs::read(&ticket.path).unwrap();
        let mut terminal = ticket.record;
        terminal.status = status;
        match status {
            Status::Completed => {
                terminal.receipt =
                    Some(serde_json::to_value(receipt(&config, &request())).unwrap());
            }
            Status::Failed | Status::Indeterminate => {
                terminal.failure = Some(stored_failure(&RuntimeError::rejected(
                    if status == Status::Failed {
                        "fixture_refused"
                    } else {
                        "runtime_mutation_outcome_unknown"
                    },
                    "original terminal detail",
                )));
            }
            Status::Started => panic!("fixture requires a terminal outcome"),
        }
        let terminal_bytes = serde_json::to_vec_pretty(&terminal).unwrap();
        let temporary = ticket.path.with_file_name(".mutation-journal-terminal.tmp");
        std::fs::write(&temporary, &terminal_bytes).unwrap();
        if !promote {
            // The normal writer syncs contents before rename; its directory sync
            // may not have completed when the next reader sees the terminal name.
            crate::fs_durability::sync_file(&temporary).unwrap();
            std::fs::rename(&temporary, &ticket.path).unwrap();
        }
        Self {
            config,
            operation,
            path: ticket.path,
            temporary,
            started_bytes,
            terminal_bytes,
            terminal,
        }
    }

    pub(super) fn directory(&self) -> &Path {
        self.path.parent().unwrap()
    }

    pub(super) fn assert_replay(&self) {
        let found = begin::<WorktreeReceipt>(&self.config, &self.operation).unwrap();
        match (self.terminal.status, found) {
            (Status::Completed, Begin::Completed(found)) => {
                assert_eq!(found, receipt(&self.config, &request()))
            }
            (Status::Failed, Begin::Failed(error))
            | (Status::Indeterminate, Begin::Indeterminate(error)) => {
                assert_eq!(
                    stored_failure(&error),
                    self.terminal.failure.clone().unwrap()
                );
            }
            (_, other) => panic!("terminal must never become executable again: {other:?}"),
        }
        assert_eq!(std::fs::read(&self.path).unwrap(), self.terminal_bytes);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        cleanup(&self.config);
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Entrance {
    Replay,
    Recovery,
}
impl Entrance {
    pub(super) fn run(self, fixture: &Fixture) -> Result<()> {
        match self {
            Self::Replay => {
                begin::<WorktreeReceipt>(&fixture.config, &fixture.operation).map(|_| ())
            }
            Self::Recovery => started(&fixture.config).map(|pending| assert!(pending.is_empty())),
        }
    }
}

fn canonical_failure(directory: bool, entrance: Entrance) {
    for status in [Status::Completed, Status::Failed, Status::Indeterminate] {
        let fixture = Fixture::new(
            &format!("canonical-{directory}-{status:?}-{entrance:?}"),
            status,
            false,
        );
        let failure = if directory {
            Step::Directory(fixture.directory().into())
        } else {
            Step::File(fixture.path.clone())
        };
        {
            let guard = FaultGuard::arm(Some(failure.clone()));
            let error = entrance
                .run(&fixture)
                .expect_err("terminal acceptance requires a durability barrier");
            assert_eq!(error.code, "mutation_journal_io_failed");
            assert!(error.retryable);
            assert_eq!(guard.attempted().last(), Some(&failure));
            assert_eq!(
                std::fs::read(&fixture.path).unwrap(),
                fixture.terminal_bytes
            );
        }
        let guard = FaultGuard::arm(None);
        entrance.run(&fixture).unwrap();
        assert_eq!(
            guard.completed(),
            vec![
                Step::File(fixture.path.clone()),
                Step::Directory(fixture.directory().into())
            ]
        );
        fixture.assert_replay();
    }
}

#[test]
fn canonical_terminal_file_sync_failure_holds_replay() {
    canonical_failure(false, Entrance::Replay);
}

#[test]
fn canonical_terminal_file_sync_failure_holds_recovery() {
    canonical_failure(false, Entrance::Recovery);
}

#[test]
fn canonical_terminal_directory_sync_failure_holds_replay() {
    canonical_failure(true, Entrance::Replay);
}

#[test]
fn canonical_terminal_directory_sync_failure_holds_recovery() {
    canonical_failure(true, Entrance::Recovery);
}

#[test]
fn started_records_keep_existing_recovery_without_terminal_barriers() {
    let fixture = Fixture::new("started-has-no-terminal-barrier", Status::Completed, true);
    std::fs::remove_file(&fixture.temporary).unwrap();
    let guard = FaultGuard::arm(None);
    assert!(matches!(
        begin::<WorktreeReceipt>(&fixture.config, &fixture.operation).unwrap(),
        Begin::Started(_)
    ));
    assert_eq!(started(&fixture.config).unwrap().len(), 1);
    assert!(guard.attempted().is_empty());
}

#[test]
fn mismatched_replay_and_corrupt_terminal_are_refused_before_sync() {
    let fixture = Fixture::new("validate-before-barrier", Status::Completed, false);
    let guard = FaultGuard::arm(Some(Step::File(fixture.path.clone())));
    let mut changed = request();
    changed.base = "f".repeat(40);
    changed.request_sha256 = changed.computed_sha256();
    assert_eq!(
        begin::<WorktreeReceipt>(&fixture.config, &Operation::add_worktree(&changed))
            .unwrap_err()
            .code,
        "runtime_mutation_replay_mismatch"
    );
    let mut wrong = fixture.terminal.clone();
    wrong.receipt.as_mut().unwrap()["path"] = serde_json::json!("invalid-receipt-path");
    std::fs::write(&fixture.path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    for entrance in [Entrance::Replay, Entrance::Recovery] {
        assert_eq!(
            entrance.run(&fixture).unwrap_err().code,
            "mutation_journal_corrupt"
        );
    }
    assert!(guard.attempted().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_store_propagates_barrier_failure_and_replays_original_outcome() {
    use crate::mutation_store::{Begin as StoreBegin, Cursor, Store};
    for status in [Status::Completed, Status::Failed, Status::Indeterminate] {
        let mut fixture = Fixture::new(&format!("store-barrier-{status:?}"), status, false);
        fixture.config.legacy_journal_recovery_enabled = true;
        let store = Store::new(fixture.config.clone(), None);
        for failure in [
            Step::File(fixture.path.clone()),
            Step::Directory(fixture.directory().into()),
        ] {
            let _guard = FaultGuard::arm(Some(failure));
            assert_eq!(
                store
                    .begin::<WorktreeReceipt>(&fixture.operation)
                    .await
                    .unwrap_err()
                    .code,
                "mutation_journal_io_failed"
            );
            let error = match store.started(&mut Cursor::default()).await {
                Err(error) => error,
                Ok(_) => panic!("store startup must hold on journal sync failure"),
            };
            assert_eq!(error.code, "mutation_journal_io_failed");
        }
        // Startup retries begin a fresh scan, as they do for a new supervisor.
        assert!(store
            .started(&mut Cursor::default())
            .await
            .unwrap()
            .is_empty());
        match (
            status,
            store
                .begin::<WorktreeReceipt>(&fixture.operation)
                .await
                .unwrap(),
        ) {
            (Status::Completed, StoreBegin::Completed(found)) => {
                assert_eq!(found, receipt(&fixture.config, &request()))
            }
            (Status::Failed, StoreBegin::Failed(error))
            | (Status::Indeterminate, StoreBegin::Indeterminate(error)) => {
                assert_eq!(
                    stored_failure(&error),
                    fixture.terminal.failure.clone().unwrap()
                );
            }
            (_, other) => panic!("store must retain original terminal outcome: {other:?}"),
        }
    }
}

// Successful real IO calls drive this storage model. Directory entries and
// inode contents persist independently; dropping the caches is not an OS reboot.
struct CacheModel {
    names: BTreeMap<PathBuf, u8>,
    durable_names: BTreeMap<PathBuf, u8>,
    bytes: BTreeMap<u8, Vec<u8>>,
    durable_bytes: BTreeMap<u8, Vec<u8>>,
}
impl CacheModel {
    fn interrupted(fixture: &Fixture, promotion: bool) -> Self {
        let mut model = Self {
            names: BTreeMap::from([(fixture.path.clone(), 1)]),
            durable_names: BTreeMap::from([(fixture.path.clone(), 1)]),
            bytes: BTreeMap::from([
                (1, fixture.started_bytes.clone()),
                (2, fixture.terminal_bytes.clone()),
            ]),
            durable_bytes: BTreeMap::from([(1, fixture.started_bytes.clone())]),
        };
        if promotion {
            model.names.insert(fixture.temporary.clone(), 2);
            model.durable_names.insert(fixture.temporary.clone(), 2);
        } else {
            model.names.insert(fixture.path.clone(), 2);
            model
                .durable_bytes
                .insert(2, fixture.terminal_bytes.clone());
        }
        model
    }

    fn apply(&mut self, step: &Step) {
        match step {
            Step::File(path) => {
                let inode = self.names[path];
                self.durable_bytes.insert(inode, self.bytes[&inode].clone());
            }
            Step::Rename(source, target) => {
                let inode = self.names.remove(source).unwrap();
                self.names.insert(target.clone(), inode);
            }
            Step::Directory(directory) => {
                self.durable_names
                    .retain(|path, _| path.parent() != Some(directory));
                for (path, inode) in &self.names {
                    if path.parent() == Some(directory) {
                        self.durable_names.insert(path.clone(), *inode);
                    }
                }
            }
            Step::Remove(path) => {
                self.names.remove(path);
                self.durable_names.remove(path);
            }
        }
    }

    fn after_cache_loss(self) -> BTreeMap<PathBuf, Vec<u8>> {
        self.durable_names
            .into_iter()
            .map(|(path, inode)| {
                (
                    path,
                    self.durable_bytes.get(&inode).cloned().unwrap_or_default(),
                )
            })
            .collect()
    }
}

#[test]
fn production_barriers_preserve_terminal_across_modeled_cache_loss_and_second_recovery() {
    for promotion in [false, true] {
        for status in [Status::Completed, Status::Failed, Status::Indeterminate] {
            for entrance in [Entrance::Replay, Entrance::Recovery] {
                let fixture = Fixture::new(
                    &format!("cache-loss-{promotion}-{status:?}-{entrance:?}"),
                    status,
                    promotion,
                );
                let steps = {
                    let guard = FaultGuard::arm(None);
                    entrance.run(&fixture).unwrap();
                    guard.completed()
                };
                fixture.assert_replay();
                let mut model = CacheModel::interrupted(&fixture, promotion);
                for step in &steps {
                    model.apply(step);
                }
                let recovered = model.after_cache_loss();
                assert_eq!(
                    recovered,
                    BTreeMap::from([(fixture.path.clone(), fixture.terminal_bytes.clone())])
                );

                // Negative controls: a directory barrier alone cannot persist
                // unsynced temp contents, and file sync cannot persist a rename.
                for omit_file in if promotion {
                    vec![true, false]
                } else {
                    vec![false]
                } {
                    let mut missing_barrier = CacheModel::interrupted(&fixture, promotion);
                    for step in &steps {
                        if (omit_file && matches!(step, Step::File(_)))
                            || (!omit_file && matches!(step, Step::Directory(_)))
                        {
                            continue;
                        }
                        missing_barrier.apply(step);
                    }
                    assert_ne!(missing_barrier.after_cache_loss(), recovered);
                }

                std::fs::remove_dir_all(fixture.directory()).unwrap();
                std::fs::create_dir_all(fixture.directory()).unwrap();
                for (path, bytes) in recovered {
                    std::fs::write(path, bytes).unwrap();
                }
                assert!(started(&fixture.config).unwrap().is_empty());
                fixture.assert_replay();
            }
        }
    }
}

#[test]
fn owner_stop_close_proof_checks_durability_of_the_visible_terminal_record() {
    let fixture = Fixture::new("close-proof-durability", Status::Completed, false);
    let path = &fixture.path;
    assert!(!fixture.temporary.exists());
    let guard = FaultGuard::arm(Some(Step::Directory(fixture.directory().into())));
    assert_eq!(
        find_proof_record(
            &fixture.config,
            &fixture.operation.operation_id,
            &fixture.operation.subject
        )
        .unwrap_err()
        .code,
        "mutation_journal_io_failed"
    );
    assert_eq!(guard.completed(), [Step::File(path.clone())]);
    assert_eq!(std::fs::read(path).unwrap(), fixture.terminal_bytes);
    drop(guard);

    let guard = FaultGuard::arm(None);
    let proof = find_proof_record(
        &fixture.config,
        &fixture.operation.operation_id,
        &fixture.operation.subject,
    )
    .unwrap()
    .unwrap();
    assert_eq!(proof, fixture.terminal);
    assert_eq!(
        guard.completed(),
        [
            Step::File(path.clone()),
            Step::Directory(fixture.directory().into())
        ]
    );
}
