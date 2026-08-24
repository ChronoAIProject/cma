//! One supervised agent process per runtime, under a real pseudo-terminal.
//!
//! A pipe is not a substitute for a pty here. The agent kernels render
//! full-screen interfaces: they query the terminal, switch to the alternate
//! screen buffer, and repaint regions. Given a pipe they either refuse to
//! render or produce output no terminal emulator can reconstruct. So the
//! process gets a genuine pty, with real window-size propagation.

use std::collections::HashMap;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::sync::{Arc, Mutex as StdMutex};
#[cfg(unix)]
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, watch, Mutex};

/// How many output chunks may buffer for a subscriber before it is dropped.
const OUTPUT_BUFFER: usize = 512;

/// Bytes read from the pty in one go.
const READ_CHUNK: usize = 8192;

/// Longest the supervisor waits between checking the primary child.
///
/// PTY readability cannot be the child-liveness signal: a descendant may keep
/// the slave open after the supervised process exits.
#[cfg(unix)]
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Bound on output draining after the primary process has been reaped.
///
/// Ordinarily the drain stops immediately when the kernel buffer is empty. A
/// descendant that escaped the primary's process group can keep writing, so a
/// bound is still required to make the primary's ending report inevitable.
#[cfg(unix)]
const EXIT_DRAIN_DEADLINE: Duration = Duration::from_millis(250);

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

    /// Replace this session's writer with one that reports an observed failure.
    #[cfg(test)]
    pub(crate) fn fail_input_writes_for_test(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (observed, failure_observed) = tokio::sync::oneshot::channel();
        let mut writer = self.writer.lock().expect("the pty writer is healthy");
        *writer = Box::new(FailingWriter {
            observed: Some(observed),
        });
        failure_observed
    }

    /// Replace this session's writer with one that records every input byte.
    #[cfg(test)]
    pub(crate) fn capture_input_writes_for_test(&self) -> Arc<StdMutex<Vec<u8>>> {
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let mut writer = self.writer.lock().expect("the pty writer is healthy");
        *writer = Box::new(CapturingWriter(Arc::clone(&captured)));
        captured
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

#[cfg(test)]
struct FailingWriter {
    observed: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(test)]
impl Write for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(observed) = self.observed.take() {
            let _ = observed.send(());
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "injected pty write failure",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
struct CapturingWriter(Arc<StdMutex<Vec<u8>>>);

#[cfg(test)]
impl Write for CapturingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("the captured input is healthy")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
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

    /// Inspect a started runtime from an in-crate integration test.
    #[cfg(test)]
    pub(crate) async fn session_for_test(&self, id: &str) -> Option<Arc<PtySession>> {
        self.inner.lock().await.get(id).cloned()
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

    let child = pair
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

    // PTY reads and child polling are blocking operating-system work, so one
    // dedicated thread supervises both rather than occupying an async worker.
    let out = output.clone();
    #[cfg(unix)]
    let terminal_fd = {
        let raw = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| std::io::Error::other("native pty exposes no pollable descriptor"))?;
        // `MasterPty` exposes only a borrowed integer. Duplicate it while the
        // owner is still alive so the supervisor never polls a descriptor that
        // a removed session has closed and the process has since reused.
        unsafe { BorrowedFd::borrow_raw(raw) }.try_clone_to_owned()?
    };
    std::thread::spawn(move || {
        #[cfg(unix)]
        supervise_process(child, &mut reader, terminal_fd, &out, exit_tx);
        #[cfg(not(unix))]
        supervise_process(child, &mut reader, &out, exit_tx);
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

fn process_exit(status: &portable_pty::ExitStatus) -> ProcessExit {
    let code = if status.signal().is_some() {
        None
    } else {
        i32::try_from(status.exit_code()).ok()
    };
    ProcessExit { code }
}

/// Supervise the primary process without treating PTY EOF as its liveness.
///
/// `portable-pty` starts the primary as a new session leader. Polling its
/// handle reaps that exact process even when one of its descendants inherited
/// the slave descriptor. Once it exits, every byte already readable from the
/// master is forwarded, then the status is latched. No output-forwarding path
/// survives this function, so terminal bytes cannot overtake the report.
#[cfg(unix)]
fn supervise_process(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: &mut Box<dyn Read + Send>,
    terminal_fd: OwnedFd,
    output: &broadcast::Sender<Vec<u8>>,
    exit: watch::Sender<Option<ProcessExit>>,
) {
    let process_group = child.process_id().and_then(|pid| i32::try_from(pid).ok());
    let mut terminal_open = true;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The primary is gone, so none of its descendants may remain
                // attached to this terminal. Kill its process group before
                // draining; an escaped descendant is handled by the deadline.
                terminate_process_group(process_group);
                if terminal_open {
                    drain_after_exit(reader, terminal_fd.as_fd(), output);
                }
                tracing::info!(?status, "runtime process exited");
                exit.send_replace(Some(process_exit(&status)));
                return;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "polling runtime process failed");
                return;
            }
        }

        if !terminal_open {
            std::thread::sleep(PROCESS_POLL_INTERVAL);
            continue;
        }

        match poll_terminal(terminal_fd.as_fd(), PROCESS_POLL_INTERVAL) {
            Ok(true) => terminal_open = forward_one_chunk(reader, output),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, "polling pty output failed");
                terminal_open = false;
            }
        }
    }
}

