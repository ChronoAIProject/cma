//! Lifetime ownership of the runtime's shared desktop process group.
use tokio::process::{Child, Command};
use tokio::sync::watch;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::writer_fence::{ManagedChild, ManagedSpawnPermit};

const LAUNCHER: &str = "/usr/local/bin/cma-desktop-session";
const GENERATION_DIR_ENV: &str = "CMA_DESKTOP_GENERATION_DIR";
const TOKEN_FILE_ENV: &str = "CMA_DESKTOP_TOKEN_FILE";

#[cfg(target_os = "linux")]
pub(crate) mod guardian;
pub(crate) mod legacy;
#[cfg(any(target_os = "linux", test))]
mod restart_budget;
#[cfg(target_os = "linux")]
pub(crate) mod supervision;
pub(crate) mod transport;

pub(crate) fn supervision_enabled(
    env: &std::collections::BTreeMap<String, String>,
) -> std::result::Result<bool, String> {
    const FLAG: &str = "CHRONO_RUNTIME_DESKTOP_SUPERVISION_ENABLED";
    match env.get(FLAG).map(String::as_str) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(format!("{FLAG} must be true or false")),
    }
}

#[derive(Clone)]
pub(crate) struct DesktopGeneration {
    token: String,
    invalidation: Option<watch::Receiver<()>>,
    #[cfg(target_os = "linux")]
    listener: Option<std::sync::Arc<transport::DisplayBinding>>,
}

impl DesktopGeneration {
    pub(crate) fn legacy(token: String) -> Self {
        Self {
            token,
            invalidation: None,
            #[cfg(target_os = "linux")]
            listener: None,
        }
    }

    #[cfg(target_os = "linux")]
    fn supervised(token: String, listener: transport::DisplayBinding) -> (Self, watch::Sender<()>) {
        let mut generation = Self::legacy(token);
        generation.listener = Some(std::sync::Arc::new(listener));
        generation.with_invalidation()
    }

    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn with_invalidation(mut self) -> (Self, watch::Sender<()>) {
        let (owner, invalidation) = watch::channel(());
        self.invalidation = Some(invalidation);
        (self, owner)
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn active(&self) -> bool {
        self.invalidation
            .as_ref()
            .is_none_or(|signal| signal.has_changed().is_ok())
    }

    pub(crate) async fn invalidated(&self) {
        if let Some(mut signal) = self.invalidation.clone() {
            // The sender belongs to the desktop generation and is dropped
            // before replacement can begin.
            let _ = signal.changed().await;
        } else {
            std::future::pending::<()>().await;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopState {
    Starting,
    Ready,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Restarting,
    Failed,
}

impl DesktopState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Restarting => "restarting",
            Self::Failed => "failed",
        }
    }
}

pub(crate) enum DesktopSession {
    Legacy(Box<legacy::LegacyDesktop>),
    #[cfg(target_os = "linux")]
    Supervised(Box<supervision::Supervision>),
}

#[derive(Debug)]
pub(crate) struct DesktopUnavailable {
    pub(crate) retryable: bool,
}

impl DesktopSession {
    pub(crate) fn start(config: &RuntimeConfig) -> Result<Self> {
        let child = DesktopProcess::start(config)?;
        #[cfg(target_os = "linux")]
        if config.desktop_supervision_enabled {
            return Ok(Self::Supervised(Box::new(supervision::Supervision::new(
                config.clone(),
                child,
            ))));
        }
        Ok(Self::Legacy(Box::new(legacy::LegacyDesktop::new(child))))
    }

    pub(crate) fn status(&self) -> DesktopState {
        match self {
            Self::Legacy(process) => process.status(),
            #[cfg(target_os = "linux")]
            Self::Supervised(process) => process.status(),
        }
    }

    pub(crate) fn generation(&self) -> std::result::Result<DesktopGeneration, DesktopUnavailable> {
        match self {
            Self::Legacy(process) => process.generation(),
            #[cfg(target_os = "linux")]
            Self::Supervised(process) => process.generation(),
        }
    }

