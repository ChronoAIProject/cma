//! Execution authority and durable receipts are separate from Agent ownership.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use layer3_adapter::{
    ingress_budget::{IngressBudget, IngressReservation},
    resource_execution_protocol::*,
};
use tokio::sync::Semaphore;

pub(crate) mod bootstrap;
mod clock;
mod control;
mod gate;
mod journal;
mod runner;
mod start;
mod supervisor;
mod watchdog;

use clock::{BootClock, Clock};
use gate::Gate;
use journal::{Assignment, Journal, Snapshot};
use runner::{SharedExecutionRunner, UnavailableRunner};

struct State {
    journal: Option<Journal>,
    snapshot: Option<Snapshot>,
    gates: Vec<Arc<Gate>>,
    poisoned: bool,
    recovery_pending: bool,
}

pub(crate) struct Receiver {
    pub(crate) controls: Arc<Semaphore>,
    pub(crate) admissions: Arc<Semaphore>,
    identity: ReceiverKey,
    root: PathBuf,
    enabled: bool,
    state: Mutex<State>,
    runner: Arc<dyn SharedExecutionRunner>,
    clock: Arc<dyn Clock>,
    watchdog: Option<watchdog::Watchdog>,
    _custody: Option<IngressReservation>,
}

impl Receiver {
    pub(crate) fn open(
        config: &crate::config::RuntimeConfig,
        boot: &str,
    ) -> ProtocolResult<Arc<Self>> {
        use sha2::{Digest as _, Sha256};
        let assignment =
            serde_json::to_vec(&(&config.sandbox_id, config.generation, &config.home_identity))
                .map_err(|_| ProtocolError::InvalidIdentity)?;
        let identity = ReceiverKey {
            kind: ReceiverKind::Runtime,
            instance: OpaqueId::new(&format!(
                "{:x}",
                Sha256::digest(config.sandbox_id.as_bytes())
            ))?,
            incarnation: Digest::new(&format!("{:x}", Sha256::digest(assignment)))?,
            boot: OpaqueId::new(boot)?,
        };
        Self::with_runner(
            config.state_root.join("resource-execution-v1"),
            identity,
            config.resource_execution_fencing_enabled,
            Arc::new(UnavailableRunner),
            Arc::new(BootClock),
        )
    }

    fn with_runner(
        root: PathBuf,
        identity: ReceiverKey,
        enabled: bool,
        runner: Arc<dyn SharedExecutionRunner>,
        clock: Arc<dyn Clock>,
    ) -> ProtocolResult<Arc<Self>> {
        let namespace = [&root, &root.with_extension("activated")]
            .into_iter()
            .try_fold(false, |present, path| {
                match std::fs::symlink_metadata(path) {
                    Ok(_) => Ok(true),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(present),
                    Err(_) => Err(ProtocolError::Unknown),
                }
            })?;
        let custody = if namespace {
            Some(
                IngressReservation::try_acquire(
                    &Arc::new(Semaphore::new(16 * 1024 * 1024)),
                    Some(&IngressBudget::process()),
                    16 * 1024 * 1024,
                )
                .ok_or(ProtocolError::Capacity)?,
            )
        } else {
            None
        };
        let loaded = Journal::load(
            root.clone(),
            &Assignment {
                instance: identity.instance.clone(),
                incarnation: identity.incarnation.clone(),
            },
        )?;
        let (journal, snapshot) = loaded.map_or((None, None), |(journal, snapshot)| {
            (Some(journal), Some(snapshot))
        });
        let gates: Vec<_> = snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
            snapshot
                .records
                .iter()
                .map(|record| Arc::new(Gate::new(record.challenge.deadline_tick_ms, true)))
                .collect()
        });
        let watchdog = if let Some(snapshot) = &snapshot {
            let watchdog = watchdog::Watchdog::start(clock.clone(), runner.clone())
                .map_err(|_| ProtocolError::Unavailable)?;
            for (record, gate) in snapshot.records.iter().zip(&gates) {
                if let Some(cell) = &record.cell {
                    watchdog
                        .register(cell.clone(), gate.clone())
                        .map_err(|_| ProtocolError::Capacity)?;
                }
            }
            Some(watchdog)
        } else {
            None
        };
        let recovery_pending = snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .records
                .iter()
                .any(|record| record.cell.is_some() && record.stop.is_none())
        });
        Ok(Arc::new(Self {
            controls: Arc::new(Semaphore::new(8)),
            admissions: Arc::new(Semaphore::new(4)),
            identity,
            root,
            enabled,
            state: Mutex::new(State {
                journal,
                snapshot,
                gates,
                poisoned: false,
                recovery_pending,
            }),
            runner,
            clock,
            watchdog,
            _custody: custody,
        }))
    }

    pub(crate) fn legacy_allowed(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if self.enabled || state.poisoned || state.snapshot.is_some() {
            return false;
        }
        let absent = [&self.root, &self.root.with_extension("activated")]
            .into_iter()
            .all(|path| {
                std::fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            });
        if !absent {
            state.poison();
        }
        absent
    }

    pub(crate) fn identity(&self) -> &ReceiverKey {
        &self.identity
    }

    pub(crate) fn capabilities(&self) -> Option<ExecutionCapabilities> {
        let state = self.state.lock().ok()?;
        if !self.enabled || state.poisoned || state.recovery_pending || state.snapshot.is_none() {
            return None;
        }
        self.runner.capabilities().filter(|caps| {
            caps.protocol == PROTOCOL_VERSION && caps.targeted_native_stop && caps.mediated_effects
        })
    }

    fn state(&self) -> ProtocolResult<std::sync::MutexGuard<'_, State>> {
        let mut state = self.state.lock().map_err(|_| ProtocolError::Unknown)?;
        if state.poisoned {
            return Err(ProtocolError::Unknown);
        }
        let journal = state.journal.as_ref().ok_or(ProtocolError::Unsupported)?;
        if let Err(error) = journal.verify() {
            state.poison();
            return Err(error);
        }
        Ok(state)
    }

    pub(crate) async fn recover(self: &Arc<Self>) -> ProtocolResult<()> {
        let cells = {
            let state = self.state.lock().map_err(|_| ProtocolError::Unknown)?;
            if state.snapshot.is_none() {
                return Ok(());
            }
            state.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
                snapshot
                    .records
                    .iter()
                    .filter(|r| r.stop.is_none())
                    .filter_map(|r| r.cell.clone())
                    .collect()
            })
        };
        let mut result = Ok(());
        for cell in cells {
            if let Err(error) = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                self.runner.recover(&cell),
            )
            .await
            .unwrap_or(Err(ProtocolError::Unknown))
            {
                result = Err(error);
            }
        }
        if result.is_ok() {
            self.state()?.recovery_pending = false;
        }
        result
    }
}

impl State {
    fn poison(&mut self) {
        self.poisoned = true;
        for gate in &self.gates {
            gate.close();
        }
    }

    fn publish(&mut self, snapshot: Snapshot) -> ProtocolResult<()> {
        if let Err(error) = self
            .journal
            .as_mut()
            .ok_or(ProtocolError::Unsupported)?
            .save(&snapshot)
        {
            self.poison();
            return Err(error);
        }
        self.snapshot = Some(snapshot);
        Ok(())
    }
}

pub(crate) fn enabled(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err("CHRONO_RESOURCE_EXECUTION_FENCING_ENABLED must be false or true".into()),
    }
}

#[cfg(test)]
mod tests;
