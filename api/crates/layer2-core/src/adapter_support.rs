//! A runtime that provisions nothing, for tests about everything else.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit, and re-exported from there so a test still has one place to import
//! from. It is the provider's side of the contract: what it was asked to do,
//! what it currently has running, and every way it can refuse.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::{
    AdapterError, KernelAdapter, RuntimeGeneration, RuntimeHandle, RuntimeIdentity, RuntimeListing,
    RuntimeLossReason, RuntimeMode, RuntimeSpec, TerminalChannel, TerminalInput,
    TerminalInputSender, TerminalOutput, WorkspaceCheckpointPackage, WorkspaceCheckpointRequest,
};
use tokio::sync::{mpsc, Mutex, Notify, Semaphore};

/// An adapter that provisions nothing and records what it was asked to tear
/// down, so teardown can be asserted without a sandbox.
pub(crate) struct FakeAdapter {
    pub(crate) terminated: Mutex<Vec<String>>,
    /// When set, provisioning fails. Models a provider that cannot give us a
    /// runtime at all.
    pub(crate) refuse_provision: bool,
    /// When set, provisioning receives a typed answer that no room remains.
    pub(crate) refuse_at_capacity: bool,
    /// How many attempts fail before one succeeds. Models the transient
    /// failure that retrying exists for.
    pub(crate) fail_first: std::sync::atomic::AtomicUsize,
    /// When set, provisioning succeeds and attaching does not. This is the
    /// case that leaks: a runtime exists that nothing has a handle to yet.
    pub(crate) refuse_attach: bool,
    /// Terminal attach attempts to fail before returning to normal behavior.
    pub(crate) fail_terminal_attaches: std::sync::atomic::AtomicUsize,
    /// Kept alive so the relay's output channel does not close immediately,
    /// which would make a live session look dead.
    pub(crate) senders: Mutex<Vec<mpsc::Sender<TerminalOutput>>>,
    /// Terminal operations that reached their runtime-link writer.
    pub(crate) terminal_inputs: Arc<Mutex<Vec<(String, TerminalInput)>>>,
    /// Deterministically hold one terminal write inside its delivery receipt.
    pub(crate) pause_next_terminal_input: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) terminal_input_paused: Arc<Notify>,
    pub(crate) terminal_input_resume: Arc<Notify>,
    /// What the provider currently has running: handle to the session it was
    /// provisioned for. This is the provider's own view, which is the thing
    /// reconciliation compares the record against.
    pub(crate) running: Mutex<HashMap<String, Option<String>>>,
    /// Provider-visible generation recorded for runtimes provisioned through
    /// the generation-aware contract. Directly seeded legacy fixtures omit it.
    pub(crate) generations: Mutex<HashMap<String, std::num::NonZeroU64>>,
    /// What each `open_conversation` was asked to rejoin, in order.
    ///
    /// `None` for a conversation being started; a name for one being taken
    /// back over.
    pub(crate) rejoined: Mutex<Vec<Option<String>>>,
    /// When set, enumeration fails. Models a provider that cannot be asked,
    /// which must never be read as a provider running nothing.
    pub(crate) refuse_list: bool,
    /// Handles the provider still lists but reports as no longer serving.
    ///
    /// The case that matters: a sandbox whose container exited is still a
    /// sandbox. A test that merely removes the handle is describing a
    /// different failure — one the census already handled.
    pub(crate) stopped: Mutex<std::collections::HashSet<String>>,
    /// Why a stopped runtime stopped, where the provider named a reason.
    pub(crate) stopped_because: Mutex<HashMap<String, RuntimeLossReason>>,
    /// The identity this provider reports on everything it is running.
    ///
    /// Set to model a provider shared with another control plane, whose
    /// runtimes must never be reclaimed by this one.
    pub(crate) created_by: Option<String>,
    /// The last thing this adapter was asked to provision.
    ///
    /// What the core asked for is the only place the environment a session was
    /// resolved to can be observed: it is passed to the provider and never
    /// stored, because the values in it are the caller's to keep.
    pub(crate) last_spec: Mutex<Option<RuntimeSpec>>,
    /// Handles issued so far. Monotonic rather than a count of what is running,
    /// so a replacement sandbox is never named after the one it replaced.
    pub(crate) issued: std::sync::atomic::AtomicUsize,
    /// Times provisioning was asked for at all, successful or not.
    ///
    /// Distinct from `issued`, which counts only the attempts that produced a
    /// runtime. This is what makes "the core retried" and "the core accepted
    /// the answer" tellable apart.
    pub(crate) provision_calls: std::sync::atomic::AtomicUsize,
    /// Pauses the next provision call after it enters the adapter.
    ///
    /// Armed only by concurrency tests after the original runtime is live. A
    /// coordinated rotation calls provision after marking that entry rotating,
    /// making this the deterministic outgoing-retained, pre-replacement boundary
    /// those tests need.
    pub(crate) pause_next_provision: std::sync::atomic::AtomicBool,
    pub(crate) provision_paused: Notify,
    pub(crate) provision_resume: Notify,
    /// What each runtime says its agent's conversation is.
    pub(crate) conversations: Mutex<HashMap<String, Vec<u8>>>,
    /// Whether this adapter would accept an in-place plane switch.
    pub(crate) accept_plane_switch: bool,
    /// Every in-place plane switch the core attempted.
    pub(crate) plane_switches: Mutex<Vec<(String, RuntimeMode)>>,
    /// Provider teardown calls to fail before changing any provider state.
    pub(crate) fail_terminations: std::sync::atomic::AtomicUsize,
    /// Conversations handed to newly provisioned runtimes, in order.
    pub(crate) handovers: Mutex<Vec<Vec<u8>>>,
    /// Stable claim-bound handover objects shared by restarted test services.
    pub(crate) stored_handovers: Mutex<HashMap<String, crate::store::HandoverSnapshot>>,
    /// Handover object writes to fail before changing stored state.
    pub(crate) fail_handover_puts: std::sync::atomic::AtomicUsize,
    /// Handover object reads to fail before returning stored state.
    pub(crate) fail_handover_gets: std::sync::atomic::AtomicUsize,
    /// Snapshot calls completed, so concurrency tests can wait for rehearsal.
    pub(crate) snapshot_calls: std::sync::atomic::AtomicUsize,
    /// One-based snapshot call to pause, or zero when none is armed.
    pub(crate) pause_snapshot_call: std::sync::atomic::AtomicUsize,
    pub(crate) snapshot_paused: Notify,
    pub(crate) snapshot_resume: Notify,
    /// When set, the runtime cannot hand its conversation over.
    pub(crate) refuse_snapshot: bool,
    /// The conversation this runtime is holding, once one has been opened.
    ///
    /// Held so a test can be the kernel: push an update or a request down, and
    /// read back what the core sent up.
    pub(crate) conversation: Mutex<Option<crate::conversation_support::FakeConversation>>,
    /// Credentials this runtime was handed, in order.
    ///
    /// The credential no longer travels as terminal input, so the only place
    /// its delivery is observable is the adapter that was asked to deliver it.
    pub(crate) credentials: Mutex<Vec<String>>,
    /// When set, the kernel asks its owner to sign in from inside the open.
    ///
    /// Which is what a real one does on an own-account profile, and is the
    /// only update that arrives before the session has a live entry — the
    /// case #351 was about. Raised before the channel is returned, so the
    /// core meets it at the moment it genuinely has nowhere to put it.
    pub(crate) sign_in_on_open: bool,
    /// Ordered updates emitted before `open_conversation` returns.
    ///
    /// Used to exercise the production opening receiver rather than calling a
    /// fold helper directly. The vector is drained once, preserving its order.
    pub(crate) opening_updates: Mutex<Vec<layer3_adapter::ConversationUpdate>>,
    /// Refuse the conversation after emitting `opening_updates`.
    pub(crate) refuse_conversation_open: std::sync::atomic::AtomicBool,
    /// A permission request emitted before `open_conversation` returns.
    ///
    /// Used to model a blocked request replayed on the replacement link while
    /// readoption is still between its first history read and entry install.
    pub(crate) permission_on_open: Mutex<Option<layer3_adapter::PermissionRequest>>,
    /// Hold a replacement conversation open at the point after its durable
    /// history was selected but before its entry can be installed.
    pub(crate) hold_conversation_open: std::sync::atomic::AtomicBool,
    pub(crate) conversation_open_started: Semaphore,
    pub(crate) continue_conversation_open: Semaphore,
    /// What this runtime says it can do.
    ///
    /// Defaults to the shape of the only kernel that exists today — a terminal
    /// that can be carried onto a replacement — so tests about anything else
    /// keep working without stating it.
    pub(crate) capabilities: layer3_adapter::KernelCapabilities,
    /// The receiver a resource watch was asked to feed.
    ///
    /// Held so a test can be the session and push readings into the core.
    pub(crate) resource_sink: Mutex<Option<mpsc::Sender<layer3_adapter::ResourceSample>>>,
    /// When set, a resource watch reports that this session cannot provide one.
    pub(crate) refuse_watch: bool,
    /// When set, a resource watch fails transiently.
    pub(crate) fail_watch: bool,
    /// Resource watch attempts, including refusals and transient failures.
    pub(crate) resource_watch_calls: std::sync::atomic::AtomicUsize,
    /// A sealed workspace capture returned to Layer 2 when a test configures
    /// this runtime to support the v2 checkpoint capability.
    pub(crate) checkpoint: Mutex<Option<WorkspaceCheckpointPackage>>,
    pub(crate) checkpoint_calls: std::sync::atomic::AtomicUsize,
    pub(crate) hold_checkpoint: std::sync::atomic::AtomicBool,
    pub(crate) checkpoint_started: Semaphore,
    pub(crate) continue_checkpoint: Semaphore,
}

