//! Kernel-enforced quiescence for repository workspace capture.

use std::collections::BTreeSet;

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock, RwLock, RwLockReadGuard,
};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
const QUIESCE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const POLL_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(target_os = "linux")]
const ADOPTED_REAP_INTERVAL: Duration = Duration::from_millis(100);

#[cfg(target_os = "linux")]
struct ChildSupervisor {
    root: i32,
    /// A write lock closes the spawn-before-registration window while the
    /// adopted-child reaper decides which direct children it owns.
    spawn_gate: RwLock<()>,
    managed: Mutex<BTreeSet<i32>>,
    reaper_started: AtomicBool,
}

#[cfg(target_os = "linux")]
static CHILD_SUPERVISOR: OnceLock<ChildSupervisor> = OnceLock::new();

/// A process boundary that remains the parent of every orphaned agent
/// descendant. Linux reparents a double-forked process to the nearest living
/// child subreaper, so the runtime can enumerate writers after they leave the
/// original process session instead of trusting an image convention.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SupervisorBoundary {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    root: Option<i32>,
}

impl SupervisorBoundary {
    /// Establish the boundary before the runtime starts any agent process.
    pub(crate) fn install() -> Result<Self, String> {
        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self::default())
        }

        #[cfg(target_os = "linux")]
        {
            let root = i32::try_from(std::process::id())
                .map_err(|_| "runtime supervisor process ID exceeds Linux pid_t".to_string())?;
            if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
                return Err(format!(
                    "establish workspace-writer child subreaper: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut enabled: libc::c_int = 0;
            if unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut enabled) } != 0 {
                return Err(format!(
                    "verify workspace-writer child subreaper: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if enabled != 1 {
                return Err("workspace-writer child subreaper was not retained".into());
            }
            let supervisor = CHILD_SUPERVISOR.get_or_init(|| ChildSupervisor {
                root,
                spawn_gate: RwLock::new(()),
                managed: Mutex::new(BTreeSet::new()),
                reaper_started: AtomicBool::new(false),
            });
            if supervisor.root != root {
                return Err("workspace-writer child subreaper belongs to another process".into());
            }
            if supervisor
                .reaper_started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if let Err(error) = std::thread::Builder::new()
                    .name("cma-adopted-child-reaper".into())
                    .spawn(reap_adopted_children)
                {
                    supervisor.reaper_started.store(false, Ordering::Release);
                    return Err(format!("start adopted workspace-writer reaper: {error}"));
                }
            }
            Ok(Self { root: Some(root) })
        }
    }
}

/// Holds the reaper's spawn gate until a normal child handle has registered
/// its PID. The reaper may collect only adopted descendants, never a process
/// still owned by Tokio or portable-pty.
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

/// Registration held until the child's normal owner has positively waited it.
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

#[cfg(target_os = "linux")]
fn reap_adopted_children() {
    loop {
        let Some(supervisor) = CHILD_SUPERVISOR.get() else {
            return;
        };
        {
            let _spawn = supervisor
                .spawn_gate
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let managed = supervisor
                .managed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Ok(table) = process_table() {
                for (pid, record) in table {
                    if record.parent != supervisor.root
                        || !matches!(record.state, 'Z' | 'X')
                        || managed.contains(&pid)
                    {
                        continue;
                    }
                    let mut status = 0;
                    let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                    if reaped == pid {
                        tracing::debug!(pid, "reaped an adopted agent descendant");
                    } else if reaped == -1 {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::ECHILD) {
                            tracing::warn!(pid, %error, "could not reap an adopted agent descendant");
                        }
                    }
                }
            }
        }
        std::thread::sleep(ADOPTED_REAP_INTERVAL);
    }
}

/// Processes stopped by one checkpoint request. Drop is a fail-safe;
/// successful callers resume explicitly and receive a positive result.
pub(crate) struct WriterFence {
    stopped: Vec<i32>,
    writer_count: u32,
    resumed: bool,
}

