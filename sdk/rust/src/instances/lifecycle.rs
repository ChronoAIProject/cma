use serde::Deserialize;

string_enum! { /// Reason a new lifecycle transition cannot currently begin.
    pub enum SandboxLifecycleBlocker {
        NotActive => "not_active", NotPaused => "not_paused",
        CheckpointInProgress => "checkpoint_in_progress", WorkInProgress => "work_in_progress"
    }
}
string_enum! { /// Recorded stage of an active Sandbox checkpoint.
    pub enum SandboxCheckpointPhase { Capture => "capture", ResumeWriters => "resume_writers" }
}
string_enum! { /// Recorded pause stage.
    pub enum SandboxPausePhase {
        CaptureAndPublishCheckpoint => "capture_and_publish_checkpoint",
        PauseAgents => "pause_agents", TeardownRuntime => "teardown_runtime", PublishPaused => "publish_paused", ResumeWriters => "resume_writers"
    }
}
string_enum! { /// Recorded revival stage.
    pub enum SandboxRevivePhase {
        CheckpointVerification => "checkpoint_verification", RuntimeProvisioning => "runtime_provisioning",
        CredentialDelivery => "credential_delivery", HomeRestore => "home_restore", ConversationReopen => "conversation_reopen",
        QueuedMessageDelivery => "queued_message_delivery", PublishActive => "publish_active"
    }
}
string_enum! { /// Recorded runtime image rotation stage.
    pub enum SandboxImageRotationPhase {
        CaptureAndPublishCheckpoint => "capture_and_publish_checkpoint", PauseAgents => "pause_agents", TeardownRuntime => "teardown_runtime",
        CheckpointVerification => "checkpoint_verification", RuntimeProvisioning => "runtime_provisioning",
        CredentialDelivery => "credential_delivery", HomeRestore => "home_restore", ConversationReopen => "conversation_reopen",
        QueuedMessageDelivery => "queued_message_delivery", PublishActive => "publish_active"
    }
}

/// Eligibility to begin a new transition, independent of idempotent command replay.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxLifecycleEligibility {
    /// Whether the current record admits a new transition.
    pub allowed: bool,
    /// Safe explanation when admission is unavailable.
    pub reason: Option<SandboxLifecycleBlocker>,
}

/// Checkpoint progress without worker or writer authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxCheckpoint {
    /// Recorded capture or writer-release phase.
    pub phase: SandboxCheckpointPhase,
    /// Original capture request timestamp.
    pub started_at: String,
    /// Monotonic checkpoint epoch.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub epoch: u64,
}

/// Latest completed whole-home snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxBackgroundSave {
    /// Safe background save stage; live user actions remain available.
    pub phase: String,
    /// Original save request time.
    pub started_at: String,
    /// Actual provider capture boundary, once observed.
    pub captured_at: Option<String>,
    /// Reserved checkpoint epoch.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub epoch: u64,
}

/// Safe projection of the currently admitted lifecycle operation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case")]
pub enum SandboxLifecycleOperation {
    /// Pause the Sandbox without changing its identity.
    Pause {
        /// Recorded operation phase.
        phase: SandboxPausePhase,
        /// Original operation request timestamp.
        started_at: String,
    },
    /// Restore the Sandbox and its recorded conversations.
    Revive {
        /// Recorded operation phase.
        phase: SandboxRevivePhase,
        /// Original operation request timestamp.
        started_at: String,
    },
    /// Replace the runtime while retaining the Sandbox home.
    ImageRotation {
        /// Recorded operation phase.
        phase: SandboxImageRotationPhase,
        /// Original operation request timestamp.
        started_at: String,
    },
}

string_enum! { /// Confirmed cause or unknown cause of the latest interruption.
    pub enum SandboxInterruptionCause {
        OutOfMemory => "out_of_memory", RuntimeRestarted => "runtime_restarted",
        RuntimeUnavailable => "runtime_unavailable"
    }
}

/// Latest recorded unexpected runtime interruption, retained after recovery.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxInterruption {
    /// Only explicit provider evidence can identify memory exhaustion.
    pub cause: SandboxInterruptionCause,
    /// Time CMA admitted the corresponding recovery operation.
    pub detected_at: String,
    /// Original immutable workload memory limit in MiB.
    pub memory_limit_mib: u32,
}

/// Authoritative transition eligibility and recorded lifecycle progress.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxLifecycle {
    /// Optional latest interruption; older servers may omit this field.
    pub interruption: Option<SandboxInterruption>,
    /// Eligibility to begin a new pause.
    pub pause: SandboxLifecycleEligibility,
    /// Eligibility to begin a new revival.
    pub revive: SandboxLifecycleEligibility,
    /// Active checkpoint, when present.
    pub checkpoint: Option<SandboxCheckpoint>,
    /// Offline export progress, independent of live writer availability.
    pub background_save: Option<SandboxBackgroundSave>,
    /// Admitted lifecycle operation, when present.
    pub operation: Option<SandboxLifecycleOperation>,
}
