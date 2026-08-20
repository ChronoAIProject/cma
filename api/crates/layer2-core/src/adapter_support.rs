//! A runtime that provisions nothing, for tests about everything else.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit, and re-exported from there so a test still has one place to import
//! from. It is the provider's side of the contract: what it was asked to do,
//! what it currently has running, and every way it can refuse.

use std::collections::HashMap;

use layer3_adapter::{
    AdapterError, KernelAdapter, RuntimeHandle, RuntimeListing, RuntimeSpec, TerminalChannel,
};
use tokio::sync::{mpsc, Mutex};

/// An adapter that provisions nothing and records what it was asked to tear
/// down, so teardown can be asserted without a sandbox.
pub(crate) struct FakeAdapter {
    pub(crate) terminated: Mutex<Vec<String>>,
    /// When set, provisioning fails. Models a provider that cannot give us a
    /// runtime at all.
    pub(crate) refuse_provision: bool,
    /// How many attempts fail before one succeeds. Models the transient
    /// failure that retrying exists for.
    pub(crate) fail_first: std::sync::atomic::AtomicUsize,
    /// When set, provisioning succeeds and attaching does not. This is the
    /// case that leaks: a runtime exists that nothing has a handle to yet.
    pub(crate) refuse_attach: bool,
    /// Kept alive so the relay's output channel does not close immediately,
    /// which would make a live session look dead.
    pub(crate) senders: Mutex<Vec<mpsc::Sender<Vec<u8>>>>,
    /// What the provider currently has running: handle to the session it was
    /// provisioned for. This is the provider's own view, which is the thing
    /// reconciliation compares the record against.
    pub(crate) running: Mutex<HashMap<String, Option<String>>>,
    /// What each `open_conversation` was asked to rejoin, in order.
    ///
    /// `None` for a conversation being started; a name for one being taken
    /// back over.
    pub(crate) rejoined: Mutex<Vec<Option<String>>>,
    /// When set, enumeration fails. Models a provider that cannot be asked,
    /// which must never be read as a provider running nothing.
    pub(crate) refuse_list: bool,
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
    /// What each runtime says its agent's conversation is.
    pub(crate) conversations: Mutex<HashMap<String, Vec<u8>>>,
    /// Conversations handed to newly provisioned runtimes, in order.
    pub(crate) handovers: Mutex<Vec<Vec<u8>>>,
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
    /// What this runtime says it can do.
    ///
    /// Defaults to the shape of the only kernel that exists today — a terminal
    /// that can be carried onto a replacement — so tests about anything else
    /// keep working without stating it.
    pub(crate) capabilities: layer3_adapter::KernelCapabilities,
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
            fail_first: std::sync::atomic::AtomicUsize::new(0),
            refuse_attach: false,
            senders: Mutex::default(),
            running: Mutex::default(),
            refuse_list: false,
            created_by: None,
            last_spec: Mutex::default(),
            issued: std::sync::atomic::AtomicUsize::new(0),
            provision_calls: std::sync::atomic::AtomicUsize::new(0),
            conversations: Mutex::default(),
            handovers: Mutex::default(),
            refuse_snapshot: false,
            conversation: Mutex::default(),
            rejoined: Mutex::default(),
            credentials: Mutex::default(),
            sign_in_on_open: false,
            capabilities: layer3_adapter::KernelCapabilities {
                conversation: false,
                terminal: true,
                snapshot: true,
                steer: false,
                interrupt: false,
                usage: false,
            },
        }
    }
}

impl FakeAdapter {
    /// Forget a runtime without being told to, as a provider does when a node
    /// is replaced underneath a sandbox.
    pub(crate) async fn lose(&self, handle: &str) {
        self.running.lock().await.remove(handle);
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
        if self.refuse_provision {
            return Err(AdapterError::Unavailable("no capacity".into()));
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
        Ok(RuntimeHandle(handle))
    }

    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        if self.refuse_attach {
            return Err(AdapterError::Unavailable("terminal not listening".into()));
        }
        // A sandbox that is not running cannot be attached to. Without this the
        // fake would let a session reattach to something that no longer exists,
        // which is the failure these tests are about.
        if !self.running.lock().await.contains_key(&handle.0) {
            return Err(AdapterError::Unavailable("no such runtime".into()));
        }
        let (out_tx, out_rx) = mpsc::channel(8);
        let (in_tx, _in_rx) = mpsc::channel(8);
        self.senders.lock().await.push(out_tx);
        // The input receiver is dropped: nothing in these tests reads it, and
        // the service tolerates a closed input channel by design.
        Ok(TerminalChannel {
            output: out_rx,
            input: in_tx,
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

    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        self.terminated.lock().await.push(handle.0.clone());
        self.running.lock().await.remove(&handle.0);
        Ok(())
    }

    async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        if self.refuse_snapshot {
            return Err(AdapterError::Unsupported("cannot capture".into()));
        }
        Ok(self.conversations.lock().await.get(&handle.0).cloned())
    }

    async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        if self.refuse_list {
            return Err(AdapterError::Unsupported("cannot enumerate".into()));
        }
        Ok(self
            .running
            .lock()
            .await
            .iter()
            .map(|(h, s)| RuntimeListing {
                handle: RuntimeHandle(h.clone()),
                session: s.clone(),
                created_by: self.created_by.clone(),
            })
            .collect())
    }
}
