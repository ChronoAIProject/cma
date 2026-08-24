//! One supervised agent process per runtime, under a real pseudo-terminal.
//!
//! A pipe is not a substitute for a pty here. The agent kernels render
//! full-screen interfaces: they query the terminal, switch to the alternate
//! screen buffer, and repaint regions. Given a pipe they either refuse to
//! render or produce output no terminal emulator can reconstruct. So the
//! process gets a genuine pty, with real window-size propagation.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex as StdMutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, watch, Mutex};

/// How many output chunks may buffer for a subscriber before it is dropped.
const OUTPUT_BUFFER: usize = 512;

/// Bytes read from the pty in one go.
const READ_CHUNK: usize = 8192;

/// A live agent process and its pty.
pub struct PtySession {
    master: Arc<StdMutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<StdMutex<Box<dyn Write + Send>>>,
    /// Fan-out of everything the process has written since it started.
    output: broadcast::Sender<Vec<u8>>,
    /// The process ending, latched so a reattach to an ended process still
    /// receives the real status rather than waiting on a channel that is done.
    exit: watch::Receiver<Option<ProcessExit>>,
}

/// How the supervised process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessExit {
    /// Its numeric status, or `None` when a signal ended it.
    pub(crate) code: Option<i32>,
}

impl PtySession {
    /// Watch for the process to end.
    pub(crate) fn subscribe_exit(&self) -> watch::Receiver<Option<ProcessExit>> {
        self.exit.clone()
    }

    /// Write bytes to the process as if they had been typed.
    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| std::io::Error::other("pty writer poisoned"))?;
        w.write_all(bytes)?;
        w.flush()
    }

    /// Propagate a new window size to the process.
    ///
    /// This is what makes a full-screen application repaint at the client's
    /// real size; without it the process keeps painting at the size it was
    /// started with and the client sees corruption that never self-heals.
    pub fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        if cols == 0 || rows == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "terminal geometry must be non-zero in both dimensions",
            ));
        }
        let m = self
            .master
            .lock()
            .map_err(|_| std::io::Error::other("pty master poisoned"))?;
        m.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(std::io::Error::other)
    }
}

/// All live runtimes on this supervisor, keyed by runtime id.
#[derive(Clone, Default)]
pub struct PtyRegistry {
    inner: Arc<Mutex<HashMap<String, Arc<PtySession>>>>,
}

impl PtyRegistry {
    /// Build an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch the runtime for `id`, starting it if this is the first attach.
    ///
    /// Keyed rather than per-connection so that a dropped connection detaches
    /// without killing the process: the agent keeps working, and reattaching
    /// finds the same session.
    pub async fn get_or_start(
        &self,
        id: &str,
        command: &str,
        cols: u16,
        rows: u16,
        placeholder_key: &str,
    ) -> std::io::Result<(Arc<PtySession>, broadcast::Receiver<Vec<u8>>)> {
        let mut sessions = self.inner.lock().await;
        if let Some(existing) = sessions.get(id) {
            tracing::info!(runtime = %id, "reattaching to existing runtime");
            return Ok((Arc::clone(existing), existing.output.subscribe()));
        }

        tracing::info!(runtime = %id, %command, cols, rows, "starting runtime process");
        let (started, output) = start_process(command, cols, rows, placeholder_key)?;
        let session = Arc::new(started);
        sessions.insert(id.to_string(), Arc::clone(&session));
        Ok((session, output))
    }

    /// Whether any agent has been started on this supervisor.
    ///
    /// Asked before accepting a conversation to resume from: the agent reads
    /// its history when it starts, so anything laid down after that is never
    /// seen, and accepting it would report a handover that did not happen.
    pub async fn is_running(&self) -> bool {
        !self.inner.lock().await.is_empty()
    }

    /// Drop a runtime, ending its process.
    #[allow(dead_code)] // part of the registry's API; exercised by tests
    pub async fn remove(&self, id: &str) {
        if self.inner.lock().await.remove(id).is_some() {
            tracing::info!(runtime = %id, "runtime removed");
        }
    }

    /// How many runtimes are live.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// Whether no runtime is live.
    #[allow(dead_code)] // part of the registry's API; exercised by tests
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

fn start_process(
    command: &str,
    cols: u16,
    rows: u16,
    placeholder_key: &str,
) -> std::io::Result<(PtySession, broadcast::Receiver<Vec<u8>>)> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(std::io::Error::other)?;

