//! Lifetime ownership of the runtime's shared desktop process group.
use tokio::process::{Child, Command};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::writer_fence::{ManagedChild, ManagedSpawnPermit};

pub(crate) struct DesktopProcess {
    child: Child,
    _managed: ManagedChild,
}

impl DesktopProcess {
    pub(crate) fn start(config: &RuntimeConfig) -> Result<Self> {
        let mut command = Command::new("/usr/local/bin/cma-desktop-session");
        command
            .env_clear()
            .envs(&config.install_env)
            .current_dir(&config.home);
        Self::spawn(command).map_err(|error| {
            RuntimeError::unavailable(
                "desktop_start_failed",
                format!("start desktop session: {error}"),
            )
        })
    }

    fn spawn(mut command: Command) -> std::io::Result<Self> {
        // A group belongs only to this desktop. Killing just the launcher
        // bypasses its shell trap and leaves its browser/display children alive.
        #[cfg(unix)]
        command.process_group(0);
        let permit = ManagedSpawnPermit::begin();
        let child = command.kill_on_drop(true).spawn()?;
        let managed = permit.register(child.id());
        Ok(Self {
            child,
            _managed: managed,
        })
    }
}

impl Drop for DesktopProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id().and_then(|pid| i32::try_from(pid).ok()) {
            // The child is retained and never reaped while this guard lives,
            // so its process-group identifier cannot have been reused.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

    #[tokio::test]
    async fn dropping_desktop_closes_output_held_by_its_background_child() {
        let mut command = Command::new("/bin/bash");
        command
            .args(["-c", "sleep 120 & echo started; wait"])
            .stdout(std::process::Stdio::piped());
        let mut desktop = DesktopProcess::spawn(command).unwrap();
        let mut output = BufReader::new(desktop.child.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).await.unwrap();
        assert_eq!(line, "started\n");
        drop(desktop);
        let mut rest = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            output.read_to_end(&mut rest),
        )
        .await
        .expect("desktop background child retained its output pipe")
        .unwrap();
    }
}