#[cfg(unix)]
fn drain_after_exit(
    reader: &mut Box<dyn Read + Send>,
    terminal_fd: BorrowedFd<'_>,
    output: &broadcast::Sender<Vec<u8>>,
) {
    let deadline = Instant::now() + EXIT_DRAIN_DEADLINE;
    while Instant::now() < deadline {
        match poll_terminal(terminal_fd, Duration::ZERO) {
            Ok(true) if forward_one_chunk(reader, output) => {}
            Ok(true) | Ok(false) => return,
            Err(error) => {
                tracing::warn!(%error, "draining pty output failed");
                return;
            }
        }
    }
    tracing::warn!("pty output remained busy after the primary process exited; ending the drain");
}

#[cfg(unix)]
fn poll_terminal(fd: BorrowedFd<'_>, timeout: Duration) -> std::io::Result<bool> {
    let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    loop {
        let mut descriptor = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result == 0 {
            return Ok(false);
        }
        if result > 0 {
            if descriptor.revents & libc::POLLNVAL != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "poll reported an invalid pty descriptor",
                ));
            }
            if descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                return Ok(true);
            }
            return Err(std::io::Error::other(format!(
                "poll reported unexpected pty events: {}",
                descriptor.revents
            )));
        }

        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn forward_one_chunk(
    reader: &mut Box<dyn Read + Send>,
    output: &broadcast::Sender<Vec<u8>>,
) -> bool {
    let mut buffer = vec![0u8; READ_CHUNK];
    match reader.read(&mut buffer) {
        Ok(0) => {
            tracing::info!("pty output ended");
            false
        }
        Ok(read) => {
            // No subscribers simply means nobody is attached right now.
            let _ = output.send(buffer[..read].to_vec());
            true
        }
        Err(error) => {
            tracing::warn!(%error, "pty read failed");
            false
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group: Option<i32>) {
    let Some(process_group) = process_group.filter(|pid| *pid > 0) else {
        return;
    };
    let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(%error, process_group, "terminating runtime process group failed");
        }
    }
}