    pub(crate) async fn start_background_with_probe<Fut>(
        &self,
        budget: std::time::Duration,
        connect: impl FnMut() -> Fut + Send + 'static,
    ) where
        Fut: std::future::Future<Output = bool> + Send + 'static,
    {
        match self {
            Self::Legacy(process) => process.start_background_with_probe(budget, connect),
            #[cfg(target_os = "linux")]
            Self::Supervised(process) => {
                let _ = connect;
                process.start_background(budget).await;
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn wait_initial_ready(&self) -> bool {
        match self {
            Self::Legacy(process) => process.wait_initial_ready().await,
            #[cfg(target_os = "linux")]
            Self::Supervised(process) => process.wait_initial_ready().await,
        }
    }

    pub(crate) async fn shutdown(&self) {
        match self {
            Self::Legacy(process) => process.shutdown().await,
            #[cfg(target_os = "linux")]
            Self::Supervised(process) => process.shutdown().await,
        }
    }
}

pub(crate) struct DesktopProcess {
    child: Child,
    _managed: ManagedChild,
    token: String,
    #[cfg(target_os = "linux")]
    guard_files: Option<GenerationDirectory>,
    #[cfg(target_os = "linux")]
    backend: Option<std::sync::Arc<transport::OwnedListener>>,
}

impl DesktopProcess {
    pub(crate) fn start(config: &RuntimeConfig) -> Result<Self> {
        let result = if config.desktop_supervision_enabled {
            Self::start_guarded(config)
        } else {
            Self::start_legacy(config)
        };
        result.map_err(|error| {
            RuntimeError::unavailable(
                "desktop_start_failed",
                format!("start desktop session: {error}"),
            )
        })
    }

    fn start_legacy(config: &RuntimeConfig) -> std::io::Result<Self> {
        let mut command = Command::new(LAUNCHER);
        command
            .env_clear()
            .envs(&config.install_env)
            .current_dir(&config.home);
        Self::spawn(command)
    }

    fn spawn(mut command: Command) -> std::io::Result<Self> {
        command
            .env_remove(GENERATION_DIR_ENV)
            .env_remove(TOKEN_FILE_ENV);
        Self::spawn_in_group(command, true)
    }

    fn spawn_in_group(mut command: Command, kill_on_drop: bool) -> std::io::Result<Self> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        command.env("CMA_DESKTOP_TOKEN", &token);
        // A group belongs only to this desktop. Killing just the launcher
        // bypasses its shell trap and leaves its browser/display children alive.
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // Browser helpers may double-fork or start their own session.
                // Keep them under the desktop launcher, rather than adopting
                // them into the runtime beside unrelated Agent processes.
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let permit = ManagedSpawnPermit::begin();
        let child = command.kill_on_drop(kill_on_drop).spawn()?;
        let managed = permit.register(child.id());
        Ok(Self {
            child,
            _managed: managed,
            token,
            #[cfg(target_os = "linux")]
            guard_files: None,
            #[cfg(target_os = "linux")]
            backend: None,
        })
    }

    #[cfg(target_os = "linux")]
    fn start_guarded(config: &RuntimeConfig) -> std::io::Result<Self> {
        use std::process::Stdio;

        let files = GenerationDirectory::create()?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg(guardian::COMMAND)
            .env_clear()
            .envs(&config.install_env)
            .env(GENERATION_DIR_ENV, &files.path)
            .env(
                TOKEN_FILE_ENV,
                transport::token_map::endpoint(&files.token_map),
            )
            .current_dir(&config.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut process = Self::spawn_in_group(command, false)?;
        process.guard_files = Some(files);
        Ok(process)
    }

    #[cfg(not(target_os = "linux"))]
    fn start_guarded(_config: &RuntimeConfig) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "desktop supervision requires Linux child custody",
        ))
    }

