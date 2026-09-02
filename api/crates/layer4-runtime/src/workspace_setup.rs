//! Bounded administrator-owned setup for one prepared repository workspace.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;

pub(crate) const DEFAULT_COMMAND_DEADLINE: Duration = Duration::from_secs(600);
pub(crate) const MAX_OUTPUT_BYTES: usize = 32 * 1024;
// Six-byte JSON escapes are valid inside every settings-bounded command.
pub(crate) const MAX_REQUEST_BYTES: usize = 512 * 1024;
const MAX_COMMANDS: usize = 16;
const MAX_COMMAND_BYTES: usize = 4 * 1024;
const OUTPUT_DRAIN_DEADLINE: Duration = Duration::from_secs(1);
const TIMEOUT_EXIT_CODE: i32 = 124;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceSetupRequest {
    pub(crate) commands: Vec<String>,
}

impl WorkspaceSetupRequest {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.commands.len() > MAX_COMMANDS {
            return Err("workspace setup accepts at most 16 commands");
        }
        if self
            .commands
            .iter()
            .any(|command| command.len() > MAX_COMMAND_BYTES)
        {
            return Err("workspace setup commands must be at most 4096 bytes");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WorkspaceSetupStep {
    pub(crate) command_index: usize,
    pub(crate) exit_code: i32,
    pub(crate) output_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WorkspaceSetupReceipt {
    pub(crate) steps: Vec<WorkspaceSetupStep>,
    pub(crate) failing_index: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct WorkspaceSetupError(String);

impl std::fmt::Display for WorkspaceSetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WorkspaceSetupError {}

pub(crate) async fn run(
    repository: &Path,
    commands: &[String],
) -> Result<WorkspaceSetupReceipt, WorkspaceSetupError> {
    run_with_deadline(repository, commands, DEFAULT_COMMAND_DEADLINE).await
}

pub(crate) async fn run_with_deadline(
    repository: &Path,
    commands: &[String],
    deadline: Duration,
) -> Result<WorkspaceSetupReceipt, WorkspaceSetupError> {
    let mut steps = Vec::with_capacity(commands.len());
    for (command_index, command) in commands.iter().enumerate() {
        let step = run_command(repository, command_index, command, deadline).await?;
        let failed = step.exit_code != 0;
        steps.push(step);
        if failed {
            return Ok(WorkspaceSetupReceipt {
                steps,
                failing_index: Some(command_index),
            });
        }
    }
    Ok(WorkspaceSetupReceipt {
        steps,
        failing_index: None,
    })
}

async fn run_command(
    repository: &Path,
    command_index: usize,
    command: &str,
    deadline: Duration,
) -> Result<WorkspaceSetupStep, WorkspaceSetupError> {
    let mut process = tokio::process::Command::new("/bin/bash");
    process
        .arg("-c")
        .arg(command)
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        process.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let spawn = crate::writer_fence::ManagedSpawnPermit::begin();
    let mut child = process
        .spawn()
        .map_err(|error| WorkspaceSetupError(format!("start workspace setup command: {error}")))?;
    let managed_child = spawn.register(child.id());
    let process_session = child.id().and_then(|pid| i32::try_from(pid).ok());
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WorkspaceSetupError("workspace setup stdout was unavailable".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| WorkspaceSetupError("workspace setup stderr was unavailable".to_string()))?;
    let output = Arc::new(Mutex::new(TailBuffer::default()));
    let stdout_reader = tokio::spawn(read_output(stdout, Arc::clone(&output)));
    let stderr_reader = tokio::spawn(read_output(stderr, Arc::clone(&output)));

    let exit_code = match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) => exit_code(status),
        Ok(Err(error)) => {
            terminate_process_session(process_session);
            return Err(WorkspaceSetupError(format!(
                "wait for workspace setup command: {error}"
            )));
        }
        Err(_) => {
            terminate_process_session(process_session);
            let _ = child.kill().await;
            let _ = child.wait().await;
            TIMEOUT_EXIT_CODE
        }
    };
    // A setup command is a finite operation. Do not allow a background child
    // to retain workspace or output-pipe authority after its shell exits.
    terminate_process_session(process_session);
    finish_reader(stdout_reader).await?;
    finish_reader(stderr_reader).await?;
    drop(managed_child);

    let bytes = output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .bytes
        .clone();
    Ok(WorkspaceSetupStep {
        command_index,
        exit_code,
        output_excerpt: output_excerpt(&bytes),
    })
}

async fn read_output<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    output: Arc<Mutex<TailBuffer>>,
) -> std::io::Result<()> {
    let mut chunk = [0_u8; 4 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append(&chunk[..read]);
    }
}

async fn finish_reader(
    mut reader: tokio::task::JoinHandle<std::io::Result<()>>,
) -> Result<(), WorkspaceSetupError> {
    match tokio::time::timeout(OUTPUT_DRAIN_DEADLINE, &mut reader).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(WorkspaceSetupError(format!(
            "read workspace setup output: {error}"
        ))),
        Ok(Err(error)) => Err(WorkspaceSetupError(format!(
            "join workspace setup output reader: {error}"
        ))),
        Err(_) => {
            reader.abort();
            let _ = reader.await;
            Ok(())
        }
    }
}

#[derive(Default)]
struct TailBuffer {
    bytes: Vec<u8>,
}

impl TailBuffer {
    fn append(&mut self, chunk: &[u8]) {
        if chunk.len() >= MAX_OUTPUT_BYTES {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&chunk[chunk.len() - MAX_OUTPUT_BYTES..]);
            return;
        }
        let overflow = self
            .bytes
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(MAX_OUTPUT_BYTES);
        if overflow != 0 {
            self.bytes.drain(..overflow);
        }
        self.bytes.extend_from_slice(chunk);
    }
}

fn output_excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if matches!(character, '\n' | '\t') || !character.is_control() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>();
    if text.len() <= MAX_OUTPUT_BYTES {
        return text;
    }
    let mut start = text.len() - MAX_OUTPUT_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal().map_or(1, |signal| 128 + signal)
    }
    #[cfg(not(unix))]
    1
}

#[cfg(unix)]
fn terminate_process_session(process_session: Option<i32>) {
    let Some(process_session) = process_session.filter(|pid| *pid > 0) else {
        return;
    };
    let killed = unsafe { libc::kill(-process_session, libc::SIGKILL) };
    if killed == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(%error, process_session, "workspace setup process cleanup failed");
        }
    }
}

#[cfg(not(unix))]
fn terminate_process_session(_process_session: Option<i32>) {}

#[cfg(test)]
#[path = "workspace_setup_tests.rs"]
mod tests;
