//! Process-start rollout gates. These decisions never supply caller authority,
//! rewrite durable state, extend a deadline, or authorize retry of an unknown effect.
//!
//! Deployment registration must land with its adapters and retention integration.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum BridgeFlag {
    Management,
    Ingress,
    Allocation,
    Execution,
    Outbound,
    Telegram,
    Discord,
    Lark,
    Feishu,
    V2UiTransport,
    TelegramNew,
    TriggerCommandAdmission,
}

impl BridgeFlag {
    pub(crate) const ALL: [Self; 12] = [
        Self::Management,
        Self::Ingress,
        Self::Allocation,
        Self::Execution,
        Self::Outbound,
        Self::Telegram,
        Self::Discord,
        Self::Lark,
        Self::Feishu,
        Self::V2UiTransport,
        Self::TelegramNew,
        Self::TriggerCommandAdmission,
    ];

    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Management => "CHRONO_BRIDGE_MANAGEMENT_ENABLED",
            Self::Ingress => "CHRONO_BRIDGE_INGRESS_ENABLED",
            Self::Allocation => "CHRONO_BRIDGE_ALLOCATION_ENABLED",
            Self::Execution => "CHRONO_BRIDGE_EXECUTION_ENABLED",
            Self::Outbound => "CHRONO_BRIDGE_OUTBOUND_ENABLED",
            Self::Telegram => "CHRONO_BRIDGE_TELEGRAM_ENABLED",
            Self::TelegramNew => "CHRONO_BRIDGE_TELEGRAM_NEW_ENABLED",
            Self::Discord => "CHRONO_BRIDGE_DISCORD_ENABLED",
            Self::Lark => "CHRONO_BRIDGE_LARK_ENABLED",
            Self::Feishu => "CHRONO_BRIDGE_FEISHU_ENABLED",
            Self::V2UiTransport => "CHRONO_BRIDGE_V2_UI_TRANSPORT_ENABLED",
            Self::TriggerCommandAdmission => "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED",
        }
    }
}

/// Deployment selection is distinct from platform identity or authorization.
/// The composition adapter must exhaustively map the core BridgePlatform enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BridgePlatformFlag {
    Telegram,
    TelegramNew,
    Discord,
    Lark,
    Feishu,
}

impl BridgePlatformFlag {
    const fn flag(self) -> BridgeFlag {
        match self {
            Self::Telegram => BridgeFlag::Telegram,
            Self::TelegramNew => BridgeFlag::TelegramNew,
            Self::Discord => BridgeFlag::Discord,
            Self::Lark => BridgeFlag::Lark,
            Self::Feishu => BridgeFlag::Feishu,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BridgeStage {
    Management,
    Ingress,
    Allocation,
    Execution,
    Outbound,
}

impl BridgeStage {
    const fn flag(self) -> BridgeFlag {
        match self {
            Self::Management => BridgeFlag::Management,
            Self::Ingress => BridgeFlag::Ingress,
            Self::Allocation => BridgeFlag::Allocation,
            Self::Execution => BridgeFlag::Execution,
            Self::Outbound => BridgeFlag::Outbound,
        }
    }
}

/// The adapter must classify authoritative persisted evidence, never an HTTP
/// timeout or the flag's previous value. This is a gate input, not a ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum AcceptedEffectState {
    ReadyBeforeEffect,
    EffectStarted,
    OutcomeUnknown,
    Terminal,
    ExpiredBeforeEffect,
}

/// Disabling execution stops admission/release. Commands already released into
/// a shared Agent FIFO drain under ordinary authority without changing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum ChannelCommandGateState {
    HeldBeforeRelease,
    ReleasedIntoSharedFifo,
    DispatchBound,
    Terminal,
    ExpiredBeforeDispatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum AcceptedWorkDecision {
    /// Proceed only after the ordinary current authority and ledger checks.
    CheckAuthority,
    /// Retain identity, queue position, original deadlines and effect evidence.
    Hold(BridgeFlag),
    /// Observe/record the existing effect; this never authorizes another send.
    ReconcileExisting,
    /// Retain uncertainty; only the audited recovery command can allow a retry.
    RequireExplicitRecovery,
    ObserveTerminal,
    RecordExpiry,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BridgeFeatureFlags {
    enabled: [bool; 12],
}

impl BridgeFeatureFlags {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let mut result = Self::default();
        for flag in BridgeFlag::ALL {
            result.enabled[flag as usize] = match lookup(flag.key())?.as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                Some(_) => return Err(format!("{} must be true or false", flag.key())),
            };
        }
        Ok(result)
    }

    pub(crate) fn enabled(self, flag: BridgeFlag) -> bool {
        self.enabled[flag as usize]
    }

    /// Independent stage flags allow ingress-off draining and outbound-off
    /// execution. Enabling a stage never implicitly enables another stage.
    pub(crate) fn check_new_effect(
        self,
        stage: BridgeStage,
        platform: BridgePlatformFlag,
    ) -> Result<(), BridgeFlag> {
        for flag in [stage.flag(), platform.flag()] {
            if !self.enabled(flag) {
                return Err(flag);
            }
        }
        Ok(())
    }

    /// Ordinary human/service/Trigger commands never depend on Bridge flags.
    /// Only an authoritative Channel source may provide Some(platform). Apply
    /// this to new admission/release, not already released FIFO commands.
    pub(crate) fn check_command_release(
        self,
        channel_platform: Option<BridgePlatformFlag>,
    ) -> Result<(), BridgeFlag> {
        channel_platform.map_or(Ok(()), |platform| {
            self.check_new_effect(BridgeStage::Execution, platform)
        })
    }

    #[cfg(test)]
    pub(crate) fn channel_command_work(
        self,
        platform: BridgePlatformFlag,
        state: ChannelCommandGateState,
    ) -> AcceptedWorkDecision {
        use AcceptedWorkDecision as D;
        use ChannelCommandGateState as S;
        match state {
            S::HeldBeforeRelease => match self.check_command_release(Some(platform)) {
                Ok(()) => D::CheckAuthority,
                Err(flag) => D::Hold(flag),
            },
            S::ReleasedIntoSharedFifo => D::CheckAuthority,
            S::DispatchBound => D::ReconcileExisting,
            S::Terminal => D::ObserveTerminal,
            S::ExpiredBeforeDispatch => D::RecordExpiry,
        }
    }

    #[cfg(test)]
    pub(crate) fn accepted_work(
        self,
        stage: BridgeStage,
        platform: BridgePlatformFlag,
        state: AcceptedEffectState,
    ) -> AcceptedWorkDecision {
        use AcceptedEffectState as S;
        use AcceptedWorkDecision as D;
        match state {
            S::ReadyBeforeEffect => match self.check_new_effect(stage, platform) {
                Ok(()) => D::CheckAuthority,
                Err(flag) => D::Hold(flag),
            },
            S::EffectStarted => D::ReconcileExisting,
            S::OutcomeUnknown => D::RequireExplicitRecovery,
            S::Terminal => D::ObserveTerminal,
            S::ExpiredBeforeEffect => D::RecordExpiry,
        }
    }
}

#[cfg(test)]
#[path = "bridge_flags/tests.rs"]
mod tests;
