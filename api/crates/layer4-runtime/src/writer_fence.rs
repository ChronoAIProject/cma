use std::collections::BTreeSet;

#[cfg(target_os = "linux")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock, RwLock, RwLockReadGuard,
};

#[cfg(target_os = "linux")]
struct ChildSupervisor {
    root: i32,
    spawn_gate: RwLock<()>,
    managed: Mutex<BTreeSet<i32>>,
    reaper_started: AtomicBool,
}

#[cfg(target_os = "linux")]
static CHILD_SUPERVISOR: OnceLock<ChildSupervisor> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SupervisorBoundary {
    #[cfg(target_os = "linux")]
    pub(super) root: Option<i32>,
}

impl SupervisorBoundary {
    pub(crate) fn require_no_writers(self) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if let Some(root) = self.root {
            let supervisor = CHILD_SUPERVISOR.get().ok_or("writer boundary is absent")?;
            let _spawn = supervisor
                .spawn_gate
                .write()
                .map_err(|_| "writer spawn gate is poisoned")?;
            return crate::writer_process::require_no_other_processes(root);
        }
        if cfg!(test) {
            Ok(())
        } else {
            Err("process absence requires a supervisor writer boundary".into())
        }
    }

    pub(crate) fn install() -> Result<Self, String> {
        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self::default())
        }
        #[cfg(target_os = "linux")]
        {
            let root = i32::try_from(std::process::id())
                .map_err(|_| "runtime supervisor process ID exceeds pid_t".to_string())?;
            if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
                return Err(format!(
                    "establish writer child subreaper: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut enabled: libc::c_int = 0;
            if unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut enabled) } != 0
                || enabled != 1
            {
                return Err("writer child subreaper was not retained".into());
            }
            let supervisor = CHILD_SUPERVISOR.get_or_init(|| ChildSupervisor {
                root,
                spawn_gate: RwLock::new(()),
                managed: Mutex::new(BTreeSet::new()),
                reaper_started: AtomicBool::new(false),
            });
            if supervisor.root != root {
                return Err("writer child subreaper belongs to another process".into());
            }
            if supervisor
                .reaper_started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                std::thread::Builder::new()
                    .name("cma-adopted-child-reaper".into())
                    .spawn(reap_adopted_children)
                    .map_err(|error| format!("start adopted child reaper: {error}"))?;
            }
            Ok(Self { root: Some(root) })
        }
    }
}

pub(crate) struct ManagedSpawnPermit {
    #[cfg(target_os = "linux")]
    gate: Option<RwLockReadGuard<'static, ()>>,
}

impl ManagedSpawnPermit {
    pub(crate) fn begin() -> Self {
        #[cfg(not(target_os = "linux"))]
        {
            Self {}
        }
        #[cfg(target_os = "linux")]
        {
            let gate = CHILD_SUPERVISOR.get().map(|supervisor| {
                supervisor
                    .spawn_gate
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            });
            Self { gate }
        }
    }

    pub(crate) fn register(self, pid: Option<u32>) -> ManagedChild {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            ManagedChild {}
        }
        #[cfg(target_os = "linux")]
        {
            let mut this = self;
            let pid = pid.and_then(|pid| i32::try_from(pid).ok());
            if let (Some(supervisor), Some(pid)) = (CHILD_SUPERVISOR.get(), pid) {
                supervisor
                    .managed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(pid);
            }
            this.gate.take();
            ManagedChild { pid }
        }
    }
}

pub(crate) struct ManagedChild {
    #[cfg(target_os = "linux")]
    pid: Option<i32>,
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let (Some(supervisor), Some(pid)) = (CHILD_SUPERVISOR.get(), self.pid) {
            supervisor
                .managed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&pid);
        }
    }
}

pub(crate) struct WriterFence {
    stopped: Vec<i32>,
    writer_count: u32,
    resumed: bool,
}

impl WriterFence {
    pub(crate) fn quiesce(
        writer_roots: impl IntoIterator<Item = i32>,
        supervisor: SupervisorBoundary,
    ) -> Result<Self, String> {
        let roots = writer_roots
            .into_iter()
            .filter(|root| *root > 0)
            .collect::<BTreeSet<_>>();
        #[cfg(not(target_os = "linux"))]
        {
            let _ = supervisor;
            if !roots.is_empty() {
                return Err("positive writer quiescence requires Linux procfs".into());
            }
            Ok(Self {
                stopped: Vec::new(),
                writer_count: 0,
                resumed: false,
            })
        }
        #[cfg(target_os = "linux")]
        {
            let (stopped, writer_count) = crate::writer_process::quiesce(&roots, supervisor.root)?;
            Ok(Self {
                stopped,
                writer_count,
                resumed: false,
            })
        }
    }

    pub(crate) fn writer_count(&self) -> u32 {
        self.writer_count
    }

    pub(crate) fn resume(&mut self) -> Result<(), String> {
        if !self.resumed {
            crate::writer_process::resume(&self.stopped)?;
            self.resumed = true;
        }
        Ok(())
    }
}

impl Drop for WriterFence {
    fn drop(&mut self) {
        if !self.resumed {
            crate::writer_process::resume_best_effort(&self.stopped);
        }
    }
}

#[cfg(target_os = "linux")]
fn reap_adopted_children() {
    loop {
        let Some(supervisor) = CHILD_SUPERVISOR.get() else {
            return;
        };
        {
            let _gate = supervisor
                .spawn_gate
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let managed = supervisor
                .managed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Ok(table) = crate::writer_process::process_table() {
                for (pid, record) in table {
                    if record.parent != supervisor.root
                        || !matches!(record.state, 'Z' | 'X')
                        || managed.contains(&pid)
                    {
                        continue;
                    }
                    let mut status = 0;
                    let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                    if result == pid {
                        tracing::debug!(pid, "reaped an adopted agent descendant");
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
#[path = "writer_fence_tests.rs"]
mod tests;