impl WriterFence {
    /// Stop every process still belonging to, or descended from, a supervised
    /// writer session. Tracking the whole session matters because an
    /// interactive shell may put each foreground/background job in a distinct
    /// process group.
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
            let _ = (roots, supervisor);
            Err("positive workspace-writer quiescence requires Linux procfs support".into())
        }

        #[cfg(target_os = "linux")]
        {
            let (stopped, writer_count) = quiesce_linux(&roots, supervisor.root)?;
            Ok(Self {
                stopped,
                writer_count,
                resumed: false,
            })
        }
    }

    pub(crate) fn writer_count(&self) -> Result<u32, String> {
        Ok(self.writer_count)
    }

    pub(crate) fn resume(&mut self) -> Result<(), String> {
        if self.resumed {
            return Ok(());
        }
        resume_members(&self.stopped)?;
        self.resumed = true;
        Ok(())
    }
}

impl Drop for WriterFence {
    fn drop(&mut self) {
        if !self.resumed {
            resume_members_best_effort(&self.stopped);
        }
    }
}

/// Whether a retained process-session root still names a live workspace
/// writer. Callers retire authority only after this positive observation.
pub(crate) fn session_has_live_members(root: i32) -> Result<bool, String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Err("process-session retirement requires Linux procfs support".into())
    }

    #[cfg(target_os = "linux")]
    {
        let roots = BTreeSet::from([root]);
        Ok(writer_states(&process_table()?, &roots, None)
            .values()
            .any(|state| !matches!(state, 'Z' | 'X')))
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct ProcessRecord {
    parent: i32,
    session: i32,
    state: char,
}

#[cfg(target_os = "linux")]
fn quiesce_linux(
    roots: &BTreeSet<i32>,
    supervisor_root: Option<i32>,
) -> Result<(Vec<i32>, u32), String> {
    let deadline = Instant::now() + QUIESCE_TIMEOUT;
    let mut stopped_by_us = BTreeSet::new();
    let mut first_quiescent_observation = false;

    loop {
        let table = match process_table() {
            Ok(table) => table,
            Err(error) => {
                resume_members_best_effort(&stopped_by_us.iter().copied().collect::<Vec<_>>());
                return Err(error);
            }
        };
        let states = writer_states(&table, roots, supervisor_root);
        let running = states
            .iter()
            .filter_map(|(pid, state)| (!is_quiescent(*state)).then_some(*pid))
            .collect::<Vec<_>>();

        if running.is_empty() {
            // A second complete procfs observation closes the fork-between-
            // scan-and-signal window. Once every member is stopped, none can
            // create another writer before the archive is sealed.
            if first_quiescent_observation {
                let writer_count = u32::try_from(
                    states
                        .values()
                        .filter(|state| !matches!(state, 'Z' | 'X'))
                        .count(),
                )
                .map_err(|_| "workspace writer count exceeds its wire bound".to_string())?;
                return Ok((stopped_by_us.into_iter().collect(), writer_count));
            }
            first_quiescent_observation = true;
        } else {
            first_quiescent_observation = false;
            for pid in running {
                match signal_process(pid, libc::SIGSTOP, "stop") {
                    Ok(true) => {
                        stopped_by_us.insert(pid);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        resume_members_best_effort(
                            &stopped_by_us.iter().copied().collect::<Vec<_>>(),
                        );
                        return Err(error);
                    }
                }
            }
        }

        if Instant::now() >= deadline {
            resume_members_best_effort(&stopped_by_us.iter().copied().collect::<Vec<_>>());
            return Err("workspace writers did not positively quiesce before the deadline".into());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(target_os = "linux")]
fn writer_states(
    table: &BTreeMap<i32, ProcessRecord>,
    roots: &BTreeSet<i32>,
    supervisor_root: Option<i32>,
) -> BTreeMap<i32, char> {
    let mut selected = table
        .iter()
        .filter_map(|(pid, record)| {
            (roots.contains(pid) || roots.contains(&record.session)).then_some(*pid)
        })
        .collect::<BTreeSet<_>>();

    // The installed child-subreaper makes every fully daemonized agent process
    // a descendant of this root again. Include that entire boundary, while
    // never signalling the Layer 4 supervisor itself.
    if let Some(supervisor_root) = supervisor_root {
        selected.insert(supervisor_root);
    }

    // Retain descendants that created another session after the agent started,
    // including processes reparented to the installed supervisor boundary.
    loop {
        let before = selected.len();
        for (pid, record) in table {
            if selected.contains(&record.parent) {
                selected.insert(*pid);
            }
        }
        if selected.len() == before {
            break;
        }
    }

    if let Some(supervisor_root) = supervisor_root {
        selected.remove(&supervisor_root);
    }

    selected
        .into_iter()
        .filter_map(|pid| table.get(&pid).map(|record| (pid, record.state)))
        .collect()
}

#[cfg(target_os = "linux")]
fn process_table() -> Result<BTreeMap<i32, ProcessRecord>, String> {
    let mut table = BTreeMap::new();
    for entry in std::fs::read_dir("/proc")
        .map_err(|error| format!("inspect process table for workspace fence: {error}"))?
    {
        let entry = entry.map_err(|error| format!("inspect process table entry: {error}"))?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "inspect process {pid} for workspace fence: {error}"
                ))
            }
        };
        let after_name = stat
            .rsplit_once(')')
            .map(|(_, rest)| rest.trim())
            .ok_or_else(|| format!("process {pid} has a malformed procfs stat record"))?;
        let mut fields = after_name.split_whitespace();
        let state = fields
            .next()
            .and_then(|field| field.chars().next())
            .ok_or_else(|| format!("process {pid} has no procfs state"))?;
        let parent = fields
            .next()
            .and_then(|field| field.parse::<i32>().ok())
            .ok_or_else(|| format!("process {pid} has no procfs parent"))?;
        let _group = fields
            .next()
            .and_then(|field| field.parse::<i32>().ok())
            .ok_or_else(|| format!("process {pid} has no procfs process group"))?;
        let session = fields
            .next()
            .and_then(|field| field.parse::<i32>().ok())
            .ok_or_else(|| format!("process {pid} has no procfs session"))?;
        table.insert(
            pid,
            ProcessRecord {
                parent,
                session,
                state,
            },
        );
    }
    Ok(table)
}