impl Default for FakeAdapter {
    /// A runtime shaped like the only real one: a terminal that can be carried
    /// onto a replacement.
    ///
    /// Spelled out rather than derived, because a derived default would
    /// advertise nothing and every session in these tests would be refused
    /// before it started — which would test the refusal and nothing else.
    fn default() -> Self {
        Self {
            terminated: Mutex::default(),
            refuse_provision: false,
            refuse_at_capacity: false,
            fail_first: std::sync::atomic::AtomicUsize::new(0),
            refuse_attach: false,
            fail_terminal_attaches: std::sync::atomic::AtomicUsize::new(0),
            senders: Mutex::default(),
            terminal_inputs: Arc::new(Mutex::default()),
            pause_next_terminal_input: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            terminal_input_paused: Arc::new(Notify::new()),
            terminal_input_resume: Arc::new(Notify::new()),
            running: Mutex::default(),
            generations: Mutex::default(),
            refuse_list: false,
            stopped: Mutex::default(),
            stopped_because: Mutex::default(),
            created_by: None,
            last_spec: Mutex::default(),
            issued: std::sync::atomic::AtomicUsize::new(0),
            provision_calls: std::sync::atomic::AtomicUsize::new(0),
            pause_next_provision: std::sync::atomic::AtomicBool::new(false),
            provision_paused: Notify::new(),
            provision_resume: Notify::new(),
            conversations: Mutex::default(),
            accept_plane_switch: false,
            plane_switches: Mutex::default(),
            fail_terminations: std::sync::atomic::AtomicUsize::new(0),
            handovers: Mutex::default(),
            stored_handovers: Mutex::default(),
            fail_handover_puts: std::sync::atomic::AtomicUsize::new(0),
            fail_handover_gets: std::sync::atomic::AtomicUsize::new(0),
            snapshot_calls: std::sync::atomic::AtomicUsize::new(0),
            pause_snapshot_call: std::sync::atomic::AtomicUsize::new(0),
            snapshot_paused: Notify::new(),
            snapshot_resume: Notify::new(),
            refuse_snapshot: false,
            conversation: Mutex::default(),
            rejoined: Mutex::default(),
            credentials: Mutex::default(),
            sign_in_on_open: false,
            opening_updates: Mutex::default(),
            refuse_conversation_open: std::sync::atomic::AtomicBool::new(false),
            permission_on_open: Mutex::default(),
            hold_conversation_open: std::sync::atomic::AtomicBool::new(false),
            conversation_open_started: Semaphore::new(0),
            continue_conversation_open: Semaphore::new(0),
            capabilities: layer3_adapter::KernelCapabilities {
                conversation: false,
                terminal: true,
                snapshot: true,
                steer: false,
                interrupt: false,
                usage: false,
                resources: false,
            },
            resource_sink: Mutex::default(),
            refuse_watch: false,
            fail_watch: false,
            resource_watch_calls: std::sync::atomic::AtomicUsize::new(0),
            checkpoint: Mutex::default(),
            checkpoint_calls: std::sync::atomic::AtomicUsize::new(0),
            hold_checkpoint: std::sync::atomic::AtomicBool::new(false),
            checkpoint_started: Semaphore::new(0),
            continue_checkpoint: Semaphore::new(0),
        }
    }
}

