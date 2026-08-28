//! Kernel-enforced quiescence for repository workspace capture.

use std::collections::BTreeSet;

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
const QUIESCE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    pub(crate) fn quiesce(writer_roots: impl IntoIterator<Item = i32>) -> Result<Self, String> {
        let roots = writer_roots
            .into_iter()
            .filter(|root| *root > 0)
            .collect::<BTreeSet<_>>();

        #[cfg(not(target_os = "linux"))]
        {
            let _ = roots;
            Err("positive workspace-writer quiescence requires Linux procfs support".into())
        }

        #[cfg(target_os = "linux")]
        {
            let (stopped, writer_count) = quiesce_linux(&roots)?;
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
        Ok(writer_states(&process_table()?, &roots)
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
fn quiesce_linux(roots: &BTreeSet<i32>) -> Result<(Vec<i32>, u32), String> {
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
        let states = writer_states(&table, roots);
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
) -> BTreeMap<i32, char> {
    let mut selected = table
        .iter()
        .filter_map(|(pid, record)| {
            (roots.contains(pid) || roots.contains(&record.session)).then_some(*pid)
        })
        .collect::<BTreeSet<_>>();

    // Also retain descendants that created another session after the agent
    // started. A fully daemonized/reparented process is outside the supported
    // writer model and must not be created by a Trigger runtime image.
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
    use std::os::unix::process::CommandExt as _;
    use std::process::{Child, Command, Stdio};

    use super::*;

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
            if let Ok(table) = process_table() {
                for pid in writer_states(&table, &BTreeSet::from([self.root])).keys() {
                    unsafe {
                        libc::kill(*pid, libc::SIGKILL);
                    }
                }
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
            let states = writer_states(&process_table().unwrap(), &root_set);
            if states.len() >= 4 {
                break;
            }
            assert!(Instant::now() < deadline, "background descendants started");
            std::thread::sleep(POLL_INTERVAL);
        }

        let mut fence = WriterFence::quiesce(roots).expect("all writers quiesce");
        assert!(fence.writer_count().unwrap() >= 4);
        let stopped = writer_states(&process_table().unwrap(), &root_set);
        assert!(stopped.len() >= 4);
        assert!(stopped.values().all(|state| is_quiescent(*state)));

        fence.resume().expect("all writers resume");
        let resumed = writer_states(&process_table().unwrap(), &root_set);
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

        let mut fence = WriterFence::quiesce([session.root]).expect("all writers quiesce");
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
}
