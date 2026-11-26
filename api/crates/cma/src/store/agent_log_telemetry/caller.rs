//! Closed labels: callers cannot supply resource identities or error strings.
use layer2_core::sandbox::AgentWorkStep;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum ScanCaller {
    WorkerPublication(AgentWorkStep),
    CapacitySettlement(AgentWorkStep),
    RuntimeReconciliation,
    RecoveryProof,
    InactivityProof,
    TitleDiscovery,
    PublicWindowPrefix,
    ConversationPublication,
    Checkpoint,
    CommandAdmission,
    CommandReconciliation,
    UnknownCommitProof,
    Other,
}

// Only meaningful caller/step pairs exist; non-worker scans use not_applicable.
pub(super) const CALLERS: [ScanCaller; 47] = [
    ScanCaller::WorkerPublication(AgentWorkStep::InstallSkill),
    ScanCaller::WorkerPublication(AgentWorkStep::WaitForWorkspace),
    ScanCaller::WorkerPublication(AgentWorkStep::AddWorktree),
    ScanCaller::WorkerPublication(AgentWorkStep::MaterializeHome),
    ScanCaller::WorkerPublication(AgentWorkStep::OpenConversation),
    ScanCaller::WorkerPublication(AgentWorkStep::PublishReady),
    ScanCaller::WorkerPublication(AgentWorkStep::PublishConnection),
    ScanCaller::WorkerPublication(AgentWorkStep::DispatchFirstMessage),
    ScanCaller::WorkerPublication(AgentWorkStep::FenceInteraction),
    ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess),
    ScanCaller::WorkerPublication(AgentWorkStep::PublishStopped),
    ScanCaller::WorkerPublication(AgentWorkStep::EnsureProcessClosed),
    ScanCaller::WorkerPublication(AgentWorkStep::RemoveHome),
    ScanCaller::WorkerPublication(AgentWorkStep::RemoveWorktree),
    ScanCaller::WorkerPublication(AgentWorkStep::PublishDeleted),
    ScanCaller::WorkerPublication(AgentWorkStep::None),
    ScanCaller::CapacitySettlement(AgentWorkStep::InstallSkill),
    ScanCaller::CapacitySettlement(AgentWorkStep::WaitForWorkspace),
    ScanCaller::CapacitySettlement(AgentWorkStep::AddWorktree),
    ScanCaller::CapacitySettlement(AgentWorkStep::MaterializeHome),
    ScanCaller::CapacitySettlement(AgentWorkStep::OpenConversation),
    ScanCaller::CapacitySettlement(AgentWorkStep::PublishReady),
    ScanCaller::CapacitySettlement(AgentWorkStep::PublishConnection),
    ScanCaller::CapacitySettlement(AgentWorkStep::DispatchFirstMessage),
    ScanCaller::CapacitySettlement(AgentWorkStep::FenceInteraction),
    ScanCaller::CapacitySettlement(AgentWorkStep::CloseProcess),
    ScanCaller::CapacitySettlement(AgentWorkStep::PublishStopped),
    ScanCaller::CapacitySettlement(AgentWorkStep::EnsureProcessClosed),
    ScanCaller::CapacitySettlement(AgentWorkStep::RemoveHome),
    ScanCaller::CapacitySettlement(AgentWorkStep::RemoveWorktree),
    ScanCaller::CapacitySettlement(AgentWorkStep::PublishDeleted),
    ScanCaller::CapacitySettlement(AgentWorkStep::None),
    ScanCaller::RuntimeReconciliation,
    ScanCaller::RecoveryProof,
    ScanCaller::InactivityProof,
    ScanCaller::TitleDiscovery,
    ScanCaller::PublicWindowPrefix,
    ScanCaller::ConversationPublication,
    ScanCaller::Checkpoint,
    ScanCaller::CommandAdmission,
    ScanCaller::UnknownCommitProof,
    ScanCaller::Other,
    ScanCaller::CommandReconciliation,
    ScanCaller::WorkerPublication(AgentWorkStep::RestartConversation),
    ScanCaller::CapacitySettlement(AgentWorkStep::RestartConversation),
    ScanCaller::WorkerPublication(AgentWorkStep::RestartTeardown),
    ScanCaller::CapacitySettlement(AgentWorkStep::RestartTeardown),
];

impl ScanCaller {
    pub(super) fn index(self) -> usize {
        match self {
            Self::WorkerPublication(AgentWorkStep::RestartConversation) => 43,
            Self::CapacitySettlement(AgentWorkStep::RestartConversation) => 44,
            Self::WorkerPublication(AgentWorkStep::RestartTeardown) => 45,
            Self::CapacitySettlement(AgentWorkStep::RestartTeardown) => 46,
            Self::WorkerPublication(step) => step_index(step),
            Self::CapacitySettlement(step) => 16 + step_index(step),
            Self::RuntimeReconciliation => 32,
            Self::RecoveryProof => 33,
            Self::InactivityProof => 34,
            Self::TitleDiscovery => 35,
            Self::PublicWindowPrefix => 36,
            Self::ConversationPublication => 37,
            Self::Checkpoint => 38,
            Self::CommandAdmission => 39,
            Self::UnknownCommitProof => 40,
            Self::Other => 41,
            Self::CommandReconciliation => 42,
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::WorkerPublication(_) => "worker_publication",
            Self::CapacitySettlement(_) => "capacity_settlement",
            Self::RuntimeReconciliation => "runtime_reconciliation",
            Self::RecoveryProof => "recovery_proof",
            Self::InactivityProof => "inactivity_proof",
            Self::TitleDiscovery => "title_discovery",
            Self::PublicWindowPrefix => "public_window_prefix",
            Self::ConversationPublication => "conversation_publication",
            Self::Checkpoint => "checkpoint",
            Self::CommandAdmission => "command_admission",
            Self::CommandReconciliation => "command_reconciliation",
            Self::UnknownCommitProof => "unknown_commit_proof",
            Self::Other => "other",
        }
    }
    pub(super) fn worker_step(self) -> &'static str {
        match self {
            Self::WorkerPublication(step) | Self::CapacitySettlement(step) => step.operation_name(),
            _ => "not_applicable",
        }
    }
}

fn step_index(step: AgentWorkStep) -> usize {
    match step {
        AgentWorkStep::InstallSkill => 0,
        AgentWorkStep::WaitForWorkspace => 1,
        AgentWorkStep::AddWorktree => 2,
        AgentWorkStep::MaterializeHome => 3,
        AgentWorkStep::OpenConversation => 4,
        AgentWorkStep::PublishReady => 5,
        AgentWorkStep::PublishConnection => 6,
        AgentWorkStep::DispatchFirstMessage => 7,
        AgentWorkStep::FenceInteraction => 8,
        AgentWorkStep::CloseProcess => 9,
        AgentWorkStep::PublishStopped => 10,
        AgentWorkStep::EnsureProcessClosed => 11,
        AgentWorkStep::RemoveHome => 12,
        AgentWorkStep::RemoveWorktree => 13,
        AgentWorkStep::PublishDeleted => 14,
        AgentWorkStep::None => 15,
        AgentWorkStep::RestartConversation => 16,
        AgentWorkStep::RestartTeardown => 17,
    }
}
