use super::*;

pub(crate) enum LaunchResourceEffect {
    CreateSandbox {
        caller: Caller,
        authority: Capability,
        request: CreateSandbox,
    },
    ReviveSandbox {
        caller: Caller,
        sandbox_id: SandboxId,
        launch_id: String,
        reused: bool,
    },
    RepairSandboxServiceGrant {
        caller: Caller,
        sandbox_id: SandboxId,
        launch_id: String,
    },
    CreateWorkspace {
        caller: Caller,
        authority: Capability,
        sandbox_id: SandboxId,
        request: CreateWorkspace,
        launch_id: String,
    },
    CreateAgent {
        initiation: Option<Box<layer2_core::trigger_domain::LaunchInitiation>>,
        caller: Caller,
        authority: Capability,
        workspace_id: WorkspaceId,
        request: CreateAgent,
        reuse_existing: bool,
    },
}

impl LaunchResourceEffect {
    pub(crate) const fn kind(&self) -> LaunchEffectKind {
        match self {
            Self::CreateSandbox { .. } => LaunchEffectKind::CreateSandbox,
            Self::ReviveSandbox { .. } => LaunchEffectKind::ReviveSandbox,
            Self::RepairSandboxServiceGrant { .. } => LaunchEffectKind::RepairSandboxServiceGrant,
            Self::CreateWorkspace { .. } => LaunchEffectKind::CreateWorkspace,
            Self::CreateAgent { .. } => LaunchEffectKind::CreateAgent,
        }
    }
}
