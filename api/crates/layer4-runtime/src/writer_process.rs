#[cfg(any(target_os = "linux", test))]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
const TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const POLL: Duration = Duration::from_millis(10);

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy)]
pub(super) struct ProcessRecord {
    pub parent: i32,
    pub session: i32,
    pub state: char,
}

#[cfg(target_os = "linux")]
pub(super) fn quiesce(
    roots: &BTreeSet<i32>,
    supervisor: Option<i32>,
) -> Result<(Vec<i32>, u32), String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut stopped = BTreeSet::new();
    let mut observed_quiet = false;
    loop {
        let table = process_table().inspect_err(|_| resume_best_effort_set(&stopped))?;
        let states = writer_states(&table, roots, supervisor);
        let running = states
            .iter()
            .filter_map(|(pid, state)| (!quiet(*state)).then_some(*pid))
            .collect::<Vec<_>>();
        if running.is_empty() {
            if observed_quiet {
                let count = states
                    .values()
                    .filter(|state| !matches!(state, 'Z' | 'X'))
                    .count();
                return u32::try_from(count)
                    .map(|count| (stopped.into_iter().collect(), count))
                    .map_err(|_| "writer count exceeds its wire bound".into());
            }
            observed_quiet = true;
        } else {
            observed_quiet = false;
            for pid in running {
                match signal(pid, libc::SIGSTOP, "stop") {
                    Ok(true) => {
                        stopped.insert(pid);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        resume_best_effort_set(&stopped);
                        return Err(error);
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            resume_best_effort_set(&stopped);
            return Err("writers did not positively quiesce before the deadline".into());
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(any(target_os = "linux", test))]
fn writer_states(
    table: &BTreeMap<i32, ProcessRecord>,
    roots: &BTreeSet<i32>,
    supervisor: Option<i32>,
) -> BTreeMap<i32, char> {
    let mut selected = table
        .iter()
        .filter_map(|(pid, record)| {
            (roots.contains(pid) || roots.contains(&record.session)).then_some(*pid)
        })
        .collect::<BTreeSet<_>>();
    if let Some(root) = supervisor {
        selected.insert(root);
    }
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
    if let Some(root) = supervisor {
        selected.remove(&root);
    }
    selected
        .into_iter()
        .filter_map(|pid| table.get(&pid).map(|record| (pid, record.state)))
        .collect()
}

#[cfg(target_os = "linux")]
pub(super) fn process_table() -> Result<BTreeMap<i32, ProcessRecord>, String> {
    process_table_from(std::path::Path::new("/proc"), |path| {
        std::fs::read_to_string(path)
    })
}

#[cfg(any(target_os = "linux", test))]
fn process_table_from(
    proc_root: &std::path::Path,
    mut read_stat: impl FnMut(&std::path::Path) -> std::io::Result<String>,
) -> Result<BTreeMap<i32, ProcessRecord>, String> {
    let mut table = BTreeMap::new();
    for entry in std::fs::read_dir(proc_root).map_err(|error| format!("inspect /proc: {error}"))? {
        let entry = entry.map_err(|error| format!("inspect /proc entry: {error}"))?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let stat = match read_stat(&entry.path().join("stat")) {
            Ok(stat) => stat,
            // A task can exit after enumeration. procfs reports either ENOENT
            // or ESRCH; neither leaves a writer to stop. Other failures must
            // still refuse the checkpoint instead of hiding an unknown writer.
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(error) => return Err(format!("inspect process {pid}: {error}")),
        };
        let fields = stat
            .rsplit_once(')')
            .map(|(_, fields)| fields.trim())
            .ok_or_else(|| format!("process {pid} has malformed stat"))?;
        let mut fields = fields.split_whitespace();
        let state = fields
            .next()
            .and_then(|field| field.chars().next())
            .ok_or_else(|| format!("process {pid} has no state"))?;
        let parent = number(&mut fields, pid, "parent")?;
        let _group = number(&mut fields, pid, "group")?;
        let session = number(&mut fields, pid, "session")?;
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

#[cfg(any(target_os = "linux", test))]
fn number<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    pid: i32,
    name: &str,
) -> Result<i32, String> {
    fields
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| format!("process {pid} has no {name}"))
}

#[cfg(target_os = "linux")]
fn quiet(state: char) -> bool {
    matches!(state, 'T' | 't' | 'Z' | 'X')
}

#[cfg(target_os = "linux")]
fn signal(pid: i32, signal: i32, operation: &str) -> Result<bool, String> {
    if unsafe { libc::kill(pid, signal) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(format!("{operation} writer process {pid}: {error}"))
}

#[cfg(target_os = "linux")]
pub(super) fn resume(members: &[i32]) -> Result<(), String> {
    for pid in members {
        signal(*pid, libc::SIGCONT, "resume")?;
    }
    let deadline = Instant::now() + TIMEOUT;
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
            return Err("writers did not positively resume before the deadline".into());
        }
        for pid in members {
            signal(*pid, libc::SIGCONT, "resume")?;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn resume(_members: &[i32]) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn resume_best_effort(members: &[i32]) {
    for pid in members {
        let _ = signal(*pid, libc::SIGCONT, "resume");
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn resume_best_effort(_members: &[i32]) {}

#[cfg(target_os = "linux")]
fn resume_best_effort_set(members: &BTreeSet<i32>) {
    resume_best_effort(&members.iter().copied().collect::<Vec<_>>());
}

#[cfg(test)]
#[path = "writer_process_tests.rs"]
mod tests;
