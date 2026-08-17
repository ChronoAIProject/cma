//! The kernel's conversation bridge, and the buffer that outlives a reconnect.
//!
//! The bridge is a process this pod starts and talks to over its standard
//! input and output. What crosses is newline-delimited frames, and this module
//! reads exactly one thing about them: where a line ends. Not what a frame
//! means, not what it is called, not whether it is well formed — the same
//! deliberate naivety the conversation capture commits to, for the same
//! reason. A kernel version that changes what a frame looks like must not be
//! able to break the pod hosting it.
//!
//! Its stdout is protocol and its stderr is not. Diagnostics go to the
//! supervisor's log, which is where a person looks; mixing them into the
//! protocol stream would make a bridge's debugging output indistinguishable
//! from what it is saying to the control plane.
//!
//! ## Why anything is buffered
//!
//! The terminal plane survives a control-plane restart because a screen can be
//! re-derived: the pty keeps running, and whoever reattaches is repainted from
//! the current state. A conversation has no such property. Everything the
//! agent did while nobody was listening — messages, tool calls, a turn that
//! completed — happened once and is gone if it was not carried. So frames
//! produced with nobody attached are held until somebody is, which is what
//! CLAUDE.md §5 requires of an upgrade that must not cost anyone their work.
//!
//! The buffer is bounded, because a pod cannot grow without limit while its
//! control plane is away. When the bound is reached the oldest frames are
//! dropped and a marker is left where they were — so the gap is recorded at
//! the point it happened and its owner can be shown a boundary, rather than
//! left to puzzle over a conversation that skips.

use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};

/// How much unacknowledged conversation this pod holds while detached.
///
/// Generous against a long control-plane restart and bounded against one that
/// never comes back. Frames are small; this is thousands of them.
const BUFFER_BYTES: usize = 8 * 1024 * 1024;

/// How many frames may queue toward a bridge before writing applies back
/// pressure.
const INPUT_BUFFER: usize = 256;

/// What came out of the bridge, in the order it came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    /// One frame, exactly as the bridge wrote it, without its newline.
    Frame(Vec<u8>),
    /// Frames were dropped here rather than kept.
    Gap {
        /// How many were dropped, so the boundary can say how large it is.
        frames: usize,
    },
}

/// Frames the bridge has produced that nobody has taken yet.
///
/// Separate from the relay so it survives the connection: the point of holding
/// anything is that the control plane is not here.
#[derive(Default)]
struct Pending {
    held: std::collections::VecDeque<Outbound>,
    bytes: usize,
    /// Frames dropped since the last marker, folded into one boundary rather
    /// than one marker per frame.
    dropped: usize,
}

impl Pending {
    /// Hold a frame, dropping the oldest if that is what it takes.
    fn push(&mut self, frame: Vec<u8>) {
        self.bytes += frame.len();
        self.held.push_back(Outbound::Frame(frame));
        while self.bytes > BUFFER_BYTES {
            match self.held.pop_front() {
                Some(Outbound::Frame(dropped)) => {
                    self.bytes -= dropped.len();
                    self.dropped += 1;
                }
                // A marker at the front is folded into the count being built
                // rather than stopping the trim, which it would otherwise do
                // for good — the oldest thing held would be a boundary, and
                // nothing behind it could ever be reached. The record is not
                // lost: its count carries into the boundary written below.
                Some(Outbound::Gap { frames }) => self.dropped += frames,
                None => break,
            }
        }
        if self.dropped > 0 {
            // One marker in front of what survives, carrying the whole count.
            // Where the gap falls is the useful part; how many separate times
            // the bound was hit is not.
            self.held.push_front(Outbound::Gap {
                frames: std::mem::take(&mut self.dropped),
            });
        }
    }

    /// Take everything held, leaving the buffer empty.
    fn drain(&mut self) -> Vec<Outbound> {
        self.bytes = 0;
        self.held.drain(..).collect()
    }
}