    let mut cmd = CommandBuilder::new_default_prog();
    let mut parts = command.split_whitespace();
    if let Some(program) = parts.next() {
        cmd = CommandBuilder::new(program);
        for arg in parts {
            cmd.arg(arg);
        }
    }
    // The kernel decides how to render; the client must not negotiate this.
    cmd.env("TERM", "xterm-256color");
    // The kernel refuses to start without a key for its provider. This one is
    // inert: the supervisor's proxy strips it and attaches the real credential,
    // so nothing reachable from inside the sandbox is usable elsewhere.
    cmd.env("CHRONO_UPSTREAM_PLACEHOLDER_KEY", placeholder_key);
    cmd.env("COLORTERM", "truecolor");
    if let Ok(home) = std::env::var("HOME") {
        cmd.cwd(home);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(std::io::Error::other)?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(std::io::Error::other)?;
    let writer = pair.master.take_writer().map_err(std::io::Error::other)?;

    let (output, _) = broadcast::channel(OUTPUT_BUFFER);
    // The first receiver exists before the reader thread can publish. Without
    // this, a short-lived program can print and exit between spawn and attach.
    let first_output = output.subscribe();
    let (exit_tx, exit) = watch::channel(None);

    // The pty reader is blocking, so it owns a dedicated thread rather than
    // occupying an async worker.
    let out = output.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    // No subscribers simply means nobody is attached right now.
                    let _ = out.send(buf[..n].to_vec());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "pty read failed");
                    break;
                }
            }
        }
        tracing::info!("pty output ended");
        // Reap only after the pty has reached EOF. The status therefore cannot
        // overtake bytes the process wrote immediately before it ended.
        match child.wait() {
            Ok(status) => {
                let code = if status.signal().is_some() {
                    None
                } else {
                    i32::try_from(status.exit_code()).ok()
                };
                tracing::info!(?status, "runtime process exited");
                exit_tx.send_replace(Some(ProcessExit { code }));
            }
            Err(e) => tracing::warn!(error = %e, "waiting on runtime process failed"),
        }
    });

    Ok((
        PtySession {
            master: Arc::new(StdMutex::new(pair.master)),
            writer: Arc::new(StdMutex::new(writer)),
            output,
            exit,
        },
        first_output,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_started_runtime_is_reused_on_reattach() {
        let reg = PtyRegistry::new();
        assert!(reg.is_empty().await);

        let (a, _output) = reg
            .get_or_start("r1", "/bin/cat", 80, 24, "k")
            .await
            .expect("starts");
        let (b, _output) = reg
            .get_or_start("r1", "/bin/cat", 80, 24, "k")
            .await
            .expect("reattaches");
        assert!(Arc::ptr_eq(&a, &b), "reattach must reuse the same process");
        assert_eq!(reg.len().await, 1);

        reg.remove("r1").await;
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn distinct_ids_get_distinct_processes() {
        let reg = PtyRegistry::new();
        let (a, _output) = reg
            .get_or_start("a", "/bin/cat", 80, 24, "k")
            .await
            .expect("a");
        let (b, _output) = reg
            .get_or_start("b", "/bin/cat", 80, 24, "k")
            .await
            .expect("b");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(reg.len().await, 2);
    }

    #[tokio::test]
    async fn input_is_echoed_back_through_the_pty() {
        let reg = PtyRegistry::new();
        let (s, mut rx) = reg
            .get_or_start("echo", "/bin/cat", 80, 24, "k")
            .await
            .expect("starts");
        s.write_input(b"ping\n").expect("write succeeds");

        let got = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut seen = Vec::new();
            while let Ok(chunk) = rx.recv().await {
                seen.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&seen).contains("ping") {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(got, "expected the pty to echo the input back");
    }

    #[tokio::test]
    async fn a_quick_process_exit_is_latched_for_reattach() {
        let reg = PtyRegistry::new();
        let (started, _output) = reg
            .get_or_start("quick", "/bin/echo quick", 80, 24, "k")
            .await
            .expect("starts");
        let mut first_exit = started.subscribe_exit();
        tokio::time::timeout(std::time::Duration::from_secs(5), first_exit.changed())
            .await
            .expect("the process exits in time")
            .expect("the exit watch remains open");
        assert_eq!(*first_exit.borrow(), Some(ProcessExit { code: Some(0) }));

        let (reattached, _output) = reg
            .get_or_start("quick", "/bin/echo replacement", 80, 24, "k")
            .await
            .expect("reattaches");
        assert!(Arc::ptr_eq(&started, &reattached));
        let reattached_exit = reattached.subscribe_exit();
        assert_eq!(
            *reattached_exit.borrow(),
            Some(ProcessExit { code: Some(0) }),
            "reattach must see the original process's latched status"
        );
    }

    #[tokio::test]
    async fn zero_geometry_is_refused() {
        let reg = PtyRegistry::new();
        let (s, _output) = reg
            .get_or_start("z", "/bin/cat", 80, 24, "k")
            .await
            .expect("starts");
        assert!(s.resize(0, 24).is_err());
        assert!(s.resize(80, 0).is_err());
        assert!(s.resize(100, 40).is_ok());
    }
}