#[cfg(target_os = "linux")]
fn is_quiescent(state: char) -> bool {
    matches!(state, 'T' | 't' | 'Z' | 'X')
}

fn signal_process(pid: i32, signal: i32, operation: &str) -> Result<bool, String> {
    let result = unsafe { libc::kill(pid, signal) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        // The process ended before the fence; an absent writer is quiescent.
        return Ok(false);
    }
    Err(format!(
        "{operation} workspace writer process {pid}: {error}"
    ))
}

#[cfg(target_os = "linux")]
fn resume_members(members: &[i32]) -> Result<(), String> {
    for pid in members {
        signal_process(*pid, libc::SIGCONT, "resume")?;
    }
    let deadline = Instant::now() + QUIESCE_TIMEOUT;
    loop {
        let table = process_table()?;
        if members.iter().all(|pid| {
            table
                .get(pid)
                .is_none_or(|record| !matches!(record.state, 'T' | 't'))
        }) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("workspace writers did not positively resume before the deadline".into());
        }
        for pid in members {
            signal_process(*pid, libc::SIGCONT, "resume")?;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(not(target_os = "linux"))]
fn resume_members(_members: &[i32]) -> Result<(), String> {
    Ok(())
}

fn resume_members_best_effort(members: &[i32]) {
    for pid in members {
        let _ = signal_process(*pid, libc::SIGCONT, "resume");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Child, Command, Stdio};

    use super::*;

    const DAEMON_HELPER_ENV: &str = "CMA_WRITER_FENCE_DAEMON_HELPER";

    struct DaemonProcess(i32);

    impl Drop for DaemonProcess {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
            }
        }
    }

    struct WriterSession {
        child: Child,
        root: i32,
    }

    impl WriterSession {
        fn background_writer() -> Self {
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", "set -m; sleep 60 & wait"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn().expect("writer process starts");
            let root = i32::try_from(child.id()).expect("process ID fits a session ID");
            Self { child, root }
        }

        fn active_workspace_writers(workspace: &std::path::Path) -> Self {
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    "set -m; while :; do printf a >> \"$CMA_FENCE_TEST_ROOT/turn\"; done & while :; do printf b >> \"$CMA_FENCE_TEST_ROOT/background\"; done & wait",
                ])
                .env("CMA_FENCE_TEST_ROOT", workspace)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn().expect("workspace writers start");
            let root = i32::try_from(child.id()).expect("process ID fits a session ID");
            Self { child, root }
        }
    }

    impl Drop for WriterSession {
        fn drop(&mut self) {
            let roots = BTreeSet::from([self.root]);
            let deadline = Instant::now() + QUIESCE_TIMEOUT;
            loop {
                let Ok(table) = process_table() else {
                    break;
                };
                let states = writer_states(&table, &roots, None);
                let live = states
                    .iter()
                    .filter_map(|(pid, state)| (!matches!(state, 'Z' | 'X')).then_some(*pid))
                    .collect::<Vec<_>>();
                if live.is_empty() {
                    break;
                }
                for pid in live {
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            let _ = self.child.wait();
        }
    }

    #[test]
    fn every_foreground_and_background_session_member_is_stopped_and_resumed() {
        let mut first = WriterSession::background_writer();
        let mut second = WriterSession::background_writer();
        let roots = [first.root, second.root];
        let root_set = roots.into_iter().collect::<BTreeSet<_>>();
        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        loop {
            let states = writer_states(&process_table().unwrap(), &root_set, None);
            if states.len() >= 4 {
                break;
            }
            assert!(Instant::now() < deadline, "background descendants started");
            std::thread::sleep(POLL_INTERVAL);
        }

        let mut fence = WriterFence::quiesce(roots, SupervisorBoundary::default())
            .expect("all writers quiesce");
        assert!(fence.writer_count().unwrap() >= 4);
        let stopped = writer_states(&process_table().unwrap(), &root_set, None);
        assert!(stopped.len() >= 4);
        assert!(stopped.values().all(|state| is_quiescent(*state)));

        fence.resume().expect("all writers resume");
        let resumed = writer_states(&process_table().unwrap(), &root_set, None);
        assert!(resumed.values().all(|state| !matches!(state, 'T' | 't')));

        // Keep the children mutable and live through all assertions; Drop
        // kills each whole session even if this test fails above.
        let _ = (&mut first, &mut second);
    }

    #[test]
    fn active_turn_and_background_writes_are_stable_inside_the_fence() {
        let workspace = std::env::temp_dir().join(format!(
            "cma-writer-fence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir(&workspace).unwrap();
        let mut session = WriterSession::active_workspace_writers(&workspace);
        let turn = workspace.join("turn");
        let background = workspace.join("background");
        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        loop {
            let turn_bytes = std::fs::metadata(&turn)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let background_bytes = std::fs::metadata(&background)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if turn_bytes > 128 && background_bytes > 128 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "both workspace writers became active"
            );
            std::thread::sleep(POLL_INTERVAL);
        }

        let mut fence = WriterFence::quiesce([session.root], SupervisorBoundary::default())
            .expect("all writers quiesce");
        assert!(fence.writer_count().unwrap() >= 3);
        let frozen = (
            std::fs::metadata(&turn).unwrap().len(),
            std::fs::metadata(&background).unwrap().len(),
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            frozen,
            (
                std::fs::metadata(&turn).unwrap().len(),
                std::fs::metadata(&background).unwrap().len(),
            )
        );

        fence.resume().expect("all writers resume");
        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        loop {
            let resumed = std::fs::metadata(&turn).unwrap().len() > frozen.0
                && std::fs::metadata(&background).unwrap().len() > frozen.1;
            if resumed {
                break;
            }
            assert!(Instant::now() < deadline, "both workspace writers resumed");
            std::thread::sleep(POLL_INTERVAL);
        }

        let _ = &mut session;
        drop(session);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_fully_daemonized_writer_is_stopped_by_the_supervisor_boundary() {
        // Run the process-level proof in its own test process. Installing a
        // child subreaper is process-wide, and a parallel libtest sibling must
        // never become part of this test's writer boundary.
        if std::env::var_os(DAEMON_HELPER_ENV).is_none() {
            let test_name = std::thread::current()
                .name()
                .expect("libtest names the current test")
                .to_string();
            let output = Command::new(std::env::current_exe().expect("test executable exists"))
                .args(["--exact", &test_name, "--nocapture"])
                .env(DAEMON_HELPER_ENV, "1")
                .output()
                .expect("isolated daemon-fence test starts");
            assert!(
                output.status.success(),
                "isolated daemon-fence proof failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let supervisor = SupervisorBoundary::install().expect("child subreaper installs");
        let supervisor_root = supervisor.root.expect("Linux boundary has a root");
        let workspace = std::env::temp_dir().join(format!(
            "cma-daemon-writer-fence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir(&workspace).expect("daemon workspace is created");
        let written = workspace.join("daemon");
        let written_c = CString::new(written.as_os_str().as_bytes()).expect("path has no NUL");
        let mut pipe = [-1_i32; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );

        let spawn = ManagedSpawnPermit::begin();
        let session_root = unsafe { libc::fork() };
        if session_root == 0 {
            unsafe {
                libc::close(pipe[0]);
                if libc::setsid() == -1 {
                    libc::_exit(101);
                }
                let daemon = libc::fork();
                if daemon < 0 {
                    libc::_exit(102);
                }
                if daemon > 0 {
                    libc::_exit(0);
                }
                // The second session change is what escapes both the original
                // session ID and ancestry once the intermediate exits.
                if libc::setsid() == -1 {
                    libc::_exit(103);
                }
                let daemon_pid = libc::getpid();
                let pid_bytes = daemon_pid.to_ne_bytes();
                if libc::write(pipe[1], pid_bytes.as_ptr().cast(), pid_bytes.len())
                    != pid_bytes.len() as isize
                {
                    libc::_exit(104);
                }
                libc::close(pipe[1]);
                let file = libc::open(
                    written_c.as_ptr(),
                    libc::O_CREAT | libc::O_WRONLY | libc::O_APPEND,
                    0o600,
                );
                if file == -1 {
                    libc::_exit(105);
                }
                let byte = [b'x'];
                loop {
                    if libc::write(file, byte.as_ptr().cast(), byte.len()) != 1 {
                        libc::_exit(106);
                    }
                    libc::usleep(1_000);
                }
            }
        }
        assert!(session_root > 0, "first fork succeeds");
        let managed_session = spawn.register(u32::try_from(session_root).ok());

        unsafe {
            libc::close(pipe[1]);
        }
        let mut daemon_bytes = [0_u8; std::mem::size_of::<i32>()];
        let mut received = 0;
        while received < daemon_bytes.len() {
            let read = unsafe {
                libc::read(
                    pipe[0],
                    daemon_bytes[received..].as_mut_ptr().cast(),
                    daemon_bytes.len() - received,
                )
            };
            assert!(read > 0, "daemon reports its process ID");
            received += usize::try_from(read).expect("read length is positive");
        }
        unsafe {
            libc::close(pipe[0]);
        }
        let daemon_pid = i32::from_ne_bytes(daemon_bytes);
        let _daemon = DaemonProcess(daemon_pid);
        let mut first_status = 0;
        assert_eq!(
            unsafe { libc::waitpid(session_root, &mut first_status, 0) },
            session_root
        );
        assert!(libc::WIFEXITED(first_status));
        assert_eq!(libc::WEXITSTATUS(first_status), 0);
        drop(managed_session);

        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        loop {
            let table = process_table().expect("process table is readable");
            let adopted = table.get(&daemon_pid).is_some_and(|record| {
                record.parent == supervisor_root && record.session == daemon_pid
            });
            let bytes = std::fs::metadata(&written)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if adopted && bytes > 128 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "double-forked writer is adopted and active"
            );
            std::thread::sleep(POLL_INTERVAL);
        }

        let mut fence = WriterFence::quiesce([session_root], supervisor)
            .expect("adopted daemon writer positively quiesces");
        assert!(fence.writer_count().unwrap() >= 1);
        let selected = writer_states(
            &process_table().unwrap(),
            &BTreeSet::from([session_root]),
            Some(supervisor_root),
        );
        assert!(selected
            .get(&daemon_pid)
            .is_some_and(|state| is_quiescent(*state)));
        let frozen = std::fs::metadata(&written).unwrap().len();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(frozen, std::fs::metadata(&written).unwrap().len());

        fence.resume().expect("adopted daemon writer resumes");
        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        while std::fs::metadata(&written).unwrap().len() == frozen {
            assert!(Instant::now() < deadline, "adopted daemon writer resumed");
            std::thread::sleep(POLL_INTERVAL);
        }

        drop(_daemon);
        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        let daemon_proc = std::path::PathBuf::from(format!("/proc/{daemon_pid}"));
        while daemon_proc.exists() {
            assert!(
                Instant::now() < deadline,
                "adopted daemon is reaped after exit"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(daemon_pid, &mut status, libc::WNOHANG) },
            -1,
            "the adopted-child reaper consumed the zombie"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }
}