/// Non-Unix native PTYs do not expose a pollable descriptor. Preserve their
/// blocking drain, while still keeping the platform-specific code isolated.
#[cfg(not(unix))]
fn supervise_process(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: &mut Box<dyn Read + Send>,
    output: &broadcast::Sender<Vec<u8>>,
    exit: watch::Sender<Option<ProcessExit>>,
) {
    while forward_one_chunk(reader, output) {}
    match child.wait() {
        Ok(status) => {
            tracing::info!(?status, "runtime process exited");
            exit.send_replace(Some(process_exit(&status)));
        }
        Err(error) => tracing::warn!(%error, "waiting on runtime process failed"),
    }
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

    #[cfg(unix)]
    #[tokio::test]
    async fn a_descendant_holding_the_slave_cannot_hide_the_primary_exit() {
        let reg = PtyRegistry::new();
        let (session, mut output) = reg
            .get_or_start("descendant", "/bin/sh", 80, 24, "k")
            .await
            .expect("starts");
        let mut exited = session.subscribe_exit();

        session
            .write_input(b"sleep 3 &\nprintf primary-; printf final-output\nexit 7\n")
            .expect("the shell receives its commands");

        tokio::time::timeout(Duration::from_secs(1), exited.changed())
            .await
            .expect("the primary exit is not held behind the descendant")
            .expect("the exit watch remains open");
        assert_eq!(*exited.borrow(), Some(ProcessExit { code: Some(7) }));

        let mut terminal = Vec::new();
        while let Ok(chunk) = output.try_recv() {
            terminal.extend_from_slice(&chunk);
        }
        assert!(
            String::from_utf8_lossy(&terminal).contains("primary-final-output"),
            "the primary's final output is drained before its status: {terminal:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn removing_a_silent_live_session_does_not_strand_its_supervisor() {
        let reg = PtyRegistry::new();
        let (session, mut output) = reg
            .get_or_start("silent", "/bin/sh", 80, 24, "k")
            .await
            .expect("the silent process starts");
        let original_master = session
            .master
            .lock()
            .expect("the master lock is healthy")
            .as_raw_fd()
            .expect("the native pty has a descriptor");
        let mut exited = session.subscribe_exit();
        let writer = Arc::clone(&session.writer);

        // Keep the shell silent so a readable replacement descriptor cannot
        // accidentally make the real pty reader return before its slave shuts.
        session
            .write_input(b"PS1=; trap '' HUP; stty -echo; printf '\\101\\122\\115\\105\\104'\n")
            .expect("the shell disables terminal echo");
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut seen = Vec::new();
            while let Ok(chunk) = output.recv().await {
                seen.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&seen).contains("ARMED") {
                    return;
                }
            }
            panic!("the shell output channel closed before it was armed");
        })
        .await
        .expect("the shell disables echo in time");

        reg.remove("silent").await;
        drop(session);

        // Reuse the session owner's descriptor while the silent child is still
        // alive. The supervisor must keep polling its own duplicate, rather
        // than mistake this unrelated readable file for terminal output and
        // block forever on the real reader.
        let mut replacements = Vec::new();
        let mut reused = None;
        while replacements.len() < 64 {
            let pair = std::os::unix::net::UnixStream::pair().expect("a local socket pair opens");
            let target = if pair.0.as_raw_fd() == original_master {
                Some(true)
            } else if pair.1.as_raw_fd() == original_master {
                Some(false)
            } else {
                None
            };
            replacements.push(pair);
            if let Some(target_is_left) = target {
                reused = Some((replacements.len() - 1, target_is_left));
                break;
            }
        }
        let (pair_index, target_is_left) =
            reused.expect("the removed session's descriptor is reused for the regression");

        // Let the supervisor repoll the quiet replacement, then make only that
        // descriptor readable. An implementation that retained the bare
        // integer consequently blocks on the unrelated pty reader here.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let peer = if target_is_left {
            &mut replacements[pair_index].1
        } else {
            &mut replacements[pair_index].0
        };
        peer.write_all(b"wake")
            .expect("the replacement is signalled");
        peer.flush().expect("the replacement signal is flushed");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            *exited.borrow(),
            None,
            "the primary stays alive until the explicit exit command"
        );
        {
            let mut writer = writer.lock().expect("the retained writer is healthy");
            writer
                .write_all(b"exec /bin/sh -c 'sleep 5 & exit 0'\n")
                .expect("the primary receives its exit command");
            writer.flush().expect("the exit command is flushed");
        }
        drop(writer);

        tokio::time::timeout(Duration::from_secs(2), exited.changed())
            .await
            .expect("the primary exit is reported before its child closes the pty slave")
            .expect("the exit watch remains open");
        assert_eq!(*exited.borrow(), Some(ProcessExit { code: Some(0) }));
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