/// A running conversation bridge.
pub struct Bridge {
    /// Frames to write to the bridge's standard input.
    input: mpsc::Sender<Vec<u8>>,
    /// Everything the bridge has said, live.
    output: tokio::sync::broadcast::Sender<Outbound>,
    /// What it said while nobody was listening.
    pending: Arc<Mutex<Pending>>,
    /// Set once the bridge process has ended, whichever way it went.
    ended: Arc<tokio::sync::Notify>,
}

impl Bridge {
    /// Start a bridge, running `command` with its standard streams piped.
    ///
    /// The command is whatever this pod was told to host. Parsed the way a
    /// shell would split a simple command line, which is how the terminal
    /// plane already reads the same setting.
    pub fn start(command: &str) -> std::io::Result<Self> {
        let mut parts = command.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| std::io::Error::other("no bridge command configured"))?;
        let mut child = tokio::process::Command::new(program)
            .args(parts)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("bridge stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("bridge stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("bridge stderr was not piped"))?;

        let (input, input_rx) = mpsc::channel(INPUT_BUFFER);
        let (output, _) = tokio::sync::broadcast::channel(INPUT_BUFFER);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let ended = Arc::new(tokio::sync::Notify::new());

        spawn_writer(stdin, input_rx);
        spawn_reader(stdout, output.clone(), Arc::clone(&pending));
        spawn_diagnostics(stderr);
        spawn_reaper(child, Arc::clone(&ended));

        tracing::info!(%command, "conversation bridge started");
        Ok(Self {
            input,
            output,
            pending,
            ended,
        })
    }

    /// Send one frame to the bridge.
    pub async fn send(&self, frame: Vec<u8>) -> bool {
        self.input.send(frame).await.is_ok()
    }

    /// Subscribe to what the bridge says from now on.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Outbound> {
        self.output.subscribe()
    }

    /// Take everything said while nobody was attached.
    ///
    /// Called immediately after subscribing and before any live frame is
    /// forwarded, so what a reconnecting control plane sees is the whole
    /// conversation in order.
    pub async fn backlog(&self) -> Vec<Outbound> {
        self.pending.lock().await.drain()
    }

    /// Resolves when the bridge process has ended.
    pub async fn ended(&self) {
        self.ended.notified().await;
    }
}

/// Write frames to the bridge, one line each.
fn spawn_writer(mut stdin: tokio::process::ChildStdin, mut frames: mpsc::Receiver<Vec<u8>>) {
    tokio::spawn(async move {
        while let Some(frame) = frames.recv().await {
            if stdin.write_all(&frame).await.is_err() || stdin.write_all(b"\n").await.is_err() {
                break;
            }
            if stdin.flush().await.is_err() {
                break;
            }
        }
        tracing::info!("conversation bridge input closed");
    });
}

/// Read frames from the bridge, holding them when nobody is listening.
fn spawn_reader(
    stdout: tokio::process::ChildStdout,
    output: tokio::sync::broadcast::Sender<Outbound>,
    pending: Arc<Mutex<Pending>>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).split(b'\n');
        loop {
            match lines.next_segment().await {
                Ok(Some(frame)) => {
                    // A send with no subscriber is not a failure; it is the
                    // case this whole module exists for, and the frame is
                    // held instead.
                    if output.send(Outbound::Frame(frame.clone())).is_err() {
                        pending.lock().await.push(frame);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "conversation bridge output failed");
                    break;
                }
            }
        }
        tracing::info!("conversation bridge output ended");
    });
}

/// Forward the bridge's diagnostics to this pod's log.
///
/// Its stdout is protocol and its stderr is not. A bridge writing a warning
/// must not have that warning read as something it said to the control plane.
fn spawn_diagnostics(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(bridge = %line, "conversation bridge said");
        }
    });
}

/// Notice when the bridge process ends, and say so.
///
/// A bridge that exits mid-conversation is the runtime being lost, and has to
/// surface as that rather than as a stream that quietly stops.
fn spawn_reaper(mut child: tokio::process::Child, ended: Arc<tokio::sync::Notify>) {
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                tracing::warn!(%status, "conversation bridge exited")
            }
            Err(e) => tracing::warn!(error = %e, "conversation bridge could not be waited on"),
        }
        ended.notify_waiters();
    });
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod bridge_tests;
