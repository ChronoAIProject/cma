use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use layer3_adapter::agent_capacity_recovery::{
    CapacityRecoveryConfirmation, CapacityRecoveryError, CapacityRecoveryNativeProof,
    CapacityRecoveryRequest,
};
use layer3_adapter::AgentLaunch;
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use tokio::io::{AsyncReadExt as _, BufReader};
use tokio::sync::{mpsc, Mutex};

#[path = "bridge_observation.rs"]
mod observation;
#[path = "bridge_recovery.rs"]
mod recovery;
#[path = "bridge_output.rs"]
mod retained_output;
pub(crate) use retained_output::OutputRetention;
#[path = "bridge_input.rs"]
mod input;
use input::{spawn_writer, BridgeInput};
#[path = "bridge_stale_turn.rs"]
mod stale_turn;
pub(crate) use stale_turn::FLAG as STALE_TURN_SEAL_FLAG;

const BUFFER_BYTES: usize = 8 * 1024 * 1024;
const INPUT_BUFFER: usize = 256;
pub(crate) const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const CLOSE_POLL: Duration = Duration::from_millis(10);
const CAPACITY_RECOVERY_LEASE: Duration = Duration::from_secs(30);
const OUTPUT_UNSELECTED: u8 = 0;
const OUTPUT_LEGACY: u8 = 1;
const OUTPUT_ACKNOWLEDGED: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outbound {
    Frame(Vec<u8>),
    Gap { frames: usize },
}

#[derive(Default)]
struct Pending {
    held: std::collections::VecDeque<Outbound>,
    bytes: usize,
    dropped: usize,
}

impl Pending {
    fn push(&mut self, frame: Vec<u8>) {
        self.bytes = self.bytes.saturating_add(frame.len());
        self.held.push_back(Outbound::Frame(frame));
        while self.bytes > BUFFER_BYTES {
            match self.held.pop_front() {
                Some(Outbound::Frame(bytes)) => {
                    self.bytes = self.bytes.saturating_sub(bytes.len());
                    self.dropped = self.dropped.saturating_add(1);
                }
                Some(Outbound::Gap { frames }) => {
                    self.dropped = self.dropped.saturating_add(frames);
                }
                None => break,
            }
        }
        self.record_gap();
    }

    fn gap(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
        self.record_gap();
    }

    fn record_gap(&mut self) {
        if self.dropped > 0 {
            self.held.push_front(Outbound::Gap {
                frames: std::mem::take(&mut self.dropped),
            });
        }
    }

    fn drain(&mut self) -> Vec<Outbound> {
        self.bytes = 0;
        self.held.drain(..).collect()
    }
}

pub(crate) struct Bridge {
    input: mpsc::Sender<BridgeInput>,
    output: tokio::sync::broadcast::Sender<Outbound>,
    pending: Arc<Mutex<Pending>>,
    pub(crate) retained_output: Arc<OutputRetention>,
    output_mode: AtomicU8,
    reader: tokio::task::AbortHandle,
    ended: tokio::sync::watch::Sender<bool>,
    writer_session_root: Option<i32>,
    observation: Option<Arc<Mutex<observation::NativeObservation>>>,
    capacity_recovery: Mutex<recovery::LeaseState>,
    stale_turn: Option<Arc<Mutex<Option<stale_turn::Stop>>>>,
}

impl Bridge {
    #[cfg(test)]
    pub(crate) fn start(
        launch: &AgentLaunch,
        environment: &BTreeMap<String, String>,
    ) -> std::io::Result<Self> {
        Self::start_with_recovery(launch, environment, false)
    }