impl FakeAdapter {
    pub(crate) fn pause_next_terminal_input(&self) {
        assert!(
            !self
                .pause_next_terminal_input
                .swap(true, std::sync::atomic::Ordering::SeqCst),
            "a terminal input pause is already armed"
        );
    }

    pub(crate) async fn wait_until_terminal_input_paused(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.terminal_input_paused.notified(),
        )
        .await
        .expect("terminal input reached the runtime-link writer");
    }

    pub(crate) fn resume_terminal_input(&self) {
        self.terminal_input_resume.notify_one();
    }

    pub(crate) fn pause_next_provision(&self) {
        assert!(
            !self
                .pause_next_provision
                .swap(true, std::sync::atomic::Ordering::SeqCst),
            "a provision pause is already armed"
        );
    }

    pub(crate) async fn wait_until_provision_paused(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.provision_paused.notified(),
        )
        .await
        .expect("replacement provisioning reached the adapter");
    }

    pub(crate) fn resume_provision(&self) {
        self.provision_resume.notify_one();
    }

    pub(crate) fn fail_terminations(&self, count: usize) {
        self.fail_terminations
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn fail_terminal_attaches(&self, count: usize) {
        self.fail_terminal_attaches
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn fail_handover_puts(&self, count: usize) {
        self.fail_handover_puts
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn fail_handover_gets(&self, count: usize) {
        self.fail_handover_gets
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    fn terminal_attach_fails(&self) -> bool {
        self.fail_terminal_attaches
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
    }

    fn termination_fails(&self) -> bool {
        self.fail_terminations
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
    }

    /// Forget a runtime without being told to, as a provider does when a node
    /// is replaced underneath a sandbox.
    pub(crate) async fn lose(&self, handle: &str) {
        self.running.lock().await.remove(handle);
    }

    /// The sandbox stops serving but is still listed.
    ///
    /// Distinct from `lose`, and the distinction is the whole point: a
    /// container that exits leaves its sandbox behind, so the provider keeps
    /// reporting it. A test that removes the handle describes a sandbox that
    /// vanished, which reconciliation already handled — this describes the one
    /// that killed a session on production while looking perfectly healthy.
    pub(crate) async fn stop_serving(&self, handle: &str) {
        self.stopped.lock().await.insert(handle.to_string());
    }

    /// The sandbox stops serving, and the provider says why.
    ///
    /// Separate from `stop_serving` so the two facts stay separable: a
    /// provider that stops a sandbox and names nothing is a real case.
    pub(crate) async fn stop_serving_because(&self, handle: &str, why: RuntimeLossReason) {
        self.stop_serving(handle).await;
        self.stopped_because
            .lock()
            .await
            .insert(handle.to_string(), why);
    }

    pub(crate) async fn running_count(&self) -> usize {
        self.running.lock().await.len()
    }

    /// Give a runtime a conversation to hand over when asked.
    pub(crate) async fn set_conversation(&self, handle: &str, bytes: &[u8]) {
        self.conversations
            .lock()
            .await
            .insert(handle.to_string(), bytes.to_vec());
    }

    pub(crate) fn pause_snapshot_on_call(&self, call: usize) {
        assert!(call > 0, "snapshot calls are numbered from one");
        assert_eq!(
            self.pause_snapshot_call
                .swap(call, std::sync::atomic::Ordering::SeqCst),
            0,
            "a snapshot pause is already armed"
        );
    }

    pub(crate) async fn wait_until_snapshot_paused(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.snapshot_paused.notified(),
        )
        .await
        .expect("rotation reached the selected snapshot");
    }

    pub(crate) fn resume_snapshot(&self) {
        self.snapshot_resume.notify_one();
    }
}

#[async_trait::async_trait]
impl crate::store::HandoverStore for FakeAdapter {
    async fn put(
        &self,
        id: &crate::ids::SessionId,
        snapshot: crate::store::HandoverSnapshot,
    ) -> Result<String, crate::store::StoreError> {
        if self
            .fail_handover_puts
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            return Err(crate::store::StoreError::Unavailable(
                "handover object write unavailable".into(),
            ));
        }
        let reference = format!("sessions/{}/handover.snapshot", id.0);
        self.stored_handovers
            .lock()
            .await
            .insert(reference.clone(), snapshot);
        Ok(reference)
    }

    async fn get_for(
        &self,
        id: &crate::ids::SessionId,
        reference: &str,
    ) -> Result<Option<crate::store::HandoverSnapshot>, crate::store::StoreError> {
        if self
            .fail_handover_gets
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            return Err(crate::store::StoreError::Unavailable(
                "handover object read unavailable".into(),
            ));
        }
        let expected = format!("sessions/{}/handover.snapshot", id.0);
        if reference != expected {
            return Err(crate::store::StoreError::Malformed(
                "handover reference belongs to another session".into(),
            ));
        }
        Ok(self.stored_handovers.lock().await.get(reference).cloned())
    }

    async fn delete_for(&self, id: &crate::ids::SessionId) -> Result<(), crate::store::StoreError> {
        let reference = format!("sessions/{}/handover.snapshot", id.0);
        self.stored_handovers.lock().await.remove(&reference);
        Ok(())
    }
}

