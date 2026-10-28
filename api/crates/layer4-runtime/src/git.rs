use std::ffi::OsString;
use std::process::Stdio;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use tokio::io::AsyncReadExt as _;

use crate::failure::{Result, RuntimeError};
use crate::writer_fence::ManagedSpawnPermit;

const GIT_TIMEOUT: Duration = Duration::from_secs(120);
// Clone also checks out the complete current tree. Large public repositories
// can finish transfer but still be writing files after the ordinary Git budget.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);
const OUTPUT_LIMIT: usize = 64 * 1024;

pub(crate) struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Copy)]
pub(crate) struct GitRunner {
    allow_local_sources: bool,
}

impl GitRunner {
    pub(crate) fn new(allow_local_sources: bool) -> Self {
        Self {
            allow_local_sources,
        }
    }

    pub(crate) async fn checked<I, S>(
        &self,
        cwd: &std::path::Path,
        args: I,
        operation: &'static str,
    ) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.checked_with_timeout(cwd, args, operation, GIT_TIMEOUT)
            .await
    }

    pub(crate) async fn checked_clone<I, S>(
        &self,
        cwd: &std::path::Path,
        args: I,
    ) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.checked_with_timeout(cwd, args, "clone workspace", CLONE_TIMEOUT)
            .await
    }

    async fn checked_with_timeout<I, S>(
        &self,
        cwd: &std::path::Path,
        args: I,
        operation: &'static str,
        timeout: Duration,
    ) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
        let mut command = tokio::process::Command::new("git");
        command
            .env_clear()
            .envs(std::env::vars().filter(|(name, _)| retain_inherited_environment(name.as_str())))
            .args(&args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "Never")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_ASKPASS", "/bin/false");
        if self.allow_local_sources {
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
                .env("GIT_CONFIG_VALUE_0", "always");
        } else {
            command.env("GIT_CONFIG_COUNT", "0");
        }
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
                "git_spawn_failed",
                format!("start git {operation}: {error}"),
            )
        })?;
        let managed = spawn.register(child.id());
        let session = child.id().and_then(|pid| i32::try_from(pid).ok());
        let stdout = child.stdout.take().ok_or_else(|| {
            RuntimeError::unavailable("git_io_failed", "git stdout was not piped")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            RuntimeError::unavailable("git_io_failed", "git stderr was not piped")
        })?;
        let stdout = tokio::spawn(read_bounded(stdout));
        let stderr = tokio::spawn(read_bounded(stderr));

        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => status.map_err(|error| {
                RuntimeError::unavailable(
                    "git_wait_failed",
                    format!("wait for git {operation}: {error}"),
                )
            })?,
            Err(_) => {
                terminate_session(session);
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(RuntimeError::unavailable(
                    "git_timeout",
                    format!("git {operation} exceeded {} seconds", timeout.as_secs()),
                ));
            }
        };
        drop(managed);
        let stdout = join_output(stdout, operation).await?;
        let stderr = join_output(stderr, operation).await?;
        let output = GitOutput {
            stdout: clean(&stdout),
            stderr: clean(&stderr),
        };
        if !status.success() {
            return Err(RuntimeError::rejected(
                "git_failed",
                if output.stderr.is_empty() {
                    format!("git {operation} failed")
                } else {
                    format!("git {operation} failed: {}", output.stderr)
                },
            ));
        }
        Ok(output)
    }
}

fn retain_inherited_environment(name: &str) -> bool {
    !name.starts_with("GIT_")
}

async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(retained);
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

async fn join_output(
    task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    operation: &'static str,
) -> Result<Vec<u8>> {
    task.await
        .map_err(|error| {
            RuntimeError::unavailable(
                "git_io_failed",
                format!("join git {operation} output: {error}"),
            )
        })?
        .map_err(|error| {
            RuntimeError::unavailable(
                "git_io_failed",
                format!("read git {operation} output: {error}"),
            )
        })
}

fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(unix)]
fn terminate_session(session: Option<i32>) {
    if let Some(session) = session {
        unsafe {
            libc::kill(-session, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn terminate_session(_session: Option<i32>) {}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