    pub(crate) fn start_with_recovery(
        launch: &AgentLaunch,
        environment: &BTreeMap<String, String>,
        recovery_enabled: bool,
    ) -> std::io::Result<Self> {
        let mut process = tokio::process::Command::new(&launch.program);
        process
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .env_clear()
            .envs(environment)
            .env("HOME", &launch.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        unsafe {
            process.as_std_mut().pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let spawn = crate::writer_fence::ManagedSpawnPermit::begin();
        let mut child = process.spawn()?;
        let managed = spawn.register(child.id());
        let writer_session_root = child.id().and_then(|pid| i32::try_from(pid).ok());
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
        let retained_output = Arc::new(OutputRetention::default());
        let (ended, _) = tokio::sync::watch::channel(false);
        let stale_turn = environment
            .get(stale_turn::FLAG)
            .is_some_and(|value| value == "1")
            .then(|| Arc::new(Mutex::new(None)));
        let observation = (recovery_enabled || stale_turn.is_some())
            .then(|| Arc::new(Mutex::new(observation::NativeObservation::default())));
        spawn_writer(stdin, input_rx);
        let reader = spawn_reader(
            stdout,
            output.clone(),
            Arc::clone(&pending),
            Arc::clone(&retained_output),
            observation.clone(),
            stale_turn.clone(),
        );
        observation::spawn_diagnostics(stderr);
        observation::spawn_reaper(child, managed, ended.clone());
        tracing::info!(agent = %launch.agent_id.as_str(), program = %launch.program, "agent bridge started");
        Ok(Self {
            input,
            output,
            pending,
            retained_output,
            output_mode: AtomicU8::new(OUTPUT_UNSELECTED),
            reader,
            ended,
            writer_session_root,
            observation,
            capacity_recovery: Mutex::new(recovery::LeaseState::default()),
            stale_turn,
        })
    }

    #[cfg(test)]
    pub(crate) async fn send(&self, frame: Vec<u8>) -> bool {
        self.send_input(frame, None).await
    }

    pub(crate) async fn send_owned(
        &self,
        frame: Vec<u8>,
        authority: crate::manager_ownership::OwnerSession,
    ) -> bool {
        self.send_input(frame, Some(authority)).await
    }

    async fn send_input(
        &self,
        frame: Vec<u8>,
        authority: Option<crate::manager_ownership::OwnerSession>,
    ) -> bool {
        if frame.len() > MAX_FRAME_BYTES || stale_turn::is_untrusted_stop(&frame) {
            return false;
        }
        if let Some(authority) = &authority {
            if authority.permit().await.is_err() {
                return false;
            }
        }
        if let Some(observation) = &self.observation {
            observation.lock().await.input(&frame);
        }
        if self
            .input
            .send(BridgeInput {
                frame,
                authority,
                stop: None,
            })
            .await
            .is_err()
        {
            if let Some(observation) = &self.observation {
                observation.lock().await.invalidate();
            }
            return false;
        }
        true
    }

    /// Legacy recording has no durable cursor. Never replay that history into a
    /// later reliable attachment, or let a downgrade bypass an established cursor.
    pub(crate) fn select_output_acknowledgement(&self, requested: bool) -> Result<bool, ()> {
        let selected = if requested {
            OUTPUT_ACKNOWLEDGED
        } else {
            OUTPUT_LEGACY
        };
        let mode = match self.output_mode.compare_exchange(
            OUTPUT_UNSELECTED,
            selected,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => selected,
            Err(mode) => mode,
        };
        match mode {
            OUTPUT_LEGACY => {
                self.retained_output.select_legacy();
                Ok(false)
            }
            OUTPUT_ACKNOWLEDGED if requested => Ok(true),
            _ => Err(()),
        }
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Outbound> {
        self.output.subscribe()
    }

    pub(crate) async fn backlog(&self) -> Vec<Outbound> {
        self.pending.lock().await.drain()
    }

    pub(crate) fn is_ended(&self) -> bool {
        *self.ended.subscribe().borrow()
    }

    pub(crate) fn writer_session_root(&self) -> Option<i32> {
        self.writer_session_root
    }

    pub(crate) async fn ended(&self) {
        let mut ended = self.ended.subscribe();
        if *ended.borrow() {
            return;
        }
        let _ = ended.changed().await;
    }

    pub(crate) async fn terminate(&self) -> Result<(), String> {
        terminate_process_session(self.writer_session_root, self).await
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Attached readers keep the Bridge alive to drain final output after
        // process exit. Once the supervisor and every attachment release it,
        // a full journal has no possible consumer and its waiter must end.
        self.reader.abort();
    }
}

fn spawn_reader(
    stdout: tokio::process::ChildStdout,
    output: tokio::sync::broadcast::Sender<Outbound>,
    pending: Arc<Mutex<Pending>>,
    retained_output: Arc<OutputRetention>,
    observation: Option<Arc<Mutex<observation::NativeObservation>>>,
    stale_turn: Option<Arc<Mutex<Option<stale_turn::Stop>>>>,
) -> tokio::task::AbortHandle {
    let reader = tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        let mut chunk = [0_u8; 8 * 1024];
        let mut frame = Vec::new();
        let mut discarding = false;
        loop {
            let read = match stdout.read(&mut chunk).await {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    tracing::warn!(%error, "agent bridge output failed");
                    retained_output.gap().await;
                    break;
                }
            };
            for byte in &chunk[..read] {
                if *byte == b'\n' {
                    if discarding {
                        retained_output.gap().await;
                        pending.lock().await.gap();
                    } else {
                        publish(
                            std::mem::take(&mut frame),
                            &output,
                            &pending,
                            &retained_output,
                            &observation,
                            stale_turn.as_deref(),
                        )
                        .await;
                    }
                    frame.clear();
                    discarding = false;
                } else if !discarding {
                    if frame.len() == MAX_FRAME_BYTES {
                        frame.clear();
                        discarding = true;
                    } else {
                        frame.push(*byte);
                    }
                }
            }
        }
        if discarding {
            retained_output.gap().await;
            pending.lock().await.gap();
        } else if !frame.is_empty() {
            if stale_turn.is_some() {
                // A partially delivered fence cannot establish the end of output.
                retained_output.gap().await;
            } else {
                publish(
                    frame,
                    &output,
                    &pending,
                    &retained_output,
                    &observation,
                    None,
                )
                .await;
            }
        }
        retained_output.finish().await;
    });
    reader.abort_handle()
}