#[async_trait::async_trait]
impl KernelAdapter for FakeAdapter {
    fn kernel_name(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> layer3_adapter::KernelCapabilities {
        self.capabilities
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        self.provision_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self
            .pause_next_provision
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.provision_paused.notify_one();
            self.provision_resume.notified().await;
        }
        if self.refuse_provision {
            return Err(AdapterError::Unavailable("no capacity".into()));
        }
        if self.refuse_at_capacity {
            return Err(AdapterError::AtCapacity(
                "the adapter's declared capacity signal".into(),
            ));
        }
        // No kernel authenticates against the owner's own vendor account yet,
        // so every adapter refuses it. Modelled here because what the core has
        // to do with such a refusal — record it where the owner can read it —
        // is the core's own behaviour, not the adapter's.
        if spec.options.own_credential {
            return Err(AdapterError::Unsupported(
                "this kernel does not support the owner's own credential yet".into(),
            ));
        }
        // Fail the first few attempts, then succeed, as a provider under load
        // does.
        if self
            .fail_first
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |n| if n > 0 { Some(n - 1) } else { None },
            )
            .is_ok()
        {
            return Err(AdapterError::Unavailable("provider busy".into()));
        }
        // Unique per provisioning, not per session: a session given a second
        // runtime must be distinguishable from one still on its first, which is
        // the whole question reconciliation answers.
        let n = self
            .issued
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        *self.last_spec.lock().await = Some(spec.clone());
        if let Some(carried) = spec.restore_from.clone() {
            self.handovers.lock().await.push(carried);
        }
        let mut running = self.running.lock().await;
        let handle = format!("runtime-{n}-for-{}", spec.session_id);
        running.insert(handle.clone(), Some(spec.session_id.clone()));
        if let Some(generation) = spec.generation {
            self.generations
                .lock()
                .await
                .insert(handle.clone(), generation);
        }
        Ok(RuntimeHandle(handle))
    }

    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        if self.refuse_attach || self.terminal_attach_fails() {
            return Err(AdapterError::Unavailable("terminal not listening".into()));
        }
        // A sandbox that is not running cannot be attached to. Without this the
        // fake would let a session reattach to something that no longer exists,
        // which is the failure these tests are about.
        if !self.running.lock().await.contains_key(&handle.0) {
            return Err(AdapterError::Unavailable("no such runtime".into()));
        }
        let (out_tx, out_rx) = mpsc::channel(8);
        self.senders.lock().await.push(out_tx);
        let runtime = handle.0.clone();
        let terminal_inputs = Arc::clone(&self.terminal_inputs);
        let pause_next = Arc::clone(&self.pause_next_terminal_input);
        let paused = Arc::clone(&self.terminal_input_paused);
        let resume = Arc::clone(&self.terminal_input_resume);
        let input = TerminalInputSender::spawn(8, move |input| {
            let runtime = runtime.clone();
            let terminal_inputs = Arc::clone(&terminal_inputs);
            let pause_next = Arc::clone(&pause_next);
            let paused = Arc::clone(&paused);
            let resume = Arc::clone(&resume);
            async move {
                if pause_next.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    paused.notify_one();
                    resume.notified().await;
                }
                terminal_inputs.lock().await.push((runtime, input));
                Ok(())
            }
        });
        Ok(TerminalChannel {
            output: out_rx,
            input,
        })
    }

    async fn open_conversation(
        &self,
        _handle: &RuntimeHandle,
        options: &layer3_adapter::RuntimeOptions,
        updates: tokio::sync::mpsc::Sender<layer3_adapter::ConversationUpdate>,
    ) -> Result<layer3_adapter::ConversationChannel, AdapterError> {
        if !self.capabilities.conversation {
            return Err(AdapterError::Unsupported(
                "this kernel does not hold a conversation".into(),
            ));
        }
        // Recorded so a test can ask what the core said, which is the whole
        // question when a control plane takes a live runtime back over:
        // rejoining what is running, or opening a second one beside it.
        self.rejoined
            .lock()
            .await
            .push(options.resume_conversation.clone());
        if self
            .hold_conversation_open
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.conversation_open_started.add_permits(1);
            self.continue_conversation_open
                .acquire()
                .await
                .expect("the test adapter remains alive")
                .forget();
        }
        if self.sign_in_on_open {
            // Before the channel is returned, because that is when a real
            // kernel raises it — and the core has not inserted the session's
            // entry yet, which is the whole of #351.
            updates
                .send(layer3_adapter::ConversationUpdate::SignInRequired {
                    url: "https://auth.example.test/device".to_string(),
                    code: Some("WDJB-MJHT".to_string()),
                    message: Some("Sign in to continue".to_string()),
                })
                .await
                .expect("the core is listening before it opens");
            /*
             * And then waits, which is the half that matters. A real kernel
             * sits here for as long as its owner takes, so the core folds the
             * request while this call is still outstanding — before it has an
             * entry to append against. Returning straight away instead let the
             * entry exist by the time the folder got to it, which is a session
             * that signs in instantly and tests nothing.
             */
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let opening_updates = std::mem::take(&mut *self.opening_updates.lock().await);
        for update in opening_updates {
            updates
                .send(update)
                .await
                .expect("the core drains updates while the conversation opens");
        }
        if let Some(request) = self.permission_on_open.lock().await.take() {
            updates
                .send(layer3_adapter::ConversationUpdate::Asked(request))
                .await
                .expect("the core is listening before it opens");
            // Let the folder reach its no-entry path before the channel is
            // returned and readoption advances to installation.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if self
            .refuse_conversation_open
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AdapterError::Rejected(
                "the test conversation refused to open".into(),
            ));
        }
        let (channel, held) = crate::conversation_support::FakeConversation::open(updates);
        *self.conversation.lock().await = Some(held);
        Ok(channel)
    }

    async fn refresh_credential(
        &self,
        handle: &RuntimeHandle,
        credential: &str,
    ) -> Result<(), AdapterError> {
        if !self.running.lock().await.contains_key(&handle.0) {
            return Err(AdapterError::Unavailable("no such runtime".into()));
        }
        self.credentials.lock().await.push(credential.to_string());
        Ok(())
    }

    async fn watch_resources(
        &self,
        _handle: &RuntimeHandle,
        readings: mpsc::Sender<layer3_adapter::ResourceSample>,
    ) -> Result<(), AdapterError> {
        self.resource_watch_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.refuse_watch {
            return Err(AdapterError::Unsupported(
                "resource readings are unavailable".into(),
            ));
        }
        if self.fail_watch {
            return Err(AdapterError::Unavailable(
                "resource reading link failed".into(),
            ));
        }
        *self.resource_sink.lock().await = Some(readings);
        Ok(())
    }

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        if self.termination_fails() {
            return Err(AdapterError::Unavailable(
                "the selected runtime teardown is unavailable".into(),
            ));
        }
        self.terminated.lock().await.push(handle.0.clone());
        self.running.lock().await.remove(&handle.0);
        self.generations.lock().await.remove(&handle.0);
        Ok(())
    }

    async fn terminate_if_identity(&self, identity: &RuntimeIdentity) -> Result<(), AdapterError> {
        if self.termination_fails() {
            return Err(AdapterError::Unavailable(
                "the selected runtime teardown is unavailable".into(),
            ));
        }
        let mut terminated = self.terminated.lock().await;
        let mut running = self.running.lock().await;
        let mut generations = self.generations.lock().await;
        let owner = running.get(&identity.handle.0);
        let generation = generations.get(&identity.handle.0).copied();
        if owner.is_none() && generation.is_none() {
            return Ok(());
        }
        let owner_matches = owner.and_then(Option::as_deref) == Some(identity.session.as_str());
        if !owner_matches || generation != identity.generation {
            return Err(AdapterError::Rejected(
                "the runtime handle belongs to another session or generation".into(),
            ));
        }
        terminated.push(identity.handle.0.clone());
        running.remove(&identity.handle.0);
        generations.remove(&identity.handle.0);
        Ok(())
    }

    async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        if self.refuse_snapshot {
            return Err(AdapterError::Unsupported("cannot capture".into()));
        }
        let snapshot = self.conversations.lock().await.get(&handle.0).cloned();
        let call = self
            .snapshot_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if self
            .pause_snapshot_call
            .compare_exchange(
                call,
                0,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            self.snapshot_paused.notify_one();
            self.snapshot_resume.notified().await;
        }
        Ok(snapshot)
    }

    async fn checkpoint_workspace(
        &self,
        handle: &RuntimeHandle,
        _request: &WorkspaceCheckpointRequest,
    ) -> Result<WorkspaceCheckpointPackage, AdapterError> {
        self.checkpoint_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.running.lock().await.contains_key(&handle.0) {
            return Err(AdapterError::Unavailable("no such runtime".into()));
        }
        if self
            .hold_checkpoint
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.checkpoint_started.add_permits(1);
            self.continue_checkpoint
                .acquire()
                .await
                .expect("the test adapter remains alive")
                .forget();
        }
        self.checkpoint.lock().await.clone().ok_or_else(|| {
            AdapterError::Unsupported("this test runtime cannot checkpoint its workspace".into())
        })
    }

    async fn switch_plane(
        &self,
        handle: &RuntimeHandle,
        mode: RuntimeMode,
    ) -> Result<(), AdapterError> {
        self.plane_switches
            .lock()
            .await
            .push((handle.0.clone(), mode));
        if self.accept_plane_switch {
            Ok(())
        } else {
            Err(AdapterError::Unsupported(
                "this test runtime cannot switch plane in place".into(),
            ))
        }
    }

    async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        if self.refuse_list {
            return Err(AdapterError::Unsupported("cannot enumerate".into()));
        }
        // Both locks taken before the map, because the closure cannot await.
        let stopped = self.stopped.lock().await.clone();
        let stopped_because = self.stopped_because.lock().await.clone();
        let generations = self.generations.lock().await.clone();
        Ok(self
            .running
            .lock()
            .await
            .iter()
            .map(|(h, s)| RuntimeListing {
                handle: RuntimeHandle(h.clone()),
                session: s.clone(),
                generation: generations
                    .get(h)
                    .copied()
                    .map_or(RuntimeGeneration::Legacy, RuntimeGeneration::Recorded),
                created_by: self.created_by.clone(),
                // Serving unless this fake was told otherwise, so every
                // existing test keeps describing a healthy sandbox and only a
                // test that asks for a dead one gets one.
                serving: !stopped.contains(h),
                lost_because: stopped_because.get(h).copied(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stopped_fake_runtime_can_carry_the_providers_verdict() {
        let adapter = FakeAdapter::default();
        adapter
            .running
            .lock()
            .await
            .insert("sbx-dead".to_string(), Some("sesn_x".to_string()));
        adapter
            .stop_serving_because("sbx-dead", RuntimeLossReason::Displaced)
            .await;

        let listed = adapter.list_runtimes().await.expect("the fake can list");
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].serving);
        assert_eq!(listed[0].lost_because, Some(RuntimeLossReason::Displaced));
    }
}
