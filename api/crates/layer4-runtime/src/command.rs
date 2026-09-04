use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use layer3_adapter::CommandReceipt;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use tokio::io::AsyncReadExt as _;

use crate::failure::{Result, RuntimeError};
use crate::writer_fence::ManagedSpawnPermit;

pub(crate) const MAX_COMMANDS: usize = 64;
pub(crate) const MAX_COMMAND_BYTES: usize = 16 * 1024;
pub(crate) const MAX_OUTPUT_BYTES: u32 = 32 * 1024;
pub(crate) const MAX_COMMAND_SECONDS: u64 = 600;
pub(crate) const TOTAL_COMMAND_SECONDS: u64 = 1_800;
const OUTPUT_DRAIN_SECONDS: u64 = 2;

#[derive(Clone, Copy)]
pub(crate) struct CommandLimits {
    pub timeout: Duration,
    pub total_timeout: Duration,
    pub output_limit: usize,
}

impl CommandLimits {
    pub(crate) fn install(timeout_seconds: u64, output_limit: u32) -> Result<Self> {
        if timeout_seconds == 0 || timeout_seconds > MAX_COMMAND_SECONDS {
            return Err(RuntimeError::rejected(
                "invalid_timeout",
                format!("command timeout must be between 1 and {MAX_COMMAND_SECONDS} seconds"),
            ));
        }
        if output_limit == 0 || output_limit > MAX_OUTPUT_BYTES {
            return Err(RuntimeError::rejected(
                "invalid_output_limit",
                format!("output limit must be between 1 and {MAX_OUTPUT_BYTES} bytes"),
            ));
        }
        Ok(Self {
            timeout: Duration::from_secs(timeout_seconds),
            total_timeout: Duration::from_secs(TOTAL_COMMAND_SECONDS),
            output_limit: output_limit as usize,
        })
    }
}

#[derive(Debug)]
struct StreamDigest {
    bytes: usize,
    sha256: String,
}

pub(crate) async fn run_shell_commands(
    commands: &[String],
    cwd: &std::path::Path,
    env: &BTreeMap<String, String>,
    limits: CommandLimits,
) -> Result<Vec<CommandReceipt>> {
    validate_commands(commands)?;
    let started = tokio::time::Instant::now();
    let total_deadline = started + limits.total_timeout;
    let mut receipts = Vec::with_capacity(commands.len());

    for (index, command) in commands.iter().enumerate() {
        let now = tokio::time::Instant::now();
        if now >= total_deadline {
            return Err(RuntimeError::rejected(
                "command_total_timeout",
                format!("command {index} was not started before the 1800 second total deadline"),
            ));
        }
        let deadline = std::cmp::min(now + limits.timeout, total_deadline);
        receipts.push(run_one(index, command, cwd, env, limits.output_limit, deadline).await?);
    }
    Ok(receipts)
}

pub(crate) fn validate_commands(commands: &[String]) -> Result<()> {
    if commands.len() > MAX_COMMANDS {
        return Err(RuntimeError::rejected(
            "too_many_commands",
            format!("at most {MAX_COMMANDS} commands may run"),
        ));
    }
    for (index, command) in commands.iter().enumerate() {
        if command.is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
            return Err(RuntimeError::rejected(
                "invalid_command",
                format!("command {index} is empty, too large, or contains NUL"),
            ));
        }
    }
    Ok(())
}