async fn publish(
    frame: Vec<u8>,
    output: &tokio::sync::broadcast::Sender<Outbound>,
    pending: &Arc<Mutex<Pending>>,
    retained_output: &OutputRetention,
    observation: &Option<Arc<Mutex<observation::NativeObservation>>>,
    stale_turn: Option<&Mutex<Option<stale_turn::Stop>>>,
) {
    if let (Some(stale_turn), Some(observation)) = (stale_turn, observation) {
        if stale_turn::observe_fence(stale_turn, retained_output, observation, &frame).await {
            return;
        }
    }
    if let Some(observation) = observation {
        // A full output window can await an ACK. Mark the in-flight boundary
        // instead of holding a state lock across backpressure.
        observation.lock().await.pending_output = true;
        retained_output.retain(&frame).await;
        let mut observed = observation.lock().await;
        observed.output(&frame);
        observed.pending_output = false;
    } else {
        retained_output.retain(&frame).await;
    }
    if output.send(Outbound::Frame(frame.clone())).is_err() {
        pending.lock().await.push(frame);
    }
}

#[cfg(unix)]
async fn terminate_process_session(session: Option<i32>, _bridge: &Bridge) -> Result<(), String> {
    let Some(session) = session else {
        return Err("agent process has no process-session identifier".into());
    };
    signal_session(session, libc::SIGTERM)?;
    if wait_until_gone(session).await {
        return Ok(());
    }
    signal_session(session, libc::SIGKILL)?;
    if wait_until_gone(session).await {
        Ok(())
    } else {
        Err("agent process session did not terminate".into())
    }
}

#[cfg(unix)]
async fn wait_until_gone(session: i32) -> bool {
    tokio::time::timeout(CLOSE_TIMEOUT, async {
        while session_exists(session) {
            tokio::time::sleep(CLOSE_POLL).await;
        }
    })
    .await
    .is_ok()
}

#[cfg(unix)]
fn signal_session(session: i32, signal: i32) -> Result<(), String> {
    if unsafe { libc::kill(-session, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!("signal agent process session {session}: {error}"))
    }
}

#[cfg(unix)]
fn session_exists(session: i32) -> bool {
    if unsafe { libc::kill(-session, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
async fn terminate_process_session(_session: Option<i32>, bridge: &Bridge) -> Result<(), String> {
    if bridge.is_ended() {
        return Ok(());
    }
    tokio::time::timeout(CLOSE_TIMEOUT, bridge.ended())
        .await
        .map_err(|_| "agent process did not terminate".to_string())
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