    #[cfg(target_os = "linux")]
    async fn guardian_event(&mut self, expected: u8) -> std::result::Result<(), String> {
        use tokio::io::AsyncReadExt as _;

        let pipe = self
            .child
            .stdout
            .as_mut()
            .ok_or("desktop guardian has no status pipe")?;
        let mut event = [0];
        let count = pipe
            .read(&mut event)
            .await
            .map_err(|error| format!("read desktop guardian status: {error}"))?;
        if count != 1 || event[0] != expected {
            return Err("desktop guardian did not confirm its lifecycle event".into());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    async fn stopped(&mut self) -> std::result::Result<(), String> {
        self.guardian_event(guardian::STOPPED).await
    }

    #[cfg(target_os = "linux")]
    fn fence(&mut self) {
        self.child.stdin.take();
        if let Some(pid) = self.child.id().and_then(|pid| i32::try_from(pid).ok()) {
            // The unreaped managed leader still anchors this group even when
            // the guardian could not account for helpers in other sessions.
            if unsafe { libc::kill(-pid, libc::SIGKILL) } == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(%error, "could not stop fenced desktop group");
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn finish(&mut self, observed_stop: bool) -> std::result::Result<(), String> {
        self.child.stdin.take();
        tokio::time::timeout(
            guardian::CLEANUP_LIMIT + std::time::Duration::from_secs(1),
            async {
                if !observed_stop {
                    self.stopped().await?;
                }
                self.guardian_event(guardian::REAPED).await?;
                let outcome = self
                    .child
                    .wait()
                    .await
                    .map_err(|error| format!("reap desktop guardian: {error}"))?;
                if !outcome.success() {
                    return Err(format!("desktop guardian exited with {outcome}"));
                }
                Ok(())
            },
        )
        .await
        .map_err(|_| "desktop guardian cleanup exceeded its deadline".to_string())?
    }

    pub(crate) fn websocket_token(&self) -> &str {
        &self.token
    }

    pub(crate) fn has_exited_unreaped(&self) -> bool {
        #[cfg(unix)]
        {
            let Some(pid) = self.child.id() else {
                return true;
            };
            // Inspect exit status with WNOWAIT so the process remains unreaped
            // and its process-group ID cannot be reused before Drop signals it.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                return std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD);
            }
            info.si_signo != 0
        }
        #[cfg(not(unix))]
        {
            self.child.id().is_none()
        }
    }

    #[cfg(target_os = "linux")]
    fn display_directory(&self) -> (&GenerationDirectory, i32) {
        let files = self
            .guard_files
            .as_ref()
            .expect("supervised desktop directory");
        let leader = self.child.id().expect("unreaped desktop guardian PID") as i32;
        (files, leader)
    }
}

impl Drop for DesktopProcess {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if self.guard_files.is_some() {
            // The runtime can go away before its async shutdown path runs.
            // Closing this pipe gives the subreaper custody of that cleanup.
            self.child.stdin.take();
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = self.child.id().and_then(|pid| i32::try_from(pid).ok()) {
            // The child is retained and never reaped while this guard lives,
            // so its process-group identifier cannot have been reused.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
}

#[cfg(target_os = "linux")]
struct GenerationDirectory {
    path: std::path::PathBuf,
    token_map: std::sync::Arc<std::fs::File>,
}

#[cfg(target_os = "linux")]
impl GenerationDirectory {
    fn create() -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt as _;

        let path = std::path::Path::new("/tmp")
            .join(format!("cma-desktop-{}", uuid::Uuid::new_v4().simple()));
        let token_map = std::sync::Arc::new(transport::token_map::create()?);
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self { path, token_map })
    }
}

#[cfg(target_os = "linux")]
impl Drop for GenerationDirectory {
    fn drop(&mut self) {
        for result in [
            std::fs::remove_file(self.path.join("token-ready")),
            std::fs::remove_file(self.path.join("display.sock")),
            std::fs::remove_file(self.path.join("rfb.sock")),
            std::fs::remove_file(self.path.join("omarchy-ready")),
            std::fs::remove_dir(&self.path),
        ] {
            match result {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    tracing::warn!(%error, "could not remove desktop generation files");
                }
                _ => {}
            }
        }
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests;
