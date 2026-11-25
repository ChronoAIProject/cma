//! Each supervised desktop has its own Linux subreaper.
//!
//! Helpers that start another session still return to this process when
//! orphaned. The runtime can retain its managed group leader until the
//! guardian proves it has reaped every descendant.

use std::io::{self, Read as _, Write as _};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

pub(crate) const COMMAND: &str = "--desktop-generation-guardian";
pub(super) const STOPPED: u8 = 1;
pub(super) const REAPED: u8 = 2;
pub(super) const CLEANUP_LIMIT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub(crate) async fn run() -> Result<(), String> {
    supervise(Command::new(super::LAUNCHER), report).await
}

async fn supervise(
    mut launcher: Command,
    notify: impl Fn(u8) -> Result<(), String>,
) -> Result<(), String> {
    // Also enforce custody at the entry point so a direct invocation cannot
    // leave its descendants with the runtime's adopted-child reaper.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == -1 {
        return Err(format!(
            "configure desktop subreaper: {}",
            io::Error::last_os_error()
        ));
    }
    launcher
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true);
    let (closed, closing) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("desktop-guardian-control".into())
        .spawn(move || {
            // The runtime owns the write end. A byte or EOF both stop this
            // generation, including when the runtime itself has exited.
            if let Err(error) = io::stdin().read(&mut [0]) {
                tracing::warn!(%error, "desktop guardian control closed");
            }
            let _ = closed.send(());
        })
        .map_err(|error| format!("start desktop guardian control: {error}"))?;
    let mut launcher = match launcher.spawn() {
        Ok(launcher) => launcher,
        Err(error) => {
            tracing::warn!(%error, "desktop launcher could not start");
            notify(STOPPED)?;
            return notify(REAPED);
        }
    };
    let exited = tokio::select! {
        biased;
        _ = closing => false,
        status = launcher.wait() => {
            let status = status.map_err(|error| format!("wait for desktop launcher: {error}"))?;
            tracing::warn!(%status, "desktop launcher exited");
            true
        }
    };
    let stopped = notify(STOPPED);
    tokio::time::timeout(CLEANUP_LIMIT, async {
        if !exited {
            launcher
                .start_kill()
                .map_err(|error| format!("stop desktop launcher: {error}"))?;
            launcher
                .wait()
                .await
                .map_err(|error| format!("reap desktop launcher: {error}"))?;
        }
        reap_descendants().await
    })
    .await
    .map_err(|_| "desktop descendants exceeded the cleanup deadline".to_string())??;
    stopped?;
    notify(REAPED)
}

#[cfg(test)]
mod tests;

fn report(event: u8) -> Result<(), String> {
    let mut output = io::stdout().lock();
    output
        .write_all(&[event])
        .and_then(|()| output.flush())
        .map_err(|error| format!("write desktop guardian status: {error}"))
}

async fn reap_descendants() -> Result<(), String> {
    let guardian = i32::try_from(std::process::id()).map_err(|_| "invalid guardian PID")?;
    loop {
        for (pid, record) in crate::writer_process::process_table()? {
            if record.parent == guardian && !matches!(record.state, 'Z' | 'X') {
                // This process alone reaps its adopted children. A child
                // discovered here still owns its PID while we signal it.
                if unsafe { libc::kill(pid, libc::SIGKILL) } == -1 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(format!("stop desktop descendant: {error}"));
                    }
                }
            }
        }
        loop {
            match unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) } {
                pid if pid > 0 => continue,
                0 => break,
                _ => {
                    let error = io::Error::last_os_error();
                    match error.raw_os_error() {
                        Some(libc::ECHILD) => return Ok(()),
                        Some(libc::EINTR) => continue,
                        _ => return Err(format!("reap desktop descendant: {error}")),
                    }
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