async fn run_one(
    index: usize,
    script: &str,
    cwd: &std::path::Path,
    env: &BTreeMap<String, String>,
    output_limit: usize,
    deadline: tokio::time::Instant,
) -> Result<CommandReceipt> {
    let mut command = tokio::process::Command::new("/bin/bash");
    command
        .args(["-o", "pipefail", "-c", script])
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let spawn = ManagedSpawnPermit::begin();
    let mut child = command.spawn().map_err(|error| {
        RuntimeError::unavailable(
            "command_spawn_failed",
            format!("start command {index} with /bin/bash: {error}"),
        )
    })?;
    let managed = spawn.register(child.id());
    let session = child.id().and_then(|pid| i32::try_from(pid).ok());
    let stdout = child.stdout.take().ok_or_else(|| {
        RuntimeError::unavailable("command_io_failed", "command stdout was not piped")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        RuntimeError::unavailable("command_io_failed", "command stderr was not piped")
    })?;
    let (overflow_tx, mut overflow_rx) = tokio::sync::mpsc::channel(2);
    let overflow_guard = overflow_tx.clone();
    let stdout_task = tokio::spawn(digest_stream(
        stdout,
        output_limit,
        "stdout",
        overflow_tx.clone(),
    ));
    let stderr_task = tokio::spawn(digest_stream(stderr, output_limit, "stderr", overflow_tx));

    let mut timed_out = false;
    let mut overflowed = None;
    let status = tokio::select! {
        status = child.wait() => status.map_err(|error| RuntimeError::unavailable(
            "command_wait_failed", format!("wait for command {index}: {error}")))?,
        _ = tokio::time::sleep_until(deadline) => {
            timed_out = true;
            terminate_session(session, libc::SIGTERM);
            let _ = child.kill().await;
            child.wait().await.map_err(|error| RuntimeError::unavailable(
                "command_wait_failed", format!("reap timed-out command {index}: {error}")))?
        }
        stream = overflow_rx.recv() => {
            overflowed = stream;
            terminate_session(session, libc::SIGTERM);
            let _ = child.kill().await;
            child.wait().await.map_err(|error| RuntimeError::unavailable(
                "command_wait_failed", format!("reap oversized command {index}: {error}")))?
        }
    };
    drop(overflow_guard);
    drop(managed);

    let streams = tokio::time::timeout(Duration::from_secs(OUTPUT_DRAIN_SECONDS), async {
        let stdout = stdout_task.await;
        let stderr = stderr_task.await;
        (stdout, stderr)
    })
    .await;
    let (stdout, stderr) = match streams {
        Ok((Ok(Ok(stdout)), Ok(Ok(stderr)))) => (stdout, stderr),
        Ok(_) => {
            return Err(RuntimeError::unavailable(
                "command_io_failed",
                format!("read command {index} output"),
            ))
        }
        Err(_) => {
            terminate_session(session, libc::SIGKILL);
            return Err(RuntimeError::rejected(
                "command_left_processes",
                format!("command {index} left a process holding its output streams"),
            ));
        }
    };

    if timed_out {
        return Err(RuntimeError::rejected(
            "command_timeout",
            format!("command {index} exceeded its wall-clock deadline"),
        ));
    }
    let overflowed = overflowed.or_else(|| {
        (stdout.bytes > output_limit)
            .then_some("stdout")
            .or_else(|| (stderr.bytes > output_limit).then_some("stderr"))
    });
    if let Some(stream) = overflowed {
        return Err(RuntimeError::rejected(
            "command_output_limit",
            format!("command {index} exceeded its {stream} output limit"),
        ));
    }
    let exit_code = exit_code(status);
    if exit_code != 0 {
        return Err(RuntimeError::rejected(
            "command_failed",
            format!("command {index} exited with code {exit_code}"),
        ));
    }
    Ok(CommandReceipt {
        index: u32::try_from(index).unwrap_or(u32::MAX),
        exit_code,
        stdout_sha256: stdout.sha256,
        stderr_sha256: stderr.sha256,
    })
}

async fn digest_stream<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    name: &'static str,
    overflow: tokio::sync::mpsc::Sender<&'static str>,
) -> std::io::Result<StreamDigest> {
    let mut digest = Sha256::new();
    let mut bytes = 0_usize;
    let mut announced = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes = bytes.saturating_add(read);
        if bytes > limit && !announced {
            announced = true;
            let _ = overflow.send(name).await;
        }
    }
    Ok(StreamDigest {
        bytes,
        sha256: hex(&digest.finalize()),
    })
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[cfg(unix)]
fn terminate_session(session: Option<i32>, signal: i32) {
    if let Some(session) = session {
        unsafe {
            libc::kill(-session, signal);
        }
    }
}

#[cfg(not(unix))]
fn terminate_session(_session: Option<i32>, _signal: i32) {}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
